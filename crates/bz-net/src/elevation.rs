//! Applying [`Plan`]s, with backups and OS elevation.
//!
//! * Non-elevated steps run in-process (file writes, commands).
//! * Each consecutive run of elevated steps is rendered into one script (`/bin/sh` on
//!   macOS/Linux, PowerShell on Windows) and executed through a single elevation prompt:
//!   macOS `osascript -e 'do shell script … with administrator privileges'`, Linux `pkexec`,
//!   Windows `Start-Process -Verb RunAs`.
//! * Before anything runs, every file the plan touches is copied to
//!   `<backups_dir>/<stamp>/<absolute path without root>`; the rendered scripts are kept there too.
//!
//! The rendering functions are pure and OS-independent so they can be unit-tested anywhere.
//! [`apply`] blocks (it may wait on a password dialog): call it from `spawn_blocking`.

use crate::error::{IoContext, NetError, Result};
use crate::plan::{Action, Os, Plan, Step};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

/// Outcome of [`apply`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApplyReport {
    /// Directory holding backups and rendered scripts for this run.
    pub backup_dir: PathBuf,
    /// `(original, backup copy)` for every file that existed and was backed up.
    pub backed_up: Vec<(PathBuf, PathBuf)>,
    /// Number of steps executed.
    pub steps_run: usize,
    /// Whether an elevation prompt was used.
    pub elevated: bool,
    /// Combined stdout/stderr of commands, for the action log.
    pub output: String,
}

/// Quote a string for POSIX `sh` (single quotes).
pub fn sh_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:=@%+,".contains(c))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Quote a string for PowerShell (single-quoted literal).
pub fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Where a backup copy of `path` goes inside `root`: the absolute path with its root/prefix
/// stripped (a Windows drive `C:` becomes a `C` directory).
pub fn backup_target(root: &Path, path: &Path) -> PathBuf {
    let mut out = root.to_path_buf();
    for c in path.components() {
        match c {
            Component::Prefix(p) => {
                let s: String = p
                    .as_os_str()
                    .to_string_lossy()
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric())
                    .collect();
                if !s.is_empty() {
                    out.push(s);
                }
            }
            Component::RootDir | Component::CurDir => {}
            Component::ParentDir => out.push("_up"),
            Component::Normal(n) => out.push(n),
        }
    }
    // Paths written as Windows strings parsed on unix ("C:\\Windows\\...") come through as one
    // component; make them filesystem-friendly.
    if !cfg!(windows) {
        let s = out.to_string_lossy().replace([':', '\\'], "/");
        return PathBuf::from(s.replace("//", "/"));
    }
    out
}

fn heredoc_delim(contents: &str) -> String {
    let mut n = 0u32;
    loop {
        let d = format!("BZ_EOF_{n}");
        if !contents.lines().any(|l| l == d) {
            return d;
        }
        n += 1;
    }
}

/// Render elevated steps (plus backup copies that could not be made unprivileged) as one
/// script for `os`.
pub fn render_script(
    os: Os,
    title: &str,
    steps: &[&Step],
    backups: &[(PathBuf, PathBuf)],
) -> String {
    match os {
        Os::Windows => render_powershell(title, steps, backups),
        Os::Macos | Os::Linux => render_sh(title, steps, backups),
    }
}

