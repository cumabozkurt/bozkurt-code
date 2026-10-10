//! Keeping supervised children tied to the daemon (P1-7).
//!
//! Three layers, so that Caddy, PHP, MariaDB and app dev servers never outlive a daemon
//! that crashed, was OOM-killed or `kill -9`ed and then block ports 80/443/3306:
//!
//! * **Windows:** every child is assigned to its own Job Object created with
//!   `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. The daemon holds the only handle, so when the
//!   daemon dies for any reason the kernel closes it and kills the whole child tree.
//!   The job is also how a stop force-kills the tree (`TerminateJobObject`).
//! * **Linux:** `prctl(PR_SET_PDEATHSIG, SIGTERM)` in the child before `exec`. The signal
//!   goes to the direct child only; grandchildren in its process group are caught by the
//!   startup reap below.
//! * **All OSes:** the supervisor records `{name, pid, pgid, start}` of every running
//!   child in `$BOZKURT_HOME/run/children.json`. At daemon startup [`reap_orphans`] kills
//!   leftovers whose pid **and** start time still match, so a recycled pid is never hit.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// One running child as recorded in `run/children.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChildRecord {
    pub pid: u32,
    /// Process group (unix; equals `pid` because children get their own group).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pgid: Option<u32>,
    /// Platform start-time token ([`start_token`]); compared before killing anything.
    pub start: String,
    pub program: String,
}

/// File content: process name → record.
pub type Registry = BTreeMap<String, ChildRecord>;

pub fn load_registry(path: &Path) -> Registry {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save_registry(path: &Path, reg: &Registry) -> std::io::Result<()> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    let data = serde_json::to_vec_pretty(reg).map_err(std::io::Error::other)?;
    crate::fsutil::atomic_write(path, &data)
}

/// A token that identifies one incarnation of `pid` (its start time), or `None` when the
/// process does not exist.
///
/// Linux: `starttime` from `/proc/<pid>/stat` (clock ticks since boot). macOS:
/// `proc_pidinfo` start seconds+µs. Windows: creation `FILETIME`.
pub fn start_token(pid: u32) -> Option<String> {
    imp::start_token(pid)
}

/// Kill leftovers from a previous daemon recorded in `path`, then delete the file.
/// Returns the names of the processes that were killed. Blocking (waits up to ~2 s for
/// a polite exit before force-killing); call before anything is started.
pub fn reap_orphans(path: &Path) -> Vec<String> {
    let reg = load_registry(path);
    let mut killed = vec![];
    for (name, rec) in &reg {
        if imp::reap(rec) {
            tracing::warn!(
                "killed orphaned {name} (pid {}) left over from a previous daemon",
                rec.pid
            );
            killed.push(name.clone());
        }
    }
    let _ = std::fs::remove_file(path);
    killed
}

/// Per-child kill guard. On Windows it owns the child's Job Object (dropping it kills
/// whatever is still in the job); elsewhere it is empty.
#[derive(Debug, Default)]
pub struct ChildGuard {
    #[cfg(windows)]
    job: Option<imp::Job>,
}

impl ChildGuard {
    /// Attach a freshly spawned child (Windows: assign it to a new kill-on-close job).
    /// Failure is logged, never fatal: the daemon may itself run in a job that forbids
    /// nesting/breakaway.
    #[allow(unused_variables)]
    pub fn attach(child: &tokio::process::Child) -> ChildGuard {
        #[cfg(windows)]
        {
            let job = child.raw_handle().and_then(|h| match imp::Job::new() {
                Ok(job) => match job.assign(h) {
                    Ok(()) => Some(job),
                    Err(e) => {
                        tracing::warn!("could not assign child to a job object: {e}");
                        None
                    }
                },
                Err(e) => {
                    tracing::warn!("could not create a job object: {e}");
                    None
                }
            });
            ChildGuard { job }
        }
        #[cfg(not(windows))]
        ChildGuard::default()
    }

    /// Force-kill the whole tree (Windows: `TerminateJobObject`). Returns false when
    /// there is no job (the caller falls back to `taskkill /F /T`).
    pub fn kill_tree(&self) -> bool {
        #[cfg(windows)]
        {
            if let Some(j) = &self.job {
                return j.terminate();
            }
        }
        false
    }
}

