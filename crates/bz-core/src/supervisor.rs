//! Process supervisor: owns every child process BOZKURT starts.
//!
//! * spawns with explicit program/args/env/cwd (own process group on unix so the whole
//!   tree is signalled; one kill-on-close Job Object per child on Windows),
//! * captures stdout/stderr line by line into a per-process ring buffer and a rotating
//!   log file `logs/<name>.log` (5 MiB × 3), and publishes `log` events,
//! * restarts crashed processes with exponential backoff (reset after 30 s of uptime),
//! * graceful stop: SIGTERM (unix) / a service-native [`GracefulStop`] hook (Windows), then
//!   kill after a timeout (Windows: `TerminateJobObject` on the child's job),
//! * reports status, pid and uptime.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, watch};

use crate::error::{CoreError, Result};
use crate::events::{Event, EventBus};
use crate::models::ServiceStatus;
use crate::procguard::{self, ChildGuard, ChildRecord};

/// Lines kept in memory per process.
pub const RING_CAPACITY: usize = 2000;
/// Rotate a log file when it exceeds this size.
pub const LOG_ROTATE_BYTES: u64 = 5 * 1024 * 1024;
/// Rotated files kept (`.log.1` … `.log.N`).
pub const LOG_KEEP: usize = 3;

/// Restart behaviour after an unexpected exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestartPolicy {
    /// Give up after this many consecutive crashes (0 = never restart).
    pub max_restarts: u32,
    pub backoff_initial: Duration,
    pub backoff_max: Duration,
    /// A run longer than this resets the backoff/crash counter.
    pub reset_after: Duration,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        RestartPolicy {
            max_restarts: 10,
            backoff_initial: Duration::from_millis(500),
            backoff_max: Duration::from_secs(30),
            reset_after: Duration::from_secs(30),
        }
    }
}

impl RestartPolicy {
    /// Never restart.
    pub fn never() -> RestartPolicy {
        RestartPolicy {
            max_restarts: 0,
            ..Default::default()
        }
    }

    /// Backoff before restart number `n` (1-based): initial × 2^(n-1), capped.
    pub fn backoff(&self, n: u32) -> Duration {
        let factor = 2u32.saturating_pow(n.saturating_sub(1).min(20));
        self.backoff_initial
            .saturating_mul(factor)
            .min(self.backoff_max)
    }
}

/// A service-native way to ask a process to shut down (P1-8).
///
/// Used on **Windows** only, where there is no SIGTERM: windowless children
/// (`CREATE_NO_WINDOW`) ignore `taskkill` without `/F`, so a process without a hook is
/// force-killed (whole tree, via its Job Object) right away instead of waiting for
/// `stop_timeout`. On unix the supervisor keeps using SIGTERM to the process group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GracefulStop {
    /// Run a command, e.g. `mariadb-admin shutdown`, `pg_ctl stop -m fast`,
    /// `valkey-cli shutdown`. A non-zero exit means "did not work": force-kill now.
    Command { program: PathBuf, args: Vec<String> },
    /// `POST` to a URL, e.g. Caddy's admin `/stop`. A non-2xx answer means "did not work".
    HttpPost(String),
}

/// What to run.
#[derive(Debug, Clone)]
pub struct ProcessSpec {
    /// Unique name; also the log file stem and the `source` of `log` events.
    pub name: String,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    /// Added to (or overriding) the daemon's environment.
    pub env: BTreeMap<String, String>,
    pub restart: RestartPolicy,
    /// Grace period between the polite stop signal and a hard kill.
    pub stop_timeout: Duration,
    /// Log file; defaults to `<logs>/<name>.log` when started through the supervisor.
    pub log_file: Option<PathBuf>,
    /// Service-native graceful stop (Windows, see [`GracefulStop`]).
    pub graceful_stop: Option<GracefulStop>,
    /// Windows only: pass this verbatim as the command line after the program instead of
    /// quoting `args` (set by [`ProcessSpec::shell`], P1-11).
    pub raw_cmdline: Option<String>,
}

