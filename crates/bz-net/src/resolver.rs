//! OS-level routing of a TLD to the local DNS responder.
//!
//! | OS | backend | wildcard |
//! |---|---|---|
//! | macOS | `/etc/resolver/<tld>` (`nameserver 127.0.0.1` + `port <port>`) | yes |
//! | Linux | systemd-resolved drop-in `/etc/systemd/resolved.conf.d/bozkurt.conf` (`DNS=127.0.0.1:<port>`, `Domains=~<tld>`) when resolved is active | yes |
//! | Linux | NetworkManager dnsmasq (`/etc/NetworkManager/dnsmasq.d/bozkurt.conf`, `server=/<tld>/127.0.0.1#<port>`) when NM runs dnsmasq | yes |
//! | Windows | NRPT rule `.<tld>` → `127.0.0.1` (port 53 only, so the responder must bind 127.0.0.1:53) | yes |
//! | any | managed block in the hosts file | no (explicit hosts) |
//!
//! Plan generation is pure ([`configure_plan`], [`unconfigure_plan`], [`hosts_plan`]) and takes a
//! [`SystemEnv`] snapshot, so every OS path is unit-tested on any OS. [`detect_env`] and
//! [`status`] inspect the real machine.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::error::{NetError, Result};
use crate::hosts;
use crate::plan::{Action, Os, Plan};

/// Comment / marker used for everything we create.
pub const MARKER: &str = "BOZKURT CODE";
/// systemd-resolved drop-in path.
pub const RESOLVED_DROPIN: &str = "/etc/systemd/resolved.conf.d/bozkurt.conf";
/// NetworkManager dnsmasq snippet path.
pub const NM_DNSMASQ_CONF: &str = "/etc/NetworkManager/dnsmasq.d/bozkurt.conf";

/// How a TLD is routed to the responder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolverBackend {
    /// macOS `/etc/resolver/<tld>`.
    MacosResolver,
    /// Linux systemd-resolved drop-in.
    SystemdResolved,
    /// Linux NetworkManager with dnsmasq plugin.
    NetworkManagerDnsmasq,
    /// Windows Name Resolution Policy Table rule.
    WindowsNrpt,
    /// Managed block in the hosts file (no wildcards).
    HostsFile,
}

impl ResolverBackend {
    /// Whether arbitrary sub-domains resolve without listing them.
    pub fn supports_wildcards(self) -> bool {
        !matches!(self, ResolverBackend::HostsFile)
    }
}

/// Facts about the machine that decide the backend (see [`detect_env`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "os", rename_all = "snake_case")]
pub enum SystemEnv {
    /// macOS (always `/etc/resolver`).
    Macos,
    /// Linux.
    Linux {
        /// `systemd-resolved` is active and owns `/etc/resolv.conf`.
        resolved_active: bool,
        /// NetworkManager is running with `dns=dnsmasq`.
        networkmanager_dnsmasq: bool,
    },
    /// Windows.
    Windows {
        /// 127.0.0.1:53 (UDP) is free or already held by our responder, so NRPT can be used.
        port53_available: bool,
    },
}

impl SystemEnv {
    /// OS of this snapshot.
    pub fn os(&self) -> Os {
        match self {
            SystemEnv::Macos => Os::Macos,
            SystemEnv::Linux { .. } => Os::Linux,
            SystemEnv::Windows { .. } => Os::Windows,
        }
    }

    /// Backend [`configure_plan`] will choose.
    pub fn backend(&self) -> ResolverBackend {
        match *self {
            SystemEnv::Macos => ResolverBackend::MacosResolver,
            SystemEnv::Linux {
                resolved_active: true,
                ..
            } => ResolverBackend::SystemdResolved,
            SystemEnv::Linux {
                networkmanager_dnsmasq: true,
                ..
            } => ResolverBackend::NetworkManagerDnsmasq,
            SystemEnv::Linux { .. } => ResolverBackend::HostsFile,
            SystemEnv::Windows {
                port53_available: true,
            } => ResolverBackend::WindowsNrpt,
            SystemEnv::Windows { .. } => ResolverBackend::HostsFile,
        }
    }
}