/// `pre_exec` hook for the child (Linux): die with SIGTERM when the daemon thread that
/// spawned us dies. Children are spawned from tokio worker threads, which live as long
/// as the runtime, i.e. the daemon.
///
/// # Safety
/// Runs between fork and exec: only async-signal-safe calls (prctl, getppid).
#[cfg(target_os = "linux")]
pub unsafe fn linux_pdeathsig(parent: libc::pid_t) -> std::io::Result<()> {
    // SAFETY: plain syscalls, async-signal-safe.
    unsafe {
        if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM, 0, 0, 0) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        // the parent may have died between fork and prctl
        if libc::getppid() != parent {
            libc::_exit(1);
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
mod imp {
    use super::ChildRecord;
    use std::time::{Duration, Instant};

    /// `(pgid, starttime)` from `/proc/<pid>/stat`; `None` for a missing or zombie process.
    pub(super) fn stat(pid: u32) -> Option<(u32, String)> {
        let s = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // comm may contain spaces and ')' — split after the last ')'
        let rest = &s[s.rfind(')')? + 1..];
        let f: Vec<&str> = rest.split_whitespace().collect();
        // rest starts at field 3 (state); pgid is field 5, starttime field 22
        if matches!(f.first(), Some(&"Z") | Some(&"X")) {
            return None;
        }
        Some((f.get(2)?.parse().ok()?, f.get(19)?.to_string()))
    }

    pub(super) fn start_token(pid: u32) -> Option<String> {
        stat(pid).map(|(_, t)| t)
    }

    fn ticks(t: &str) -> u64 {
        t.parse().unwrap_or(0)
    }

    /// Live processes of group `pgid` started no earlier than `since` (ticks).
    fn group_members(pgid: u32, since: u64) -> Vec<u32> {
        let Ok(rd) = std::fs::read_dir("/proc") else {
            return vec![];
        };
        rd.flatten()
            .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
            .filter(|pid| matches!(stat(*pid), Some((g, t)) if g == pgid && ticks(&t) >= since))
            .collect()
    }

    pub(super) fn reap(rec: &ChildRecord) -> bool {
        let since = ticks(&rec.start);
        let pgid = rec.pgid.unwrap_or(rec.pid);
        let leader = stat(rec.pid);
        let targets: Vec<u32> = match &leader {
            // same incarnation: the whole group
            Some((_, t)) if *t == rec.start => group_members(pgid, since),
            // pid recycled by an unrelated process: the group number may be reused too
            Some(_) => return false,
            // leader gone (PDEATHSIG): grandchildren may still run in its group
            None => group_members(pgid, since),
        };
        if targets.is_empty() {
            return false;
        }
        let signal_all = |sig| {
            for p in &targets {
                // SAFETY: plain syscall on a pid we just verified.
                unsafe {
                    libc::kill(*p as i32, sig);
                }
            }
        };
        signal_all(libc::SIGTERM);
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(2) {
            if targets
                .iter()
                .all(|p| !matches!(stat(*p), Some((g, t)) if g == pgid && ticks(&t) >= since))
            {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        signal_all(libc::SIGKILL);
        true
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
mod imp {
    use super::ChildRecord;
    use std::time::{Duration, Instant};

    #[cfg(target_vendor = "apple")]
    pub(super) fn start_token(pid: u32) -> Option<String> {
        // SAFETY: proc_pidinfo fills at most `size` bytes of a zeroed POD struct.
        unsafe {
            let mut info: libc::proc_bsdinfo = std::mem::zeroed();
            let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
            let n = libc::proc_pidinfo(
                pid as libc::c_int,
                libc::PROC_PIDTBSDINFO,
                0,
                &mut info as *mut _ as *mut libc::c_void,
                size,
            );
            (n == size).then(|| format!("{}.{}", info.pbi_start_tvsec, info.pbi_start_tvusec))
        }
    }

    #[cfg(not(target_vendor = "apple"))]
    pub(super) fn start_token(pid: u32) -> Option<String> {
        let o = std::process::Command::new("ps")
            .args(["-o", "lstart=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
        (o.status.success() && !s.is_empty()).then_some(s)
    }

    /// Only the same incarnation of the group leader is signalled (the whole group);
    /// grandchildren of a dead leader are not tracked here.
    pub(super) fn reap(rec: &ChildRecord) -> bool {
        if start_token(rec.pid).as_deref() != Some(rec.start.as_str()) {
            return false;
        }
        let pgid = rec.pgid.unwrap_or(rec.pid) as i32;
        // SAFETY: plain syscalls on a verified process group.
        unsafe {
            libc::kill(-pgid, libc::SIGTERM);
        }
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(2) {
            if start_token(rec.pid).as_deref() != Some(rec.start.as_str()) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        unsafe {
            libc::kill(-pgid, libc::SIGKILL);
        }
        true
    }
}

#[cfg(windows)]
mod imp {
    use super::ChildRecord;
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_TERMINATE,
    };

    /// An owned Job Object handle with KILL_ON_JOB_CLOSE.
    #[derive(Debug)]
    pub(super) struct Job(HANDLE);
    // SAFETY: a job handle is a kernel object handle, usable from any thread.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        pub(super) fn new() -> std::io::Result<Job> {
            // SAFETY: FFI with valid pointers; the handle is owned by `Job`.
            unsafe {
                let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if h.is_null() {
                    return Err(std::io::Error::last_os_error());
                }
                let job = Job(h);
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                if SetInformationJobObject(
                    h,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                ) == 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(job)
            }
        }

        pub(super) fn assign(
            &self,
            process: std::os::windows::io::RawHandle,
        ) -> std::io::Result<()> {
            // SAFETY: both handles are valid for the duration of the call.
            if unsafe { AssignProcessToJobObject(self.0, process as HANDLE) } == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        }

        pub(super) fn terminate(&self) -> bool {
            // SAFETY: valid owned handle.
            unsafe { TerminateJobObject(self.0, 1) != 0 }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: we own the handle; closing the last one kills what is left in the job.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    fn open(pid: u32, access: u32) -> Option<HANDLE> {
        // SAFETY: FFI; a null handle means failure.
        let h = unsafe { OpenProcess(access, 0, pid) };
        (!h.is_null()).then_some(h)
    }

    fn creation_time(h: HANDLE) -> Option<String> {
        let z = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut c, mut e, mut k, mut u) = (z, z, z, z);
        // SAFETY: valid handle and out pointers.
        let ok = unsafe { GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u) } != 0;
        ok.then(|| ((c.dwHighDateTime as u64) << 32 | c.dwLowDateTime as u64).to_string())
    }

    pub(super) fn start_token(pid: u32) -> Option<String> {
        let h = open(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
        let t = creation_time(h);
        // SAFETY: handle from OpenProcess.
        unsafe {
            CloseHandle(h);
        }
        t
    }

    /// Normally a no-op (the job already killed everything when the old daemon died);
    /// covers children whose job assignment failed. The tree is killed with
    /// `taskkill /F /T` after the start time matched.
    pub(super) fn reap(rec: &ChildRecord) -> bool {
        let Some(h) = open(
            rec.pid,
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE,
        ) else {
            return false;
        };
        let same = creation_time(h).as_deref() == Some(rec.start.as_str());
        if same {
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new("taskkill")
                .args(["/F", "/T", "/PID", &rec.pid.to_string()])
                .creation_flags(0x0800_0000)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            // SAFETY: valid handle with PROCESS_TERMINATE.
            unsafe {
                TerminateProcess(h, 1);
            }
        }
        // SAFETY: handle from OpenProcess.
        unsafe {
            CloseHandle(h);
        }
        same
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_roundtrip_and_missing_file() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("run").join("children.json");
        assert!(load_registry(&f).is_empty());
        let mut r = Registry::new();
        r.insert(
            "caddy".into(),
            ChildRecord {
                pid: 1,
                pgid: Some(1),
                start: "x".into(),
                program: "caddy".into(),
            },
        );
        save_registry(&f, &r).unwrap();
        assert_eq!(load_registry(&f), r);
    }

    #[test]
    fn own_start_token_is_stable() {
        let me = std::process::id();
        let a = start_token(me).expect("own process has a start token");
        assert_eq!(start_token(me), Some(a));
    }

    #[cfg(unix)]
    #[test]
    fn reap_kills_recorded_orphan_but_not_a_mismatched_one() {
        use std::os::unix::process::CommandExt;
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("children.json");
        let mut a = std::process::Command::new("sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let mut b = std::process::Command::new("sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let mut r = Registry::new();
        r.insert(
            "orphan".into(),
            ChildRecord {
                pid: a.id(),
                pgid: Some(a.id()),
                start: start_token(a.id()).unwrap(),
                program: "sleep".into(),
            },
        );
        r.insert(
            "recycled".into(),
            ChildRecord {
                pid: b.id(),
                pgid: Some(b.id()),
                start: "not-its-start-time".into(),
                program: "sleep".into(),
            },
        );
        save_registry(&f, &r).unwrap();
        let killed = reap_orphans(&f);
        assert_eq!(killed, vec!["orphan".to_string()]);
        assert!(!a.wait().unwrap().success());
        assert!(
            b.try_wait().unwrap().is_none(),
            "mismatched pid must survive"
        );
        assert!(!f.exists());
        let _ = b.kill();
        let _ = b.wait();
    }
}