impl ProcessSpec {
    pub fn new(name: impl Into<String>, program: impl Into<PathBuf>) -> ProcessSpec {
        ProcessSpec {
            name: name.into(),
            program: program.into(),
            args: vec![],
            cwd: None,
            env: BTreeMap::new(),
            restart: RestartPolicy::default(),
            stop_timeout: Duration::from_secs(10),
            log_file: None,
            graceful_stop: None,
            raw_cmdline: None,
        }
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    pub fn env(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.env.insert(k.into(), v.into());
        self
    }

    pub fn restart(mut self, p: RestartPolicy) -> Self {
        self.restart = p;
        self
    }

    /// Run a command line through the platform shell (`sh -c` / `cmd /d /s /c`).
    ///
    /// Windows (P1-11): the command reaches cmd.exe verbatim as `/d /s /c "<command>"`
    /// (`raw_cmdline`). Rust's normal argument quoting escapes inner quotes as `\"`, which
    /// cmd.exe does not understand, so `node "my server.js"` would break. `/s` makes cmd
    /// strip exactly the outer quotes; `/d` skips AutoRun registry commands.
    pub fn shell(name: impl Into<String>, command: &str) -> ProcessSpec {
        if cfg!(windows) {
            let mut spec = ProcessSpec::new(name, "cmd.exe").args([
                "/d".to_string(),
                "/s".into(),
                "/c".into(),
                command.to_string(),
            ]);
            spec.raw_cmdline = Some(windows_shell_cmdline(command));
            spec
        } else {
            ProcessSpec::new(name, "/bin/sh").args(["-c".to_string(), command.to_string()])
        }
    }
}

/// Snapshot of a supervised process.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessInfo {
    pub name: String,
    pub status: ServiceStatus,
    pub pid: Option<u32>,
    pub uptime_secs: Option<u64>,
    pub restarts: u32,
    pub last_exit: Option<String>,
}

#[derive(Debug)]
struct ProcState {
    status: ServiceStatus,
    pid: Option<u32>,
    started: Option<Instant>,
    restarts: u32,
    last_exit: Option<String>,
}

#[derive(Debug)]
struct Managed {
    spec: ProcessSpec,
    state: Mutex<ProcState>,
    ring: Mutex<VecDeque<String>>,
    stop_tx: watch::Sender<bool>,
    done_rx: watch::Receiver<bool>,
}

/// Status change notification: (process name, info).
pub type StatusUpdate = (String, ProcessInfo);

/// The supervisor. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Supervisor {
    logs_dir: PathBuf,
    procs: Arc<Mutex<HashMap<String, Arc<Managed>>>>,
    events: EventBus,
    status_tx: broadcast::Sender<StatusUpdate>,
    /// `run/children.json` and its content (P1-7); `None` until the engine sets it.
    registry: Arc<Mutex<Option<(PathBuf, procguard::Registry)>>>,
}

impl Supervisor {
    pub fn new(logs_dir: PathBuf, events: EventBus) -> Supervisor {
        let (status_tx, _) = broadcast::channel(256);
        Supervisor {
            logs_dir,
            procs: Arc::new(Mutex::new(HashMap::new())),
            events,
            status_tx,
            registry: Arc::new(Mutex::new(None)),
        }
    }

    /// Record every running child in `path` (`run/children.json`) so the next daemon can
    /// reap them after a crash (P1-7). Call [`procguard::reap_orphans`] on the old file
    /// first: the file is rewritten on the next spawn.
    pub fn set_child_registry(&self, path: PathBuf) {
        *self.registry.lock().expect("registry") = Some((path, procguard::Registry::new()));
    }

    fn record(&self, name: &str, child: &Child, program: &Path) {
        let Some(pid) = child.id() else { return };
        let mut g = self.registry.lock().expect("registry");
        let Some((path, reg)) = g.as_mut() else {
            return;
        };
        let Some(start) = procguard::start_token(pid) else {
            return;
        };
        reg.insert(
            name.to_string(),
            ChildRecord {
                pid,
                pgid: cfg!(unix).then_some(pid),
                start,
                program: program.display().to_string(),
            },
        );
        if let Err(e) = procguard::save_registry(path, reg) {
            tracing::debug!("writing {path:?}: {e}");
        }
    }

    fn unrecord(&self, name: &str) {
        let mut g = self.registry.lock().expect("registry");
        if let Some((path, reg)) = g.as_mut() {
            if reg.remove(name).is_some() {
                if let Err(e) = procguard::save_registry(path, reg) {
                    tracing::debug!("writing {path:?}: {e}");
                }
            }
        }
    }