fn render_sh(title: &str, steps: &[&Step], backups: &[(PathBuf, PathBuf)]) -> String {
    let mut s = String::from("#!/bin/sh\n# BOZKURT CODE elevated script\n");
    s.push_str(&format!("# {}\nset -e\n", title.replace('\n', " ")));
    for (from, to) in backups {
        let f = sh_quote(&from.to_string_lossy());
        let t = sh_quote(&to.to_string_lossy());
        let parent = to
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        s.push_str(&format!(
            "# backup {f}\nif [ -e {f} ]; then mkdir -p {}; cp -p {f} {t}; fi\n",
            sh_quote(&parent)
        ));
    }
    for st in steps {
        s.push_str(&format!("# {}\n", st.description.replace('\n', " ")));
        match &st.action {
            Action::WriteFile {
                path,
                contents,
                mode,
            } => {
                let p = path.to_string_lossy();
                if let Some(parent) = path.parent() {
                    s.push_str(&format!(
                        "mkdir -p {}\n",
                        sh_quote(&parent.to_string_lossy())
                    ));
                }
                let d = heredoc_delim(contents);
                let body = contents.strip_suffix('\n').unwrap_or(contents);
                s.push_str(&format!("cat > {} <<'{d}'\n{body}\n{d}\n", sh_quote(&p)));
                if let Some(m) = mode {
                    s.push_str(&format!("chmod {:o} {}\n", m, sh_quote(&p)));
                }
            }
            Action::RemoveFile { path } => {
                s.push_str(&format!("rm -f {}\n", sh_quote(&path.to_string_lossy())));
            }
            Action::Run {
                program,
                args,
                ignore_failure,
            } => {
                let mut line = sh_quote(program);
                for a in args {
                    line.push(' ');
                    line.push_str(&sh_quote(a));
                }
                if *ignore_failure {
                    line.push_str(" || true");
                }
                s.push_str(&line);
                s.push('\n');
            }
            Action::Shell {
                script,
                ignore_failure,
            } => {
                if *ignore_failure {
                    s.push_str(&format!("( {} ) || true\n", script.trim()));
                } else {
                    s.push_str(script.trim());
                    s.push('\n');
                }
            }
        }
    }
    s
}

fn render_powershell(title: &str, steps: &[&Step], backups: &[(PathBuf, PathBuf)]) -> String {
    let mut s = String::from("# BOZKURT CODE elevated script\n");
    s.push_str(&format!(
        "# {}\n$ErrorActionPreference = 'Stop'\n",
        title.replace('\n', " ")
    ));
    for (from, to) in backups {
        let f = ps_quote(&from.to_string_lossy());
        let t = ps_quote(&to.to_string_lossy());
        let parent = to
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        s.push_str(&format!(
            "if (Test-Path -LiteralPath {f}) {{ New-Item -ItemType Directory -Force -Path {} | Out-Null; Copy-Item -LiteralPath {f} -Destination {t} -Force }}\n",
            ps_quote(&parent)
        ));
    }
    for st in steps {
        s.push_str(&format!("# {}\n", st.description.replace('\n', " ")));
        match &st.action {
            Action::WriteFile { path, contents, .. } => {
                let p = path.to_string_lossy();
                if let Some(parent) = path.parent() {
                    s.push_str(&format!(
                        "New-Item -ItemType Directory -Force -Path {} | Out-Null\n",
                        ps_quote(&parent.to_string_lossy())
                    ));
                }
                // Literal here-string; CRLF line endings for Windows text files.
                let body = contents.strip_suffix('\n').unwrap_or(contents);
                s.push_str(&format!(
                    "$bzContent = @'\n{body}\n'@\n[System.IO.File]::WriteAllText({}, ($bzContent -replace \"`r?`n\", \"`r`n\") + \"`r`n\", (New-Object System.Text.UTF8Encoding $false))\n",
                    ps_quote(&p)
                ));
            }
            Action::RemoveFile { path } => {
                let p = ps_quote(&path.to_string_lossy());
                s.push_str(&format!(
                    "if (Test-Path -LiteralPath {p}) {{ Remove-Item -LiteralPath {p} -Force }}\n"
                ));
            }
            Action::Run {
                program,
                args,
                ignore_failure,
            } => {
                let mut line = format!("& {}", ps_quote(program));
                for a in args {
                    line.push(' ');
                    line.push_str(&ps_quote(a));
                }
                s.push_str(&line);
                s.push('\n');
                if !*ignore_failure {
                    s.push_str("if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }\n");
                }
            }
            Action::Shell {
                script,
                ignore_failure,
            } => {
                if *ignore_failure {
                    s.push_str(&format!("try {{ {} }} catch {{ }}\n", script.trim()));
                } else {
                    s.push_str(script.trim());
                    s.push('\n');
                }
            }
        }
    }
    s.push_str("exit 0\n");
    s
}