/// Result of [`configure_plan`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsSetup {
    /// Chosen backend.
    pub backend: ResolverBackend,
    /// Port the DNS responder must listen on for this routing to work (53 for Windows NRPT).
    pub responder_port: u16,
    /// Whether `*.tld` works without listing hosts.
    pub wildcard: bool,
    /// The changes to confirm and apply.
    pub plan: Plan,
}

/// Current routing status for a TLD.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsStatus {
    /// TLD checked.
    pub tld: String,
    /// Responder port expected.
    pub port: u16,
    /// Backend found configured (or the one that would be used).
    pub backend: ResolverBackend,
    /// True when routing for `tld` to `127.0.0.1:<port>` is in place.
    pub configured: bool,
    /// Human-readable explanation.
    pub detail: String,
}

/// Validate and normalise a TLD (`".Test."` → `"test"`).
pub fn normalize_tld(tld: &str) -> Result<String> {
    let t = tld.trim().trim_matches('.').to_ascii_lowercase();
    let ok = !t.is_empty()
        && t.len() <= 253
        && t.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        });
    if ok {
        Ok(t)
    } else {
        Err(NetError::Invalid(format!("invalid TLD {tld:?}")))
    }
}

/// `/etc/resolver/<tld>` path (macOS).
pub fn macos_resolver_path(tld: &str) -> PathBuf {
    PathBuf::from(format!("/etc/resolver/{tld}"))
}

/// Contents of the macOS resolver file.
pub fn macos_resolver_contents(port: u16) -> String {
    format!("# Generated by {MARKER}\nnameserver 127.0.0.1\nport {port}\n")
}

/// Contents of the systemd-resolved drop-in.
pub fn resolved_dropin_contents(tld: &str, port: u16) -> String {
    format!("# Generated by {MARKER}\n[Resolve]\nDNS=127.0.0.1:{port}\nDomains=~{tld}\n")
}

/// Contents of the NetworkManager dnsmasq snippet.
pub fn nm_dnsmasq_contents(tld: &str, port: u16) -> String {
    format!("# Generated by {MARKER}\nserver=/{tld}/127.0.0.1#{port}\n")
}

/// PowerShell that (re)creates the NRPT rule for `.tld`.
pub fn nrpt_add_script(tld: &str) -> String {
    format!(
        "Get-DnsClientNrptRule | Where-Object {{ $_.Comment -eq '{MARKER}' -and ($_.Namespace -contains '.{tld}') }} | ForEach-Object {{ Remove-DnsClientNrptRule -Name $_.Name -Force }}\n\
         Add-DnsClientNrptRule -Namespace '.{tld}' -NameServers '127.0.0.1' -Comment '{MARKER}'\n\
         Clear-DnsClientCache"
    )
}

/// PowerShell that removes our NRPT rule for `.tld`.
pub fn nrpt_remove_script(tld: &str) -> String {
    format!(
        "Get-DnsClientNrptRule | Where-Object {{ $_.Comment -eq '{MARKER}' -and ($_.Namespace -contains '.{tld}') }} | ForEach-Object {{ Remove-DnsClientNrptRule -Name $_.Name -Force }}\n\
         Clear-DnsClientCache"
    )
}

/// Plan that sets the hosts file's managed block to exactly `hosts` (pure).
pub fn hosts_plan(os: Os, hosts_file: &Path, current_hosts: &str, hosts: &[String]) -> Plan {
    let mut plan = Plan::new("Update BOZKURT CODE entries in the hosts file", os);
    let new = hosts::render_hosts(current_hosts, hosts);
    if new != current_hosts {
        plan.write_file(
            format!(
                "Write {} host entr{} to the managed block of {}",
                hosts.len(),
                if hosts.len() == 1 { "y" } else { "ies" },
                hosts_file.display()
            ),
            hosts_file,
            new,
            None,
            true,
        );
        if os == Os::Macos {
            plan.push(
                "Flush the macOS DNS cache",
                Action::Run {
                    program: "killall".into(),
                    args: vec!["-HUP".into(), "mDNSResponder".into()],
                    ignore_failure: true,
                },
                true,
            );
        }
    }
    plan.notes.push(
        "The hosts file has no wildcards: every site domain and alias must be listed.".into(),
    );
    plan
}