    /// Subscribe to status transitions of all processes.
    pub fn subscribe_status(&self) -> broadcast::Receiver<StatusUpdate> {
        self.status_tx.subscribe()
    }

    fn get(&self, name: &str) -> Option<Arc<Managed>> {
        self.procs.lock().expect("procs").get(name).cloned()
    }

    /// Start `spec`. If a process with the same name is alive it is an error
    /// (`Conflict`); a stopped/crashed entry is replaced.
    pub async fn start(&self, mut spec: ProcessSpec) -> Result<ProcessInfo> {
        if spec.log_file.is_none() {
            spec.log_file = Some(self.logs_dir.join(format!("{}.log", spec.name)));
        }
        let (stop_tx, stop_rx) = watch::channel(false);
        let (done_tx, done_rx) = watch::channel(false);
        // Check-and-insert under one lock scope (P1-5): two concurrent starts of the same
        // name must not both spawn. `spawn_child` is synchronous (fork/exec or
        // CreateProcess), so holding the std mutex across it is short and never awaits.
        let (m, child) = {
            let mut procs = self.procs.lock().expect("procs");
            if let Some(m) = procs.get(&spec.name) {
                if !*m.done_rx.borrow() {
                    return Err(CoreError::Conflict(format!(
                        "{} is already running",
                        spec.name
                    )));
                }
            }
            // spawn the first instance synchronously so spawn errors surface to the caller;
            // on error nothing was inserted, so there is nothing to roll back
            let (child, guard) = spawn_child(&spec)?;
            let m = Arc::new(Managed {
                spec: spec.clone(),
                state: Mutex::new(ProcState {
                    status: ServiceStatus::Running,
                    pid: child.id(),
                    started: Some(Instant::now()),
                    restarts: 0,
                    last_exit: None,
                }),
                ring: Mutex::new(VecDeque::with_capacity(256)),
                stop_tx,
                done_rx,
            });
            procs.insert(spec.name.clone(), m.clone());
            (m, (child, guard))
        };
        self.record(&spec.name, &child.0, &spec.program);
        self.notify(&m);
        let info = info_of(&m);
        let sup = self.clone();
        tokio::spawn(async move {
            sup.run_loop(m, child, stop_rx).await;
            let _ = done_tx.send(true);
        });
        Ok(info)
    }

