//! `/events` WebSocket consumption, job following with a progress bar, Ctrl-C handling.

use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use bz_core::models::{Job, JobStatus};
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use serde_json::Value;
use tungstenite::{Message, WebSocket};

use crate::client::Client;
use crate::error::CliError;
use crate::i18n::t;

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// Install the Ctrl-C handler (idempotent). Long waits check [`interrupted`].
pub fn install_ctrlc() {
    let _ = ctrlc::set_handler(|| {
        if INTERRUPTED.swap(true, Ordering::SeqCst) {
            // second Ctrl-C: leave immediately
            std::process::exit(130);
        }
    });
}

pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

/// A connected `/events` stream with a short read timeout.
pub struct EventStream {
    ws: WebSocket<TcpStream>,
}

/// The event WebSocket was closed or failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamClosed;

impl std::fmt::Display for StreamClosed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("event stream closed")
    }
}

impl std::error::Error for StreamClosed {}

impl EventStream {
    /// Connect to `client.events_url()`; `None` if the daemon has no WebSocket.
    pub fn connect(client: &Client) -> Option<EventStream> {
        let url = client.events_url();
        let hostport = url.strip_prefix("ws://")?.split('/').next()?.to_string();
        let stream = TcpStream::connect(&hostport).ok()?;
        stream.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
        use tungstenite::client::IntoClientRequest;
        let mut req = url.as_str().into_client_request().ok()?;
        if let Some(t) = client.token() {
            req.headers_mut()
                .insert("Authorization", format!("Bearer {t}").parse().ok()?);
        }
        let (ws, _resp) = tungstenite::client::client(req, stream).ok()?;
        ws.get_ref()
            .set_read_timeout(Some(Duration::from_millis(400)))
            .ok()?;
        Some(EventStream { ws })
    }

    /// Next event, `Ok(None)` on timeout, `Err(StreamClosed)` when the socket is gone.
    pub fn poll(&mut self) -> Result<Option<Value>, StreamClosed> {
        match self.ws.read() {
            Ok(Message::Text(s)) => Ok(serde_json::from_str(&s).ok()),
            Ok(Message::Binary(b)) => Ok(serde_json::from_slice(&b).ok()),
            Ok(_) => Ok(None),
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                Ok(None)
            }
            Err(_) => Err(StreamClosed),
        }
    }
}

/// Progress display for one job.
pub struct JobBar {
    bar: Option<ProgressBar>,
}

impl JobBar {
    pub fn new(visible: bool, title: &str) -> JobBar {
        if !visible {
            return JobBar { bar: None };
        }
        let bar = ProgressBar::with_draw_target(Some(1000), ProgressDrawTarget::stderr());
        bar.set_style(
            ProgressStyle::with_template("{spinner} {prefix} [{bar:30}] {percent:>3}% {wide_msg}")
                .unwrap_or_else(|_| ProgressStyle::default_bar())
                .progress_chars("=> "),
        );
        bar.set_prefix(title.to_string());
        bar.enable_steady_tick(Duration::from_millis(120));
        JobBar { bar: Some(bar) }
    }

    pub fn update(&self, job: &Job) {
        if let Some(b) = &self.bar {
            b.set_prefix(job.title.clone());
            b.set_position((job.progress.clamp(0.0, 1.0) * 1000.0) as u64);
            b.set_message(job.message.clone().unwrap_or_default());
        }
    }

    pub fn finish(&self) {
        if let Some(b) = &self.bar {
            b.finish_and_clear();
        }
    }
}

/// Follow a job until it finishes. Uses `/events` and polls `GET /jobs/{id}` as a
/// safety net (or exclusively, if the WebSocket is unavailable). Ctrl-C detaches
/// (`CliError::Interrupted`); a failed job becomes `CliError::JobFailed`.
pub fn follow_job(client: &Client, job_id: &str, show_bar: bool) -> Result<Job, CliError> {
    follow_job_with(client, job_id, show_bar, &mut |_| {})
}

/// Like [`follow_job`], also passing every other event to `on_event`.
pub fn follow_job_with(
    client: &Client,
    job_id: &str,
    show_bar: bool,
    on_event: &mut dyn FnMut(&Value),
) -> Result<Job, CliError> {
    let mut ws = EventStream::connect(client);
    let bar = JobBar::new(show_bar, "job");
    let mut last_poll = Instant::now() - Duration::from_secs(10);
    let poll_every = if ws.is_some() {
        Duration::from_secs(2)
    } else {
        Duration::from_millis(400)
    };
    let result = loop {
        if interrupted() {
            bar.finish();
            eprintln!("{}", t("detached"));
            break Err(CliError::Interrupted);
        }
        if last_poll.elapsed() >= poll_every {
            last_poll = Instant::now();
            let job = client.job(job_id)?;
            bar.update(&job);
            if job.status != JobStatus::Running {
                break Ok(job);
            }
        }
        match ws.as_mut().map(EventStream::poll) {
            Some(Ok(Some(ev))) => {
                let ty = ev["type"].as_str().unwrap_or("");
                if ty.starts_with("job.") && ev["job"]["id"].as_str() == Some(job_id) {
                    if let Ok(job) = serde_json::from_value::<Job>(ev["job"].clone()) {
                        bar.update(&job);
                        if job.status != JobStatus::Running {
                            break Ok(job);
                        }
                    }
                } else {
                    on_event(&ev);
                }
            }
            Some(Ok(None)) => {}
            Some(Err(StreamClosed)) => ws = None,
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    };
    bar.finish();
    let job = result?;
    if job.status == JobStatus::Failed {
        return Err(CliError::JobFailed(format!(
            "{}: {}",
            job.title,
            job.message
                .clone()
                .unwrap_or_else(|| t("job_failed").into())
        )));
    }
    Ok(job)
}