/// Build the routing plan for `tld` → `127.0.0.1:port` (pure).
///
/// `current_hosts` / `hosts` are only used by the hosts-file fallback (the names to list).
pub fn configure_plan(
    env: &SystemEnv,
    tld: &str,
    port: u16,
    current_hosts: &str,
    hosts: &[String],
) -> Result<DnsSetup> {
    let tld = normalize_tld(tld)?;
    let os = env.os();
    let backend = env.backend();
    let title = format!("Route *.{tld} to the BOZKURT CODE DNS server");
    let mut responder_port = port;
    let plan = match backend {
        ResolverBackend::MacosResolver => {
            let mut p = Plan::new(title, os);
            p.write_file(
                format!("Create /etc/resolver/{tld} pointing .{tld} to 127.0.0.1 port {port}"),
                macos_resolver_path(&tld),
                macos_resolver_contents(port),
                Some(0o644),
                true,
            );
            p.push(
                "Flush the macOS DNS cache",
                Action::Run {
                    program: "killall".into(),
                    args: vec!["-HUP".into(), "mDNSResponder".into()],
                    ignore_failure: true,
                },
                true,
            );
            p
        }
        ResolverBackend::SystemdResolved => {
            let mut p = Plan::new(title, os);
            p.write_file(
                format!("Create systemd-resolved drop-in routing ~{tld} to 127.0.0.1:{port}"),
                RESOLVED_DROPIN,
                resolved_dropin_contents(&tld, port),
                Some(0o644),
                true,
            );
            p.run(
                "Restart systemd-resolved",
                "systemctl",
                ["restart", "systemd-resolved"],
                true,
            );
            p.notes
                .push("Requires systemd 246+ for the DNS=ip:port syntax.".into());
            p
        }
        ResolverBackend::NetworkManagerDnsmasq => {
            let mut p = Plan::new(title, os);
            p.write_file(
                format!("Create NetworkManager dnsmasq rule server=/{tld}/127.0.0.1#{port}"),
                NM_DNSMASQ_CONF,
                nm_dnsmasq_contents(&tld, port),
                Some(0o644),
                true,
            );
            p.run(
                "Restart NetworkManager",
                "systemctl",
                ["restart", "NetworkManager"],
                true,
            );
            p
        }
        ResolverBackend::WindowsNrpt => {
            responder_port = 53;
            let mut p = Plan::new(title, os);
            p.shell(
                format!("Add NRPT rule: names under .{tld} are resolved by 127.0.0.1"),
                nrpt_add_script(&tld),
                true,
            );
            if port != 53 {
                p.notes.push(format!(
                    "NRPT cannot set a port: the DNS responder must listen on 127.0.0.1:53 instead of {port}."
                ));
            }
            p
        }
        ResolverBackend::HostsFile => {
            let mut p = hosts_plan(os, &hosts::hosts_path(os), current_hosts, hosts);
            p.title =
                format!("List .{tld} sites in the hosts file (wildcard DNS routing unavailable)");
            p
        }
    };
    Ok(DnsSetup {
        backend,
        responder_port,
        wildcard: backend.supports_wildcards(),
        plan,
    })
}

/// Plan that removes the routing created by [`configure_plan`] (pure).
pub fn unconfigure_plan(env: &SystemEnv, tld: &str, current_hosts: &str) -> Result<Plan> {
    let tld = normalize_tld(tld)?;
    let os = env.os();
    let mut p = Plan::new(format!("Remove BOZKURT CODE DNS routing for .{tld}"), os);
    match env.backend() {
        ResolverBackend::MacosResolver => {
            p.remove_file(
                format!("Delete /etc/resolver/{tld}"),
                macos_resolver_path(&tld),
                true,
            );
        }
        ResolverBackend::SystemdResolved => {
            p.remove_file("Delete the systemd-resolved drop-in", RESOLVED_DROPIN, true);
            p.run(
                "Restart systemd-resolved",
                "systemctl",
                ["restart", "systemd-resolved"],
                true,
            );
        }
        ResolverBackend::NetworkManagerDnsmasq => {
            p.remove_file(
                "Delete the NetworkManager dnsmasq rule",
                NM_DNSMASQ_CONF,
                true,
            );
            p.run(
                "Restart NetworkManager",
                "systemctl",
                ["restart", "NetworkManager"],
                true,
            );
        }
        ResolverBackend::WindowsNrpt => {
            p.shell(
                format!("Remove the NRPT rule for .{tld}"),
                nrpt_remove_script(&tld),
                true,
            );
        }
        ResolverBackend::HostsFile => {}
    }
    // Always clear our hosts block too (no-op when absent).
    let hp = hosts_plan(os, &hosts::hosts_path(os), current_hosts, &[]);
    p.steps.extend(hp.steps);
    Ok(p)
}