    async fn run_loop(
        &self,
        m: Arc<Managed>,
        first: (Child, ChildGuard),
        mut stop_rx: watch::Receiver<bool>,
    ) {
        let (mut child, mut guard) = first;
        let mut crashes: u32 = 0;
        loop {
            let readers = self.attach_output(&m, &mut child);
            let started = Instant::now();
            let exit = tokio::select! {
                status = child.wait() => Some(status),
                _ = stop_rx.changed() => None,
            };
            let Some(status) = exit else {
                // stop requested
                self.set_status(&m, ServiceStatus::Stopping, None);
                terminate(&mut child, &guard, &m.spec).await;
                self.unrecord(&m.spec.name);
                drain(readers).await;
                self.set_status(&m, ServiceStatus::Stopped, None);
                return;
            };
            self.unrecord(&m.spec.name);
            drain(readers).await;
            let desc = match &status {
                Ok(s) => s.to_string(),
                Err(e) => format!("wait failed: {e}"),
            };
            let clean = matches!(&status, Ok(s) if s.success());
            self.push_line(&m, &format!("[bozkurt] process exited: {desc}"));
            if *stop_rx.borrow() {
                self.set_status(&m, ServiceStatus::Stopped, Some(desc));
                return;
            }
            if started.elapsed() >= m.spec.restart.reset_after {
                crashes = 0;
            }
            crashes += 1;
            let policy = m.spec.restart;
            if clean && policy.max_restarts == 0 {
                self.set_status(&m, ServiceStatus::Stopped, Some(desc));
                return;
            }
            self.set_status(&m, ServiceStatus::Crashed, Some(desc));
            if crashes > policy.max_restarts {
                if policy.max_restarts > 0 {
                    self.push_line(&m, "[bozkurt] giving up after repeated crashes");
                }
                return;
            }
            let delay = policy.backoff(crashes);
            self.push_line(
                &m,
                &format!(
                    "[bozkurt] restarting in {} ms (attempt {crashes})",
                    delay.as_millis()
                ),
            );
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = stop_rx.changed() => {
                    self.set_status(&m, ServiceStatus::Stopped, None);
                    return;
                }
            }
            self.set_status(&m, ServiceStatus::Starting, None);
            match spawn_child(&m.spec) {
                Ok((c, g)) => {
                    child = c;
                    guard = g;
                    self.record(&m.spec.name, &child, &m.spec.program);
                    {
                        let mut st = m.state.lock().expect("state");
                        st.status = ServiceStatus::Running;
                        st.pid = child.id();
                        st.started = Some(Instant::now());
                        st.restarts += 1;
                    }
                    self.notify(&m);
                }
                Err(e) => {
                    self.push_line(&m, &format!("[bozkurt] restart failed: {e}"));
                    self.set_status(&m, ServiceStatus::Crashed, Some(e.to_string()));
                    return;
                }
            }
        }
    }

    fn attach_output(
        &self,
        m: &Arc<Managed>,
        child: &mut Child,
    ) -> Vec<tokio::task::JoinHandle<()>> {
        let mut handles = vec![];
        if let Some(out) = child.stdout.take() {
            let (sup, mm) = (self.clone(), m.clone());
            handles.push(tokio::spawn(async move {
                read_lines_lossy(out, |l| sup.push_line(&mm, l)).await;
            }));
        }
        if let Some(err) = child.stderr.take() {
            let (sup, mm) = (self.clone(), m.clone());
            handles.push(tokio::spawn(async move {
                read_lines_lossy(err, |l| sup.push_line(&mm, l)).await;
            }));
        }
        handles
    }

    fn push_line(&self, m: &Managed, line: &str) {
        {
            let mut r = m.ring.lock().expect("ring");
            if r.len() >= RING_CAPACITY {
                r.pop_front();
            }
            r.push_back(line.to_string());
        }
        if let Some(f) = &m.spec.log_file {
            let stamped = format!(
                "{} {line}\n",
                chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f")
            );
            if let Err(e) = append_rotating(f, stamped.as_bytes()) {
                tracing::debug!("log write {f:?}: {e}");
            }
        }
        self.events.emit(Event::Log {
            source: m.spec.name.clone(),
            line: line.to_string(),
        });
    }

    fn set_status(&self, m: &Managed, s: ServiceStatus, last_exit: Option<String>) {
        {
            let mut st = m.state.lock().expect("state");
            st.status = s;
            if matches!(s, ServiceStatus::Stopped | ServiceStatus::Crashed) {
                st.pid = None;
                st.started = None;
            }
            if last_exit.is_some() {
                st.last_exit = last_exit;
            }
        }
        self.notify(m);
    }

    fn notify(&self, m: &Managed) {
        let _ = self.status_tx.send((m.spec.name.clone(), info_of(m)));
    }

    /// Stop gracefully and wait until the process (tree) is gone.
    pub async fn stop(&self, name: &str) -> Result<()> {
        let Some(m) = self.get(name) else {
            return Ok(());
        };
        let _ = m.stop_tx.send(true);
        let mut done = m.done_rx.clone();
        let wait = m.spec.stop_timeout + Duration::from_secs(5);
        let _ = tokio::time::timeout(wait, async {
            while !*done.borrow() {
                if done.changed().await.is_err() {
                    break;
                }
            }
        })
        .await;
        Ok(())
    }

    /// Stop then start again with the same spec (or `spec` if given).
    pub async fn restart(&self, name: &str, spec: Option<ProcessSpec>) -> Result<ProcessInfo> {
        let spec = match spec {
            Some(s) => s,
            None => self
                .get(name)
                .map(|m| m.spec.clone())
                .ok_or_else(|| CoreError::NotFound(format!("process {name}")))?,
        };
        self.stop(name).await?;
        self.start(spec).await
    }

    /// Stop every process (daemon shutdown).
    pub async fn stop_all(&self) {
        let names: Vec<String> = self.procs.lock().expect("procs").keys().cloned().collect();
        futures_util::future::join_all(names.iter().map(|n| self.stop(n))).await;
    }

    /// Current info, or `None` if never started.
    pub fn info(&self, name: &str) -> Option<ProcessInfo> {
        self.get(name).map(|m| info_of(&m))
    }

    pub fn list(&self) -> Vec<ProcessInfo> {
        let procs = self.procs.lock().expect("procs");
        let mut v: Vec<_> = procs.values().map(|m| info_of(m)).collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    /// True if the process is alive (running or waiting to restart).
    pub fn is_active(&self, name: &str) -> bool {
        self.get(name).is_some_and(|m| !*m.done_rx.borrow())
    }

    /// Last `n` captured lines (from memory; falls back to the log file).
    pub fn logs(&self, name: &str, n: usize) -> Vec<String> {
        if let Some(m) = self.get(name) {
            let r = m.ring.lock().expect("ring");
            let skip = r.len().saturating_sub(n);
            return r.iter().skip(skip).cloned().collect();
        }
        crate::fsutil::tail_lines(&self.logs_dir.join(format!("{name}.log")), n).unwrap_or_default()
    }

    /// PIDs of all running processes (to mark ports as ours).
    pub fn pids(&self) -> Vec<u32> {
        self.list().into_iter().filter_map(|i| i.pid).collect()
    }
}

