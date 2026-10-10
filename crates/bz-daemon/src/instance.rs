//! Single-instance guard (`$BOZKURT_HOME/bozkurtd.pid`) and logging setup.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context};

/// Holds the pid file; removes it on drop.
#[derive(Debug)]
pub struct PidLock {
    path: PathBuf,
}

/// Is `pid` a live process?
pub fn pid_alive(pid: u32) -> bool {
    if pid == std::process::id() {
        return true;
    }
    #[cfg(unix)]
    {
        // SAFETY: kill with signal 0 only checks for existence/permission.
        let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
        r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(windows)]
    {
        std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()))
            .unwrap_or(false)
    }
    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}

impl PidLock {
    /// Take the lock, failing when another live bozkurtd owns it.
    pub fn acquire(home: &Path) -> anyhow::Result<PidLock> {
        let path = home.join("bozkurtd.pid");
        for _ in 0..2 {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut f) => {
                    write!(f, "{}", std::process::id())?;
                    return Ok(PidLock { path });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let other = std::fs::read_to_string(&path)
                        .ok()
                        .and_then(|s| s.trim().parse::<u32>().ok());
                    match other {
                        Some(pid) if pid != std::process::id() && pid_alive(pid) => {
                            bail!(
                                "bozkurtd is already running (pid {pid}, {})",
                                path.display()
                            )
                        }
                        _ => {
                            // stale
                            let _ = std::fs::remove_file(&path);
                        }
                    }
                }
                Err(e) => return Err(e).with_context(|| format!("creating {}", path.display())),
            }
        }
        bail!("could not take {}", path.display())
    }
}

impl Drop for PidLock {
    fn drop(&mut self) {
        let mine = std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            == Some(std::process::id());
        if mine {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Writer that appends to `bozkurtd.log` (rotated at startup when > 10 MB).
#[derive(Clone)]
pub struct LogFile(Arc<Mutex<std::fs::File>>);

impl LogFile {
    pub fn open(dir: &Path) -> std::io::Result<LogFile> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join("bozkurtd.log");
        if std::fs::metadata(&path)
            .map(|m| m.len() > 10 << 20)
            .unwrap_or(false)
        {
            let _ = std::fs::rename(&path, dir.join("bozkurtd.log.1"));
        }
        let f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        Ok(LogFile(Arc::new(Mutex::new(f))))
    }
}

impl Write for LogFile {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).flush()
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogFile {
    type Writer = LogFile;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// tracing → stderr + `<logs>/bozkurtd.log` (`RUST_LOG`, default `info`).
pub fn init_logging(logs_dir: &Path) {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,hyper=warn,mdns_sd=warn"));
    let stderr = fmt::layer().with_writer(std::io::stderr);
    let file = LogFile::open(logs_dir)
        .ok()
        .map(|f| fmt::layer().with_ansi(false).with_writer(f));
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(stderr)
        .with(file)
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pid_lock() {
        let t = tempfile::tempdir().unwrap();
        let l = PidLock::acquire(t.path()).unwrap();
        assert!(t.path().join("bozkurtd.pid").exists());
        drop(l);
        assert!(!t.path().join("bozkurtd.pid").exists());
        // stale pid file is replaced
        std::fs::write(t.path().join("bozkurtd.pid"), "999999999").unwrap();
        let _l = PidLock::acquire(t.path()).unwrap();
    }
}