/// Parse a macOS resolver file → (nameserver, port). Port defaults to 53.
pub fn parse_macos_resolver(contents: &str) -> (Option<String>, u16) {
    let mut ns = None;
    let mut port = 53;
    for line in contents.lines() {
        let mut it = line.split_whitespace();
        match (it.next(), it.next()) {
            (Some("nameserver"), Some(v)) if ns.is_none() => ns = Some(v.to_string()),
            (Some("port"), Some(v)) => port = v.parse().unwrap_or(53),
            _ => {}
        }
    }
    (ns, port)
}

/// Does a systemd-resolved drop-in route `tld` to `127.0.0.1:port`?
pub fn resolved_dropin_matches(contents: &str, tld: &str, port: u16) -> bool {
    let mut dns = false;
    let mut dom = false;
    for line in contents.lines() {
        let l = line.trim();
        if let Some(v) = l.strip_prefix("DNS=") {
            dns |= v
                .split_whitespace()
                .any(|s| s == format!("127.0.0.1:{port}"));
        }
        if let Some(v) = l.strip_prefix("Domains=") {
            dom |= v.split_whitespace().any(|s| s == format!("~{tld}"));
        }
    }
    dns && dom
}

/// Does a dnsmasq snippet route `tld` to `127.0.0.1#port`?
pub fn nm_dnsmasq_matches(contents: &str, tld: &str, port: u16) -> bool {
    let want = format!("server=/{tld}/127.0.0.1#{port}");
    contents.lines().any(|l| l.trim() == want)
}

/// Who holds port 53 (UDP first, then TCP) when it is not us, as `"name (pid N)"`, for
/// the message shown when Windows falls back to the hosts file (P1-14). Common holders:
/// Internet Connection Sharing (`svchost` / SharedAccess), Hyper-V / WSL (`hns`), Docker
/// Desktop, a local DNS server.
pub fn port53_holder() -> Option<String> {
    let describe = |o: crate::ports::PortOwner| -> Option<String> {
        if !o.in_use || o.ours {
            return None;
        }
        Some(match (o.process, o.pid) {
            (Some(n), Some(p)) => format!("{n} (pid {p})"),
            (None, Some(p)) => format!("pid {p}"),
            (Some(n), None) => n,
            (None, None) => "another process".into(),
        })
    };
    describe(crate::ports::udp_port_owner(53)).or_else(|| describe(crate::ports::port_owner(53)))
}

/// Note added to a Windows hosts-file plan that replaced NRPT because port 53 is taken.
pub fn port53_fallback_note(tld: &str, holder: &str) -> String {
    format!(
        "127.0.0.1:53 is in use by {holder}, so the Windows NRPT rule for .{tld} cannot be used \
         (often Internet Connection Sharing, Hyper-V/WSL or Docker Desktop). Sites are listed in \
         the hosts file instead; wildcard subdomains do not resolve. Free port 53 and run DNS \
         setup again to switch to NRPT."
    )
}

/// Inspect the running machine (Linux: systemd-resolved / NetworkManager; Windows: port 53).
pub fn detect_env() -> SystemEnv {
    match Os::current() {
        Os::Macos => SystemEnv::Macos,
        // the responder binds UDP *and* TCP 53, so both must be usable (P1-14)
        Os::Windows => SystemEnv::Windows {
            port53_available: (crate::ports::is_udp_free(53)
                || crate::ports::udp_port_owner(53).ours)
                && (crate::ports::is_free(53) || crate::ports::port_owner(53).ours),
        },
        Os::Linux => {
            let resolved_running = match crate::elevation::try_output(
                "systemctl",
                &["is-active", "--quiet", "systemd-resolved"],
            ) {
                Some((ok, _)) => ok,
                None => Path::new("/run/systemd/resolve/stub-resolv.conf").exists(),
            };
            let resolved_active = resolved_running && resolv_conf_uses_resolved();
            let networkmanager_dnsmasq = nm_uses_dnsmasq();
            SystemEnv::Linux {
                resolved_active,
                networkmanager_dnsmasq,
            }
        }
    }
}