/// The command that runs `script_path` with administrator rights on `os`.
pub fn elevation_command(os: Os, title: &str, script_path: &Path) -> (String, Vec<String>) {
    let script = script_path.to_string_lossy().to_string();
    match os {
        Os::Macos => {
            let inner = format!("/bin/sh {}", sh_quote(&script));
            let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
            let prompt = format!("BOZKURT CODE needs administrator rights to: {title}");
            (
                "osascript".into(),
                vec![
                    "-e".into(),
                    format!(
                        "do shell script \"{}\" with prompt \"{}\" with administrator privileges",
                        esc(&inner),
                        esc(&prompt)
                    ),
                ],
            )
        }
        Os::Linux => ("pkexec".into(), vec!["/bin/sh".into(), script]),
        Os::Windows => {
            let inner = format!(
                "$p = Start-Process -FilePath 'powershell.exe' -Verb RunAs -Wait -PassThru -WindowStyle Hidden -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File',{}; exit $p.ExitCode",
                ps_quote(&format!("\"{script}\""))
            );
            (
                "powershell.exe".into(),
                vec![
                    "-NoProfile".into(),
                    "-ExecutionPolicy".into(),
                    "Bypass".into(),
                    "-Command".into(),
                    inner,
                ],
            )
        }
    }
}

/// Run a command, capturing output; error on non-zero exit.
pub(crate) fn run_capture(program: &str, args: &[String]) -> Result<String> {
    let out = Command::new(program)
        .args(args)
        .output()
        .ctx(|| format!("running {program}"))?;
    let mut text = String::from_utf8_lossy(&out.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    if !out.status.success() {
        return Err(NetError::Command {
            command: format!("{program} {}", args.join(" ")),
            code: out.status.code(),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }
    Ok(text)
}

/// Like [`run_capture`] but returns `None` when the program cannot be started.
pub(crate) fn try_output(program: &str, args: &[&str]) -> Option<(bool, String)> {
    let out = Command::new(program).args(args).output().ok()?;
    let mut text = String::from_utf8_lossy(&out.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Some((out.status.success(), text))
}

fn run_step_unprivileged(os: Os, step: &Step, output: &mut String) -> Result<()> {
    match &step.action {
        Action::WriteFile {
            path,
            contents,
            mode,
        } => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).ctx(|| format!("creating {}", parent.display()))?;
            }
            std::fs::write(path, contents).ctx(|| format!("writing {}", path.display()))?;
            set_mode(path, *mode)?;
        }
        Action::RemoveFile { path } => match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(NetError::io(format!("removing {}", path.display()), e)),
        },
        Action::Run {
            program,
            args,
            ignore_failure,
        } => match run_capture(program, args) {
            Ok(o) => output.push_str(&o),
            Err(e) if *ignore_failure => output.push_str(&format!("(ignored) {e}\n")),
            Err(e) => return Err(e),
        },
        Action::Shell {
            script,
            ignore_failure,
        } => {
            let (prog, args): (&str, Vec<String>) = match os {
                Os::Windows => (
                    "powershell.exe",
                    vec!["-NoProfile".into(), "-Command".into(), script.clone()],
                ),
                _ => ("/bin/sh", vec!["-c".into(), script.clone()]),
            };
            match run_capture(prog, &args) {
                Ok(o) => output.push_str(&o),
                Err(e) if *ignore_failure => output.push_str(&format!("(ignored) {e}\n")),
                Err(e) => return Err(e),
            }
        }
    }
    Ok(())
}

/// Set unix permission bits (no-op on Windows).
pub(crate) fn set_mode(path: &Path, mode: Option<u32>) -> Result<()> {
    #[cfg(unix)]
    if let Some(m) = mode {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(m))
            .ctx(|| format!("chmod {}", path.display()))?;
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
    Ok(())
}

fn new_stamp_dir(backups_dir: &Path) -> Result<PathBuf> {
    let base = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let mut n = 0;
    loop {
        let name = if n == 0 {
            base.clone()
        } else {
            format!("{base}-{n}")
        };
        let dir = backups_dir.join(name);
        if !dir.exists() {
            std::fs::create_dir_all(&dir).ctx(|| format!("creating {}", dir.display()))?;
            return Ok(dir);
        }
        n += 1;
    }
}