/// Longest line kept from a child's output; the rest of the line is dropped (P1-6).
pub const MAX_LINE_BYTES: usize = 64 * 1024;

/// Read `r` line by line until EOF, calling `f` for each line (P1-6).
///
/// Never stops before EOF: invalid UTF-8 (Latin-1 / ANSI code pages, binary output) is
/// decoded lossily, and lines longer than [`MAX_LINE_BYTES`] are truncated (marked with
/// `…[truncated]`) while the rest of the line is still drained, so the child can never
/// block on a full pipe. A read error other than EOF is retried a few times, then the
/// pipe is closed.
pub async fn read_lines_lossy<R, F>(r: R, mut f: F)
where
    R: AsyncRead + Unpin,
    F: FnMut(&str),
{
    let mut reader = BufReader::with_capacity(16 * 1024, r);
    let mut line: Vec<u8> = Vec::with_capacity(256);
    let mut truncated = false;
    let mut errors = 0u32;
    loop {
        let (consumed, found_nl) = match reader.fill_buf().await {
            Ok([]) => break,
            Ok(buf) => {
                errors = 0;
                let (chunk, nl) = match buf.iter().position(|b| *b == b'\n') {
                    Some(i) => (&buf[..i], true),
                    None => (buf, false),
                };
                let room = MAX_LINE_BYTES.saturating_sub(line.len());
                if chunk.len() > room {
                    truncated = true;
                }
                line.extend_from_slice(&chunk[..chunk.len().min(room)]);
                (chunk.len() + usize::from(nl), nl)
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                errors += 1;
                if errors > 3 {
                    tracing::debug!("output pipe read failed: {e}");
                    break;
                }
                continue;
            }
        };
        reader.consume(consumed);
        if found_nl {
            emit_line(&mut line, &mut truncated, &mut f);
        }
    }
    if !line.is_empty() || truncated {
        emit_line(&mut line, &mut truncated, &mut f);
    }
}

fn emit_line<F: FnMut(&str)>(line: &mut Vec<u8>, truncated: &mut bool, f: &mut F) {
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    let mut s = String::from_utf8_lossy(line).into_owned();
    if *truncated {
        s.push_str("…[truncated]");
    }
    f(&s);
    line.clear();
    *truncated = false;
}

async fn drain(readers: Vec<tokio::task::JoinHandle<()>>) {
    for r in readers {
        let _ = tokio::time::timeout(Duration::from_millis(500), r).await;
    }
}

fn info_of(m: &Managed) -> ProcessInfo {
    let st = m.state.lock().expect("state");
    ProcessInfo {
        name: m.spec.name.clone(),
        status: st.status,
        pid: st.pid,
        uptime_secs: st.started.map(|s| s.elapsed().as_secs()),
        restarts: st.restarts,
        last_exit: st.last_exit.clone(),
    }
}

/// The raw command line `cmd.exe` gets for [`ProcessSpec::shell`] on Windows.
pub fn windows_shell_cmdline(command: &str) -> String {
    format!("/d /s /c \"{command}\"")
}