fn resolv_conf_uses_resolved() -> bool {
    // resolv.conf is a symlink into /run/systemd/resolve, or points at the 127.0.0.53 stub.
    if let Ok(target) = std::fs::read_link("/etc/resolv.conf") {
        if target.to_string_lossy().contains("systemd/resolve") {
            return true;
        }
    }
    std::fs::read_to_string("/etc/resolv.conf")
        .map(|s| s.lines().any(|l| l.trim() == "nameserver 127.0.0.53"))
        .unwrap_or(false)
}

fn nm_uses_dnsmasq() -> bool {
    let running =
        crate::elevation::try_output("systemctl", &["is-active", "--quiet", "NetworkManager"])
            .map(|(ok, _)| ok)
            .unwrap_or(false);
    if !running {
        return false;
    }
    let mut files = vec![PathBuf::from("/etc/NetworkManager/NetworkManager.conf")];
    if let Ok(rd) = std::fs::read_dir("/etc/NetworkManager/conf.d") {
        files.extend(rd.flatten().map(|e| e.path()));
    }
    files.iter().any(|f| {
        std::fs::read_to_string(f)
            .map(|s| s.lines().any(|l| l.replace(' ', "") == "dns=dnsmasq"))
            .unwrap_or(false)
    })
}

/// Current routing status of `tld` → `127.0.0.1:port` on this machine.
pub fn status(tld: &str, port: u16) -> Result<DnsStatus> {
    let tld = normalize_tld(tld)?;
    let env = detect_env();
    let read = |p: &Path| std::fs::read_to_string(p).ok();
    let hosts_text = read(&hosts::hosts_path(env.os())).unwrap_or_default();
    let managed = hosts::managed_hosts(&hosts_text);
    let suffix = format!(".{tld}");
    let hosts_count = managed
        .iter()
        .filter(|h| h.ends_with(&suffix) || **h == tld)
        .count();
    let hosts_status = |backend| DnsStatus {
        tld: tld.clone(),
        port,
        backend,
        configured: hosts_count > 0,
        detail: format!("{hosts_count} .{tld} host(s) listed in the hosts file managed block"),
    };
    let st = match env.os() {
        Os::Macos => {
            let path = macos_resolver_path(&tld);
            match read(&path) {
                Some(c) => {
                    let (ns, p) = parse_macos_resolver(&c);
                    let ok = ns.as_deref() == Some("127.0.0.1") && p == port;
                    DnsStatus {
                        tld: tld.clone(),
                        port,
                        backend: ResolverBackend::MacosResolver,
                        configured: ok,
                        detail: format!("{} → nameserver {:?} port {p}", path.display(), ns),
                    }
                }
                None if hosts_count > 0 => hosts_status(ResolverBackend::HostsFile),
                None => DnsStatus {
                    tld: tld.clone(),
                    port,
                    backend: ResolverBackend::MacosResolver,
                    configured: false,
                    detail: format!("{} does not exist", path.display()),
                },
            }
        }
        Os::Linux => {
            if let Some(c) = read(Path::new(RESOLVED_DROPIN)) {
                DnsStatus {
                    tld: tld.clone(),
                    port,
                    backend: ResolverBackend::SystemdResolved,
                    configured: resolved_dropin_matches(&c, &tld, port),
                    detail: format!("{RESOLVED_DROPIN} present"),
                }
            } else if let Some(c) = read(Path::new(NM_DNSMASQ_CONF)) {
                DnsStatus {
                    tld: tld.clone(),
                    port,
                    backend: ResolverBackend::NetworkManagerDnsmasq,
                    configured: nm_dnsmasq_matches(&c, &tld, port),
                    detail: format!("{NM_DNSMASQ_CONF} present"),
                }
            } else if hosts_count > 0 {
                hosts_status(ResolverBackend::HostsFile)
            } else {
                DnsStatus {
                    tld: tld.clone(),
                    port,
                    backend: env.backend(),
                    configured: false,
                    detail: "no BOZKURT CODE DNS routing found".into(),
                }
            }
        }
        Os::Windows => {
            let script = format!(
                "Get-DnsClientNrptRule | Where-Object {{ $_.Namespace -contains '.{tld}' }} | ForEach-Object {{ $_.NameServers }}"
            );
            let out = crate::elevation::try_output(
                "powershell.exe",
                &["-NoProfile", "-Command", &script],
            );
            let nrpt = out
                .map(|(ok, s)| ok && s.lines().any(|l| l.trim() == "127.0.0.1"))
                .unwrap_or(false);
            if nrpt {
                DnsStatus {
                    tld: tld.clone(),
                    port,
                    backend: ResolverBackend::WindowsNrpt,
                    configured: true,
                    detail: format!(
                        "NRPT rule .{tld} → 127.0.0.1 (responder must listen on port 53)"
                    ),
                }
            } else if hosts_count > 0 {
                hosts_status(ResolverBackend::HostsFile)
            } else {
                DnsStatus {
                    tld: tld.clone(),
                    port,
                    backend: env.backend(),
                    configured: false,
                    detail: format!("no NRPT rule for .{tld}"),
                }
            }
        }
    };
    Ok(st)
}