fn run_elevated(
    plan: &Plan,
    steps: &[&Step],
    backups: &[(PathBuf, PathBuf)],
    dir: &Path,
    idx: usize,
) -> Result<String> {
    let ext = if plan.os == Os::Windows { "ps1" } else { "sh" };
    let script_path = dir.join(format!("elevated-{idx}.{ext}"));
    let script = render_script(plan.os, &plan.title, steps, backups);
    std::fs::write(&script_path, script).ctx(|| format!("writing {}", script_path.display()))?;
    let (prog, args) = elevation_command(plan.os, &plan.title, &script_path);
    match run_capture(&prog, &args) {
        Ok(o) => Ok(o),
        Err(NetError::Command {
            code,
            stderr,
            command,
        }) => {
            let denied = match plan.os {
                Os::Macos => stderr.contains("-128") || stderr.to_lowercase().contains("cancel"),
                Os::Linux => matches!(code, Some(126) | Some(127)),
                Os::Windows => stderr.to_lowercase().contains("cancel"),
            };
            if denied {
                Err(NetError::ElevationDenied(stderr))
            } else {
                Err(NetError::Command {
                    command,
                    code,
                    stderr,
                })
            }
        }
        Err(e) => Err(e),
    }
}

/// Apply `plan` on this machine: back up every touched file into a fresh
/// `<backups_dir>/<stamp>/` directory, then run the steps (elevated runs through one prompt each).
pub fn apply(plan: &Plan, backups_dir: &Path) -> Result<ApplyReport> {
    if plan.os != Os::current() {
        return Err(NetError::Unsupported(format!(
            "plan targets {} but this is {}",
            plan.os,
            Os::current()
        )));
    }
    let dir = new_stamp_dir(backups_dir)?;
    let mut report = ApplyReport {
        backup_dir: dir.clone(),
        ..Default::default()
    };
    std::fs::write(
        dir.join("plan.json"),
        serde_json::to_string_pretty(plan).unwrap_or_default(),
    )
    .ctx(|| "writing plan.json".to_string())?;

    // Backups: unprivileged copy when readable, else defer to the elevated script.
    let mut deferred: Vec<(PathBuf, PathBuf)> = Vec::new();
    for f in plan.files_to_backup() {
        if !f.exists() {
            continue;
        }
        let target = backup_target(&dir, &f);
        if let Some(p) = target.parent() {
            std::fs::create_dir_all(p).ctx(|| format!("creating {}", p.display()))?;
        }
        match std::fs::copy(&f, &target) {
            Ok(_) => report.backed_up.push((f, target)),
            Err(e)
                if e.kind() == std::io::ErrorKind::PermissionDenied
                    && plan.requires_elevation() =>
            {
                deferred.push((f, target));
            }
            Err(e) => return Err(NetError::io(format!("backing up {}", f.display()), e)),
        }
    }

    let mut i = 0;
    let mut elevated_runs = 0;
    while i < plan.steps.len() {
        if plan.steps[i].elevated {
            let start = i;
            while i < plan.steps.len() && plan.steps[i].elevated {
                i += 1;
            }
            let group: Vec<&Step> = plan.steps[start..i].iter().collect();
            let backups = if elevated_runs == 0 {
                std::mem::take(&mut deferred)
            } else {
                Vec::new()
            };
            let out = run_elevated(plan, &group, &backups, &dir, elevated_runs)?;
            report.backed_up.extend(backups);
            report.output.push_str(&out);
            report.elevated = true;
            report.steps_run += group.len();
            elevated_runs += 1;
        } else {
            run_step_unprivileged(plan.os, &plan.steps[i], &mut report.output)?;
            report.steps_run += 1;
            i += 1;
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> Plan {
        let mut p = Plan::new("Route *.test", Os::Linux);
        p.write_file(
            "write conf",
            "/etc/x/it's.conf",
            "a=1\nBZ_EOF_0\n",
            Some(0o644),
            true,
        );
        p.run(
            "restart",
            "systemctl",
            ["restart", "systemd-resolved"],
            true,
        );
        p.remove_file("rm", "/etc/old", true);
        p
    }

    #[test]
    fn quoting() {
        assert_eq!(sh_quote("abc/d.e"), "abc/d.e");
        assert_eq!(sh_quote("a b"), "'a b'");
        assert_eq!(sh_quote("it's"), "'it'\\''s'");
        assert_eq!(ps_quote("it's"), "'it''s'");
    }

    #[test]
    fn sh_script_renders() {
        let p = plan();
        let steps: Vec<&Step> = p.steps.iter().collect();
        let s = render_script(
            Os::Linux,
            &p.title,
            &steps,
            &[(PathBuf::from("/etc/hosts"), PathBuf::from("/b/etc/hosts"))],
        );
        assert!(s.starts_with("#!/bin/sh"));
        assert!(s.contains("set -e"));
        assert!(s.contains("mkdir -p /etc/x\n"));
        assert!(s.contains("cat > '/etc/x/it'\\''s.conf' <<'BZ_EOF_1'\na=1\nBZ_EOF_0\nBZ_EOF_1\n"));
        assert!(s.contains("chmod 644 "));
        assert!(s.contains("systemctl restart systemd-resolved\n"));
        assert!(s.contains("rm -f /etc/old\n"));
        assert!(s.contains("cp -p /etc/hosts /b/etc/hosts"));
    }

    #[test]
    fn ps_script_renders() {
        let mut p = Plan::new("NRPT", Os::Windows);
        p.write_file(
            "hosts",
            r"C:\Windows\System32\drivers\etc\hosts",
            "127.0.0.1 a.test\n",
            None,
            true,
        );
        p.shell(
            "nrpt",
            "Add-DnsClientNrptRule -Namespace '.test' -NameServers '127.0.0.1'",
            true,
        );
        p.run("cu", "certutil", ["-addstore", "Root", "x.pem"], true);
        let steps: Vec<&Step> = p.steps.iter().collect();
        let s = render_script(Os::Windows, &p.title, &steps, &[]);
        assert!(s.contains("$ErrorActionPreference = 'Stop'"));
        assert!(s.contains("@'\n127.0.0.1 a.test\n'@"));
        assert!(s.contains("WriteAllText('C:\\Windows\\System32\\drivers\\etc\\hosts'"));
        assert!(s.contains("Add-DnsClientNrptRule"));
        assert!(s.contains("& 'certutil' '-addstore' 'Root' 'x.pem'\nif ($LASTEXITCODE -ne 0)"));
        assert!(s.trim_end().ends_with("exit 0"));
    }

    #[test]
    fn elevation_commands() {
        let (p, a) = elevation_command(Os::Macos, "do \"it\"", Path::new("/tmp/a b.sh"));
        assert_eq!(p, "osascript");
        assert_eq!(a[0], "-e");
        assert!(a[1].starts_with("do shell script \"/bin/sh '/tmp/a b.sh'\""));
        assert!(a[1].contains("do \\\"it\\\""));
        assert!(a[1].ends_with("with administrator privileges"));
        let (p, a) = elevation_command(Os::Linux, "t", Path::new("/tmp/s.sh"));
        assert_eq!(
            (p.as_str(), a),
            (
                "pkexec",
                vec!["/bin/sh".to_string(), "/tmp/s.sh".to_string()]
            )
        );
        let (p, a) = elevation_command(Os::Windows, "t", Path::new(r"C:\b\s.ps1"));
        assert_eq!(p, "powershell.exe");
        assert!(a.last().unwrap().contains("-Verb RunAs -Wait -PassThru"));
        assert!(a.last().unwrap().contains(r#"'"C:\b\s.ps1"'"#));
    }

    #[test]
    fn backup_paths() {
        assert_eq!(
            backup_target(Path::new("/b/1"), Path::new("/etc/hosts")),
            PathBuf::from("/b/1/etc/hosts")
        );
        let w = backup_target(Path::new("/b/1"), Path::new(r"C:\Windows\hosts"));
        assert!(w.to_string_lossy().ends_with("Windows/hosts"), "{w:?}");
        assert_eq!(
            plan().files_to_backup(),
            vec![PathBuf::from("/etc/x/it's.conf"), PathBuf::from("/etc/old")]
        );
    }

    #[test]
    fn apply_unprivileged_plan_with_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("conf/a.txt");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "old").unwrap();
        let mut p = Plan::new("test", Os::current());
        p.write_file("write", &target, "new", Some(0o600), false);
        let r = apply(&p, &tmp.path().join("backups")).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
        assert_eq!(r.backed_up.len(), 1);
        assert_eq!(std::fs::read_to_string(&r.backed_up[0].1).unwrap(), "old");
        assert!(r.backup_dir.join("plan.json").exists());
        assert!(!r.elevated);
    }
}