fn spawn_child(spec: &ProcessSpec) -> Result<(Child, ChildGuard)> {
    let mut cmd = Command::new(&spec.program);
    #[cfg(windows)]
    match &spec.raw_cmdline {
        Some(raw) => {
            cmd.raw_arg(raw);
        }
        None => {
            cmd.args(&spec.args);
        }
    }
    #[cfg(not(windows))]
    cmd.args(&spec.args);
    cmd.envs(&spec.env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(c) = &spec.cwd {
        cmd.current_dir(c);
    }
    #[cfg(unix)]
    cmd.process_group(0);
    #[cfg(target_os = "linux")]
    {
        // SAFETY: getpid is async-signal-safe; the hook only calls prctl/getppid (P1-7).
        let parent = unsafe { libc::getpid() };
        unsafe {
            cmd.pre_exec(move || procguard::linux_pdeathsig(parent));
        }
    }
    #[cfg(windows)]
    {
        // CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW
        cmd.creation_flags(0x0000_0200 | 0x0800_0000);
    }
    let child = cmd.spawn().map_err(|e| {
        CoreError::Other(anyhow::anyhow!(
            "failed to start {} ({}): {e}",
            spec.name,
            spec.program.display()
        ))
    })?;
    let guard = ChildGuard::attach(&child);
    Ok((child, guard))
}

#[cfg(unix)]
fn signal_group(pid: u32, sig: i32) {
    // SAFETY: plain syscall; a negative pid addresses the process group we created.
    unsafe {
        libc::kill(-(pid as i32), sig);
    }
}

/// Ask a process to stop through its [`GracefulStop`] hook. Returns true when the request
/// was accepted (command exited 0 / HTTP 2xx) within `timeout`.
pub async fn request_graceful_stop(g: &GracefulStop, timeout: Duration) -> bool {
    match g {
        GracefulStop::Command { program, args } => {
            let mut cmd = Command::new(program);
            cmd.args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true);
            #[cfg(windows)]
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            match cmd.spawn() {
                Ok(mut c) => matches!(
                    tokio::time::timeout(timeout, c.wait()).await,
                    Ok(Ok(s)) if s.success()
                ),
                Err(e) => {
                    tracing::debug!("graceful stop {}: {e}", program.display());
                    false
                }
            }
        }
        GracefulStop::HttpPost(url) => {
            let client = match reqwest::Client::builder()
                .no_proxy()
                .timeout(timeout)
                .build()
            {
                Ok(c) => c,
                Err(_) => return false,
            };
            matches!(client.post(url).send().await, Ok(r) if r.status().is_success())
        }
    }
}

/// Polite stop, then kill after `spec.stop_timeout`.
///
/// Unix: SIGTERM to the process group, SIGKILL after the timeout. Windows (P1-8): the
/// spec's [`GracefulStop`] hook if it has one (then wait up to the timeout), otherwise —
/// or when the hook fails — kill the whole tree at once with `TerminateJobObject`
/// (`taskkill /F /T` if the child has no job). No blocking call runs on the runtime.
async fn terminate(child: &mut Child, guard: &ChildGuard, spec: &ProcessSpec) {
    let timeout = spec.stop_timeout;
    let Some(pid) = child.id() else {
        let _ = child.wait().await;
        return;
    };
    #[cfg(unix)]
    {
        let _ = guard;
        signal_group(pid, libc::SIGTERM);
        if tokio::time::timeout(timeout, child.wait()).await.is_err() {
            signal_group(pid, libc::SIGKILL);
            let _ = child.kill().await;
            let _ = child.wait().await;
        } else {
            // the leader is gone; make sure stragglers in the group are too
            signal_group(pid, libc::SIGKILL);
        }
    }
    #[cfg(windows)]
    {
        let t0 = Instant::now();
        let accepted = match &spec.graceful_stop {
            Some(g) => request_graceful_stop(g, timeout).await,
            None => false,
        };
        let left = timeout.saturating_sub(t0.elapsed());
        let exited = accepted && tokio::time::timeout(left, child.wait()).await.is_ok();
        // kill the rest of the tree (or everything when the polite stop did not work)
        if !guard.kill_tree() && !exited {
            let _ = Command::new("taskkill")
                .args(["/F", "/T", "/PID", &pid.to_string()])
                .creation_flags(0x0800_0000)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await;
        }
        if !exited {
            let _ = child.kill().await;
        }
        let _ = child.wait().await;
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (guard, pid, timeout);
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
}

/// Append to `path`, rotating `path` → `path.1` → … when it exceeds [`LOG_ROTATE_BYTES`].
pub fn append_rotating(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() + data.len() as u64 > LOG_ROTATE_BYTES {
            rotate(path)?;
        }
    } else if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    f.write_all(data)
}

