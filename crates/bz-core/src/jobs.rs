//! Background jobs with progress reporting (`GET /jobs`, events `job.*`).

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::error::Result;
use crate::events::{Event, EventBus};
use crate::models::{Job, JobStatus};

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// How many finished jobs are kept for `GET /jobs`.
const KEEP_FINISHED: usize = 100;

#[derive(Debug, Default)]
struct Inner {
    jobs: HashMap<String, Job>,
    order: Vec<String>,
}

/// Registry of jobs. Cheap to clone.
#[derive(Debug, Clone)]
pub struct JobManager {
    inner: Arc<Mutex<Inner>>,
    events: EventBus,
}

/// Handle given to a running job to report progress.
#[derive(Debug, Clone)]
pub struct JobHandle {
    id: String,
    mgr: JobManager,
    last_emit: Arc<Mutex<Option<(Instant, f64)>>>,
}

impl JobHandle {
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Report progress (`0.0..=1.0`) and an optional status message. Events are
    /// throttled to one per 100 ms unless progress moved by ≥ 5 % or the message changed.
    pub fn progress(&self, fraction: f64, message: Option<&str>) {
        let fraction = fraction.clamp(0.0, 1.0);
        let job = {
            let mut g = self.mgr.inner.lock().expect("jobs lock");
            let Some(job) = g.jobs.get_mut(&self.id) else {
                return;
            };
            if job.status != JobStatus::Running {
                return;
            }
            let msg_changed = message.is_some() && job.message.as_deref() != message;
            job.progress = fraction.max(job.progress);
            if let Some(m) = message {
                job.message = Some(m.to_string());
            }
            let mut last = self.last_emit.lock().expect("emit lock");
            let due = match *last {
                None => true,
                Some((t, p)) => {
                    msg_changed || t.elapsed() >= Duration::from_millis(100) || fraction - p >= 0.05
                }
            };
            if !due {
                return;
            }
            *last = Some((Instant::now(), fraction));
            job.clone()
        };
        self.mgr.events.emit(Event::JobProgress { job });
    }

    /// Set the message without changing progress.
    pub fn message(&self, message: &str) {
        let p = self.mgr.get(&self.id).map(|j| j.progress).unwrap_or(0.0);
        self.progress(p, Some(message));
    }
}

impl JobManager {
    pub fn new(events: EventBus) -> JobManager {
        JobManager {
            inner: Arc::new(Mutex::new(Inner::default())),
            events,
        }
    }

    /// Register a job and run `f` on the tokio runtime. Returns the job id immediately.
    /// On `Ok(msg)` the job finishes `done` (with `msg` as message), on `Err` `failed`.
    pub fn spawn<F, Fut>(&self, title: impl Into<String>, f: F) -> String
    where
        F: FnOnce(JobHandle) -> Fut + Send + 'static,
        Fut: Future<Output = Result<Option<String>>> + Send + 'static,
    {
        let handle = self.register(title.into());
        let id = handle.id.clone();
        let mgr = self.clone();
        tokio::spawn(async move {
            let jid = handle.id.clone();
            let res = f(handle).await;
            mgr.finish(&jid, res);
        });
        id
    }

    /// Create a job entry in `running` state without spawning anything.
    pub fn register(&self, title: String) -> JobHandle {
        let id = uuid::Uuid::new_v4().to_string();
        let job = Job {
            id: id.clone(),
            title,
            status: JobStatus::Running,
            progress: 0.0,
            message: None,
            started_at: now(),
            finished_at: None,
        };
        {
            let mut g = self.inner.lock().expect("jobs lock");
            g.jobs.insert(id.clone(), job.clone());
            g.order.push(id.clone());
            Self::prune(&mut g);
        }
        self.events.emit(Event::JobProgress { job });
        JobHandle {
            id,
            mgr: self.clone(),
            last_emit: Arc::new(Mutex::new(None)),
        }
    }

    /// Mark a job finished.
    pub fn finish(&self, id: &str, res: Result<Option<String>>) {
        let job = {
            let mut g = self.inner.lock().expect("jobs lock");
            let Some(job) = g.jobs.get_mut(id) else {
                return;
            };
            job.finished_at = Some(now());
            match &res {
                Ok(msg) => {
                    job.status = JobStatus::Done;
                    job.progress = 1.0;
                    if msg.is_some() {
                        job.message = msg.clone();
                    }
                }
                Err(e) => {
                    job.status = JobStatus::Failed;
                    job.message = Some(e.to_string());
                }
            }
            job.clone()
        };
        match job.status {
            JobStatus::Done => self.events.emit(Event::JobDone { job }),
            _ => {
                tracing::warn!("job {} failed: {:?}", job.title, job.message);
                self.events.emit(Event::JobFailed { job })
            }
        }
    }

    fn prune(g: &mut Inner) {
        let finished: Vec<String> = g
            .order
            .iter()
            .filter(|id| {
                g.jobs
                    .get(*id)
                    .map(|j| j.status != JobStatus::Running)
                    .unwrap_or(true)
            })
            .cloned()
            .collect();
        if finished.len() > KEEP_FINISHED {
            let drop: Vec<String> = finished[..finished.len() - KEEP_FINISHED].to_vec();
            for id in &drop {
                g.jobs.remove(id);
            }
            g.order.retain(|id| !drop.contains(id));
        }
    }

    pub fn get(&self, id: &str) -> Option<Job> {
        self.inner.lock().expect("jobs lock").jobs.get(id).cloned()
    }

    /// All known jobs, newest first.
    pub fn list(&self) -> Vec<Job> {
        let g = self.inner.lock().expect("jobs lock");
        g.order
            .iter()
            .rev()
            .filter_map(|id| g.jobs.get(id).cloned())
            .collect()
    }

    /// Wait until the job is no longer running (polling; intended for tests and CLIs).
    pub async fn wait(&self, id: &str, timeout: Duration) -> Option<Job> {
        let start = Instant::now();
        loop {
            let j = self.get(id)?;
            if j.status != JobStatus::Running || start.elapsed() > timeout {
                return Some(j);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::CoreError;

    #[tokio::test]
    async fn job_lifecycle_and_events() {
        let bus = EventBus::default();
        let mut rx = bus.subscribe();
        let jm = JobManager::new(bus);
        let id = jm.spawn("test", |h| async move {
            h.progress(0.5, Some("half"));
            Ok(Some("all good".into()))
        });
        let j = jm.wait(&id, Duration::from_secs(5)).await.unwrap();
        assert_eq!(j.status, JobStatus::Done);
        assert_eq!(j.progress, 1.0);
        assert_eq!(j.message.as_deref(), Some("all good"));
        assert!(j.finished_at.is_some());
        let mut kinds = vec![];
        while let Ok(e) = rx.try_recv() {
            kinds.push(
                serde_json::to_value(&e).unwrap()["type"]
                    .as_str()
                    .unwrap()
                    .to_string(),
            );
        }
        assert_eq!(kinds.first().unwrap(), "job.progress");
        assert_eq!(kinds.last().unwrap(), "job.done");
        assert_eq!(jm.list().len(), 1);
    }

    #[tokio::test]
    async fn failing_job() {
        let jm = JobManager::new(EventBus::default());
        let id = jm.spawn("bad", |_h| async move {
            Err::<Option<String>, _>(CoreError::Upstream("boom".into()))
        });
        let j = jm.wait(&id, Duration::from_secs(5)).await.unwrap();
        assert_eq!(j.status, JobStatus::Failed);
        assert!(j.message.unwrap().contains("boom"));
    }
}