/// Resolve `host` through the OS resolver and report whether it maps to loopback
/// (the end-to-end check `bz doctor` wants). Blocking.
pub fn resolves_to_loopback(host: &str) -> bool {
    use std::net::ToSocketAddrs;
    (host, 80)
        .to_socket_addrs()
        .map(|mut it| it.any(|a| a.ip().is_loopback()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINUX_RESOLVED: SystemEnv = SystemEnv::Linux {
        resolved_active: true,
        networkmanager_dnsmasq: true,
    };
    const LINUX_NM: SystemEnv = SystemEnv::Linux {
        resolved_active: false,
        networkmanager_dnsmasq: true,
    };
    const LINUX_BARE: SystemEnv = SystemEnv::Linux {
        resolved_active: false,
        networkmanager_dnsmasq: false,
    };

    fn first_write(p: &Plan) -> (PathBuf, String) {
        p.steps
            .iter()
            .find_map(|s| match &s.action {
                Action::WriteFile { path, contents, .. } => Some((path.clone(), contents.clone())),
                _ => None,
            })
            .expect("write step")
    }

    #[test]
    fn tld_validation() {
        assert_eq!(normalize_tld(".Test.").unwrap(), "test");
        assert_eq!(normalize_tld("local.dev").unwrap(), "local.dev");
        assert!(normalize_tld("").is_err());
        assert!(normalize_tld("te st").is_err());
        assert!(normalize_tld("a/b").is_err());
    }

    #[test]
    fn macos_plan() {
        let s = configure_plan(&SystemEnv::Macos, "test", 5390, "", &[]).unwrap();
        assert_eq!(s.backend, ResolverBackend::MacosResolver);
        assert!(s.wildcard);
        assert_eq!(s.responder_port, 5390);
        let (path, c) = first_write(&s.plan);
        assert_eq!(path, PathBuf::from("/etc/resolver/test"));
        assert!(c.contains("nameserver 127.0.0.1\nport 5390\n"));
        assert!(s.plan.requires_elevation());
        assert_eq!(parse_macos_resolver(&c), (Some("127.0.0.1".into()), 5390));
        let u = unconfigure_plan(&SystemEnv::Macos, "test", "").unwrap();
        assert!(
            matches!(&u.steps[0].action, Action::RemoveFile { path } if path == Path::new("/etc/resolver/test"))
        );
    }

    #[test]
    fn linux_resolved_plan() {
        let s = configure_plan(&LINUX_RESOLVED, "test", 5390, "", &[]).unwrap();
        assert_eq!(s.backend, ResolverBackend::SystemdResolved);
        let (path, c) = first_write(&s.plan);
        assert_eq!(path, PathBuf::from(RESOLVED_DROPIN));
        assert!(c.contains("[Resolve]\nDNS=127.0.0.1:5390\nDomains=~test\n"));
        assert!(resolved_dropin_matches(&c, "test", 5390));
        assert!(!resolved_dropin_matches(&c, "test", 53));
        assert!(s.plan.steps.iter().any(|st| matches!(&st.action, Action::Run { program, args, .. } if program == "systemctl" && args[1] == "systemd-resolved")));
    }

    #[test]
    fn linux_nm_plan() {
        let s = configure_plan(&LINUX_NM, "test", 5390, "", &[]).unwrap();
        assert_eq!(s.backend, ResolverBackend::NetworkManagerDnsmasq);
        let (path, c) = first_write(&s.plan);
        assert_eq!(path, PathBuf::from(NM_DNSMASQ_CONF));
        assert!(nm_dnsmasq_matches(&c, "test", 5390));
    }

    #[test]
    fn linux_hosts_fallback() {
        let s = configure_plan(
            &LINUX_BARE,
            "test",
            5390,
            "127.0.0.1 localhost\n",
            &["shop.test".into()],
        )
        .unwrap();
        assert_eq!(s.backend, ResolverBackend::HostsFile);
        assert!(!s.wildcard);
        let (path, c) = first_write(&s.plan);
        assert_eq!(path, PathBuf::from("/etc/hosts"));
        assert!(c.starts_with("127.0.0.1 localhost\n"));
        assert!(c.contains("127.0.0.1 shop.test"));
        // Unconfigure clears the block.
        let u = unconfigure_plan(&LINUX_BARE, "test", &c).unwrap();
        let (_, c2) = first_write(&u);
        assert_eq!(c2, "127.0.0.1 localhost\n");
        // Nothing to do when already in sync.
        let again = hosts_plan(
            Os::Linux,
            Path::new("/etc/hosts"),
            &c,
            &["shop.test".into()],
        );
        assert!(again.is_empty());
    }

    #[test]
    fn port53_fallback_note_names_the_holder() {
        let n = port53_fallback_note("test", "svchost.exe (pid 1234)");
        assert!(n.contains("127.0.0.1:53 is in use by svchost.exe (pid 1234)"));
        assert!(n.contains(".test"));
        assert!(n.contains("hosts file"));
        // a busy port 53 on Windows picks the hosts file, not NRPT
        let env = SystemEnv::Windows {
            port53_available: false,
        };
        let s = configure_plan(&env, "test", 5390, "", &["shop.test".into()]).unwrap();
        assert_eq!(s.backend, ResolverBackend::HostsFile);
        assert_eq!(s.responder_port, 5390);
    }

    #[test]
    fn windows_nrpt_plan() {
        let env = SystemEnv::Windows {
            port53_available: true,
        };
        let s = configure_plan(&env, "test", 5390, "", &[]).unwrap();
        assert_eq!(s.backend, ResolverBackend::WindowsNrpt);
        assert_eq!(s.responder_port, 53);
        match &s.plan.steps[0].action {
            Action::Shell { script, .. } => {
                assert!(script
                    .contains("Add-DnsClientNrptRule -Namespace '.test' -NameServers '127.0.0.1'"));
                assert!(script.contains("Remove-DnsClientNrptRule"));
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(s.plan.notes.iter().any(|n| n.contains("127.0.0.1:53")));
        let u = unconfigure_plan(&env, "test", "").unwrap();
        assert!(
            matches!(&u.steps[0].action, Action::Shell { script, .. } if script.contains("Remove-DnsClientNrptRule"))
        );
    }

    #[test]
    fn windows_hosts_fallback_when_53_busy() {
        let env = SystemEnv::Windows {
            port53_available: false,
        };
        let s = configure_plan(&env, "test", 5390, "", &["a.test".into()]).unwrap();
        assert_eq!(s.backend, ResolverBackend::HostsFile);
        assert_eq!(s.responder_port, 5390);
        let (path, _) = first_write(&s.plan);
        assert!(path
            .to_string_lossy()
            .ends_with(r"System32\drivers\etc\hosts"));
    }
}