fn rotate(path: &Path) -> std::io::Result<()> {
    let name = |i: usize| PathBuf::from(format!("{}.{i}", path.display()));
    let _ = std::fs::remove_file(name(LOG_KEEP));
    for i in (1..LOG_KEEP).rev() {
        if name(i).exists() {
            std::fs::rename(name(i), name(i + 1))?;
        }
    }
    std::fs::rename(path, name(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_and_caps() {
        let p = RestartPolicy::default();
        assert_eq!(p.backoff(1), Duration::from_millis(500));
        assert_eq!(p.backoff(2), Duration::from_millis(1000));
        assert_eq!(p.backoff(4), Duration::from_millis(4000));
        assert_eq!(p.backoff(20), Duration::from_secs(30));
    }

    #[test]
    fn rotation() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("x.log");
        let chunk = vec![b'a'; (LOG_ROTATE_BYTES / 2) as usize + 1];
        for _ in 0..6 {
            append_rotating(&f, &chunk).unwrap();
        }
        assert!(f.exists());
        assert!(t.path().join("x.log.1").exists());
        assert!(t.path().join("x.log.3").exists());
        assert!(!t.path().join("x.log.4").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_capture_stop() {
        let t = tempfile::tempdir().unwrap();
        let bus = EventBus::default();
        let mut ev = bus.subscribe();
        let sup = Supervisor::new(t.path().to_path_buf(), bus);
        let spec = ProcessSpec::shell(
            "echoer",
            "echo hello $GREETING; echo err >&2; exec sleep 30",
        )
        .env("GREETING", "world");
        let info = sup.start(spec).await.unwrap();
        assert_eq!(info.status, ServiceStatus::Running);
        assert!(info.pid.is_some());
        for _ in 0..100 {
            if sup.logs("echoer", 10).len() >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let logs = sup.logs("echoer", 10);
        assert!(logs.contains(&"hello world".to_string()), "{logs:?}");
        assert!(logs.contains(&"err".to_string()));
        assert!(matches!(ev.recv().await.unwrap(), Event::Log { .. }));
        // a second start conflicts
        assert!(sup
            .start(ProcessSpec::shell("echoer", "true"))
            .await
            .is_err());
        let t0 = Instant::now();
        sup.stop("echoer").await.unwrap();
        assert!(
            t0.elapsed() < Duration::from_secs(5),
            "SIGTERM should stop sleep fast"
        );
        let i = sup.info("echoer").unwrap();
        assert_eq!(i.status, ServiceStatus::Stopped);
        assert_eq!(i.pid, None);
        let file = std::fs::read_to_string(t.path().join("echoer.log")).unwrap();
        assert!(file.contains("hello world"));
        // can start again after stop
        sup.start(ProcessSpec::shell("echoer", "exec sleep 30"))
            .await
            .unwrap();
        sup.stop_all().await;
        assert!(!sup.is_active("echoer"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn crash_restarts_with_backoff_then_gives_up() {
        let t = tempfile::tempdir().unwrap();
        let sup = Supervisor::new(t.path().to_path_buf(), EventBus::default());
        let policy = RestartPolicy {
            max_restarts: 2,
            backoff_initial: Duration::from_millis(20),
            ..Default::default()
        };
        let spec = ProcessSpec::shell("crasher", "echo boom; exit 3").restart(policy);
        sup.start(spec).await.unwrap();
        for _ in 0..200 {
            if !sup.is_active("crasher") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let i = sup.info("crasher").unwrap();
        assert_eq!(i.status, ServiceStatus::Crashed);
        assert_eq!(i.restarts, 2);
        let boom = sup
            .logs("crasher", 100)
            .iter()
            .filter(|l| *l == "boom")
            .count();
        assert_eq!(boom, 3);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stop_kills_stubborn_process_after_timeout() {
        let t = tempfile::tempdir().unwrap();
        let sup = Supervisor::new(t.path().to_path_buf(), EventBus::default());
        let mut spec = ProcessSpec::shell(
            "stubborn",
            "trap '' TERM; echo ready; while true; do sleep 0.1; done",
        );
        spec.stop_timeout = Duration::from_millis(300);
        sup.start(spec).await.unwrap();
        for _ in 0..100 {
            if sup.logs("stubborn", 5).iter().any(|l| l == "ready") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        sup.stop("stubborn").await.unwrap();
        assert_eq!(sup.info("stubborn").unwrap().status, ServiceStatus::Stopped);
    }

    #[tokio::test]
    async fn output_reader_survives_invalid_utf8_and_long_lines() {
        let mut data: Vec<u8> = b"ok\r\n".to_vec();
        data.extend_from_slice(b"caf\xe9 latin1\n"); // not UTF-8
        data.extend(std::iter::repeat_n(b'x', MAX_LINE_BYTES * 2 + 10));
        data.extend_from_slice(b"\nafter\nlast-no-newline");
        let mut got = vec![];
        read_lines_lossy(&data[..], |l| got.push(l.to_string())).await;
        assert_eq!(
            got.len(),
            5,
            "{:?}",
            got.iter().map(|l| l.len()).collect::<Vec<_>>()
        );
        assert_eq!(got[0], "ok");
        assert_eq!(got[1], "caf\u{fffd} latin1");
        assert!(got[2].starts_with("xxx") && got[2].ends_with("…[truncated]"));
        assert_eq!(got[2].len(), MAX_LINE_BYTES + "…[truncated]".len());
        assert_eq!(got[3], "after");
        assert_eq!(got[4], "last-no-newline");
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_starts_spawn_once() {
        let t = tempfile::tempdir().unwrap();
        let sup = Supervisor::new(t.path().to_path_buf(), EventBus::default());
        let starts = (0..8).map(|_| {
            let sup = sup.clone();
            tokio::spawn(async move {
                sup.start(ProcessSpec::shell("racer", "exec sleep 30"))
                    .await
                    .is_ok()
            })
        });
        let ok = futures_util::future::join_all(starts)
            .await
            .into_iter()
            .filter(|r| *r.as_ref().unwrap())
            .count();
        assert_eq!(ok, 1, "exactly one concurrent start may win");
        sup.stop_all().await;
        assert!(!sup.is_active("racer"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn graceful_stop_hooks_report_success() {
        let ok = GracefulStop::Command {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "exit 0".into()],
        };
        let bad = GracefulStop::Command {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "exit 3".into()],
        };
        let missing = GracefulStop::Command {
            program: "/definitely/not/here".into(),
            args: vec![],
        };
        let slow = GracefulStop::Command {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "sleep 5".into()],
        };
        let t = Duration::from_secs(2);
        assert!(request_graceful_stop(&ok, t).await);
        assert!(!request_graceful_stop(&bad, t).await);
        assert!(!request_graceful_stop(&missing, t).await);
        let t0 = Instant::now();
        assert!(!request_graceful_stop(&slow, Duration::from_millis(200)).await);
        assert!(t0.elapsed() < Duration::from_secs(2));
        // nothing listens on port 9 of loopback
        let http = GracefulStop::HttpPost("http://127.0.0.1:9/stop".into());
        assert!(!request_graceful_stop(&http, t).await);
    }

    #[test]
    fn windows_shell_command_line_is_verbatim() {
        assert_eq!(
            windows_shell_cmdline(r#"node "my server.js" --port %PORT%"#),
            r#"/d /s /c "node "my server.js" --port %PORT%""#
        );
        let s = ProcessSpec::shell("x", "echo hi");
        if cfg!(windows) {
            assert_eq!(s.raw_cmdline.as_deref(), Some(r#"/d /s /c "echo hi""#));
        } else {
            assert_eq!(s.raw_cmdline, None);
            assert_eq!(s.args, ["-c", "echo hi"]);
        }
    }

    #[tokio::test]
    async fn spawn_error_is_reported() {
        let t = tempfile::tempdir().unwrap();
        let sup = Supervisor::new(t.path().to_path_buf(), EventBus::default());
        let e = sup
            .start(ProcessSpec::new("nope", "/definitely/not/here"))
            .await
            .unwrap_err();
        assert!(e.to_string().contains("failed to start nope"));
    }
}
