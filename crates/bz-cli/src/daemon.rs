//! Locating, auto-starting and stopping `bozkurtd`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::client::Client;
use crate::error::CliError;
use crate::i18n::t;

/// Default API port (`config.toml` → `api_port`).
pub const DEFAULT_API_PORT: u16 = 7878;

/// `BOZKURT_HOME` (explicit override first).
pub fn home(override_home: Option<&Path>) -> PathBuf {
    match override_home {
        Some(h) => h.to_path_buf(),
        None => bz_core::paths::default_home(),
    }
}

/// Read `api_port` from `<home>/config.toml`, falling back to 7878. Tolerates a
/// missing or partially invalid file.
pub fn api_port(home: &Path) -> u16 {
    let Ok(text) = std::fs::read_to_string(home.join("config.toml")) else {
        return DEFAULT_API_PORT;
    };
    api_port_from_toml(&text)
}

pub fn api_port_from_toml(text: &str) -> u16 {
    text.parse::<toml::Table>()
        .ok()
        .and_then(|t| t.get("api_port").and_then(|v| v.as_integer()))
        .and_then(|p| u16::try_from(p).ok())
        .filter(|p| *p != 0)
        .unwrap_or(DEFAULT_API_PORT)
}

/// Client for the local daemon of `home`.
pub fn client_for(home: &Path) -> Client {
    Client::local(api_port(home)).with_token_home(home)
}

fn exe_name() -> String {
    format!("bozkurtd{}", std::env::consts::EXE_SUFFIX)
}

/// `bozkurtd` next to the current executable, in `<home>/bin`, or on `PATH`.
pub fn find_daemon_exe(home: &Path) -> Option<PathBuf> {
    let name = exe_name();
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    {
        let c = dir.join(&name);
        if c.is_file() {
            return Some(c);
        }
    }
    let c = home.join("bin").join(&name);
    if c.is_file() {
        return Some(c);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(&name))
        .find(|c| c.is_file())
}

fn pid_file(home: &Path) -> PathBuf {
    home.join("tmp").join("bozkurtd.cli.pid")
}

/// Start `bozkurtd` detached from this process. Output goes to `<home>/logs/bozkurtd.log`.
pub fn spawn_detached(home: &Path) -> Result<u32, CliError> {
    let exe =
        find_daemon_exe(home).ok_or_else(|| CliError::Daemon(t("daemon_not_found").into()))?;
    let logs = home.join("logs");
    let _ = std::fs::create_dir_all(&logs);
    let _ = std::fs::create_dir_all(home.join("tmp"));
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(logs.join("bozkurtd.log"));
    let (out, err) = match log {
        Ok(f) => match f.try_clone() {
            Ok(f2) => (Stdio::from(f), Stdio::from(f2)),
            Err(_) => (Stdio::null(), Stdio::null()),
        },
        Err(_) => (Stdio::null(), Stdio::null()),
    };
    let mut cmd = Command::new(&exe);
    cmd.env("BOZKURT_HOME", home)
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err);
    detach(&mut cmd);
    let child = cmd
        .spawn()
        .map_err(|e| CliError::Daemon(format!("{}: {e}", exe.display())))?;
    let pid = child.id();
    let _ = std::fs::write(pid_file(home), pid.to_string());
    Ok(pid)
}

#[cfg(unix)]
fn detach(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
}

#[cfg(windows)]
fn detach(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
}

/// Wait until `/health` answers or `timeout` passes.
pub fn wait_healthy(client: &Client, timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if client.health().is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

/// Make sure the daemon answers. Starts it (and waits up to 10 s) when `autostart`.
pub fn ensure_running(
    client: &Client,
    home: &Path,
    autostart: bool,
    quiet: bool,
) -> Result<(), CliError> {
    if client.health().is_ok() {
        return Ok(());
    }
    if !autostart {
        return Err(CliError::Daemon(t("daemon_unreachable").into()));
    }
    if !quiet {
        eprintln!("{}", t("starting_daemon"));
    }
    spawn_detached(home)?;
    if wait_healthy(client, Duration::from_secs(10)) {
        Ok(())
    } else {
        Err(CliError::Daemon(t("daemon_unreachable").into()))
    }
}

/// Stop the running daemon: the PID holding the API port (from `GET /ports/{port}`),
/// else the PID `bz` recorded when it started it.
pub fn stop(client: &Client, home: &Path) -> Result<bool, CliError> {
    if client.health().is_err() {
        return Ok(false);
    }
    // graceful path first: services stop in order, the pid file is removed
    if client
        .call(reqwest::Method::POST, "/daemon/shutdown", None)
        .is_ok()
    {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(30) {
            if client.health().is_err() {
                let _ = std::fs::remove_file(pid_file(home));
                return Ok(true);
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    let pid = client
        .port_owner(client.port())
        .ok()
        .and_then(|o| o.pid)
        .or_else(|| {
            // the daemon's own pid file, then the one bz wrote when it spawned it
            [home.join("bozkurtd.pid"), pid_file(home)]
                .iter()
                .find_map(|f| {
                    std::fs::read_to_string(f)
                        .ok()
                        .and_then(|s| s.trim().parse().ok())
                })
        })
        .ok_or_else(|| CliError::Other("cannot determine the bozkurtd process id".into()))?;
    kill(pid)?;
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        if client.health().is_err() {
            let _ = std::fs::remove_file(pid_file(home));
            return Ok(true);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err(CliError::Other(format!(
        "bozkurtd (pid {pid}) did not stop within 10 s"
    )))
}

#[cfg(unix)]
fn kill(pid: u32) -> Result<(), CliError> {
    let pid = libc::pid_t::try_from(pid).map_err(|_| CliError::Other("bad pid".into()))?;
    // SAFETY: plain syscall with a pid and a signal number.
    let rc = unsafe { libc::kill(pid, libc::SIGTERM) };
    if rc == 0 {
        Ok(())
    } else {
        Err(CliError::Other(std::io::Error::last_os_error().to_string()))
    }
}

#[cfg(windows)]
fn kill(pid: u32) -> Result<(), CliError> {
    let st = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if st.success() {
        Ok(())
    } else {
        Err(CliError::Other(format!("taskkill failed for pid {pid}")))
    }
}

/// Run `bozkurtd` in the foreground, inheriting stdio. Returns its exit code.
pub fn run_foreground(home: &Path) -> Result<i32, CliError> {
    let exe =
        find_daemon_exe(home).ok_or_else(|| CliError::Daemon(t("daemon_not_found").into()))?;
    let st = Command::new(exe).env("BOZKURT_HOME", home).status()?;
    Ok(st.code().unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_api_port() {
        assert_eq!(api_port_from_toml("api_port = 9000\ntld = \"test\""), 9000);
        assert_eq!(api_port_from_toml("tld = \"test\""), 7878);
        assert_eq!(api_port_from_toml("not toml ["), 7878);
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(api_port(dir.path()), 7878);
        std::fs::write(dir.path().join("config.toml"), "api_port = 7999\n").unwrap();
        assert_eq!(api_port(dir.path()), 7999);
    }
}
