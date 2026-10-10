//! `bozkurtd` — the BOZKURT CODE daemon.
//!
//! ```text
//! bozkurtd [--home DIR] [--port N] [--foreground] [--version]
//! ```
//! Without `--foreground` the daemon detaches (re-executes itself in the background)
//! and prints the child pid.

use std::path::PathBuf;
use std::process::ExitCode;

use bz_daemon::instance::{init_logging, PidLock};
use bz_daemon::{Daemon, DaemonOptions};

struct Args {
    home: Option<PathBuf>,
    port: Option<u16>,
    foreground: bool,
}

const USAGE: &str = "usage: bozkurtd [--home DIR] [--port N] [--foreground] [--version]";

fn parse_args() -> Result<Option<Args>, String> {
    let mut a = Args {
        home: None,
        port: None,
        foreground: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let (k, inline) = match arg.split_once('=') {
            Some((k, v)) if k.starts_with("--") => (k.to_string(), Some(v.to_string())),
            _ => (arg.clone(), None),
        };
        let mut val = |name: &str| {
            inline
                .clone()
                .or_else(|| it.next())
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match k.as_str() {
            "--home" => a.home = Some(PathBuf::from(val("--home")?)),
            "--port" => {
                let v = val("--port")?;
                a.port = Some(v.parse().map_err(|_| format!("invalid port `{v}`"))?)
            }
            "--foreground" | "-f" => a.foreground = true,
            "--version" | "-V" => {
                println!("bozkurtd {}", bz_core::engine::VERSION);
                return Ok(None);
            }
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(None);
            }
            o => return Err(format!("unknown argument `{o}`\n{USAGE}")),
        }
    }
    Ok(Some(a))
}

/// Re-run ourselves with `--foreground`, detached from the terminal.
fn detach() -> std::io::Result<u32> {
    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(exe);
    cmd.args(std::env::args().skip(1))
        .arg("--foreground")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid is async-signal-safe and only detaches the child session.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    Ok(cmd.spawn()?.id())
}

async fn wait_for_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(Some(a)) => a,
        Ok(None) => return ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let home = args
        .home
        .clone()
        .unwrap_or_else(bz_core::paths::default_home);
    if !args.foreground {
        return match detach() {
            Ok(pid) => {
                println!("bozkurtd started in the background (pid {pid})");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("bozkurtd: could not start in the background: {e}");
                ExitCode::FAILURE
            }
        };
    }
    if let Err(e) = std::fs::create_dir_all(&home) {
        eprintln!("bozkurtd: creating {}: {e}", home.display());
        return ExitCode::FAILURE;
    }
    init_logging(&home.join("logs"));
    let _lock = match PidLock::acquire(&home) {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("{e:#}");
            return ExitCode::FAILURE;
        }
    };
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("bozkurtd: tokio runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    rt.block_on(async move {
        let mut opts = DaemonOptions::new(&home);
        opts.api_port = args.port;
        let daemon = match Daemon::start(opts).await {
            Ok(d) => d,
            Err(e) => {
                tracing::error!("{e:#}");
                return ExitCode::FAILURE;
            }
        };
        tokio::select! {
            _ = wait_for_signal() => {}
            _ = daemon.shutdown_requested() => tracing::info!("shutdown requested over the API"),
        }
        daemon.shutdown().await;
        ExitCode::SUCCESS
    })
}
