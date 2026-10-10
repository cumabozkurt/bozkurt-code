//! [`NetService`]: the façade `bz-core` plugs in (through its own trait).

use std::net::{Ipv4Addr, SocketAddr};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};

use crate::dns::{self, DnsConfig, DnsHandle};
use crate::elevation::{self, ApplyReport};
use crate::error::{NetError, Result};
use crate::hosts;
use crate::plan::{Os, Plan};
use crate::ports::{self, PortOwner};
use crate::resolver::{self, DnsSetup, DnsStatus};
use crate::tls::{CaInfo, CaStatus, LeafCert, LocalCa};
use crate::trust;

/// Directories `bz-net` works in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetPaths {
    /// `$BOZKURT_HOME/ca` (rootCA.pem, rootCA-key.pem).
    pub ca_dir: PathBuf,
    /// `$BOZKURT_HOME/certs` (leaf certificates).
    pub certs_dir: PathBuf,
    /// `$BOZKURT_HOME/backups` (a `<stamp>/` sub-dir is created per applied plan).
    pub backups_dir: PathBuf,
    /// The user's home directory (login keychain, NSS dbs). Defaults to `$HOME` / `%USERPROFILE%`.
    pub user_home: PathBuf,
}

impl NetPaths {
    /// Explicit directories; `user_home` from the environment.
    pub fn new(
        ca_dir: impl Into<PathBuf>,
        certs_dir: impl Into<PathBuf>,
        backups_dir: impl Into<PathBuf>,
    ) -> Self {
        let user_home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_default();
        NetPaths {
            ca_dir: ca_dir.into(),
            certs_dir: certs_dir.into(),
            backups_dir: backups_dir.into(),
            user_home,
        }
    }

    /// Standard layout under a BOZKURT home (`ca/`, `certs/`, `backups/`).
    pub fn from_bozkurt_home(home: &Path) -> Self {
        Self::new(home.join("ca"), home.join("certs"), home.join("backups"))
    }
}

/// Networking services: certificates & trust, DNS responder & OS routing, ports, plans.
///
/// Methods that only *describe* changes return a [`Plan`]; nothing outside `$BOZKURT_HOME` is
/// touched until [`NetService::apply`] is called with a plan the user confirmed.
#[derive(Debug, Clone)]
pub struct NetService {
    paths: NetPaths,
}

impl NetService {
    /// New service over `paths`.
    pub fn new(paths: NetPaths) -> Self {
        NetService { paths }
    }

    /// Shortcut for `NetService::new(NetPaths::new(ca_dir, certs_dir, backups_dir))`.
    pub fn with_dirs(
        ca_dir: impl Into<PathBuf>,
        certs_dir: impl Into<PathBuf>,
        backups_dir: impl Into<PathBuf>,
    ) -> Self {
        Self::new(NetPaths::new(ca_dir, certs_dir, backups_dir))
    }

    /// Paths in use.
    pub fn paths(&self) -> &NetPaths {
        &self.paths
    }

    /// The local CA.
    pub fn ca(&self) -> LocalCa {
        LocalCa::new(&self.paths.ca_dir)
    }

    // ------------------------------------------------------------------ TLS

    /// Create the CA if needed and return its facts.
    pub fn ensure_ca(&self) -> Result<CaInfo> {
        self.ca().ensure()
    }

    /// Certificate + key for `domain` (+ aliases, + `*.` variants when `wildcard`), issuing or
    /// renewing only when needed. Returns `(cert.pem, key.pem)`.
    pub fn ensure_cert(
        &self,
        domain: &str,
        aliases: &[String],
        wildcard: bool,
    ) -> Result<(PathBuf, PathBuf)> {
        let l = self.ensure_leaf(domain, aliases, wildcard)?;
        Ok((l.cert_path, l.key_path))
    }

    /// Like [`Self::ensure_cert`] with details (whether it was issued, expiry).
    pub fn ensure_leaf(
        &self,
        domain: &str,
        aliases: &[String],
        wildcard: bool,
    ) -> Result<LeafCert> {
        self.ca()
            .ensure_leaf(&self.paths.certs_dir, domain, aliases, wildcard)
    }

    /// Delete the certificate files of `domain` (inside `certs_dir`).
    pub fn remove_cert(&self, domain: &str) -> Result<()> {
        let d = crate::tls::normalize_domain(domain)?;
        let d = d.trim_start_matches("*.");
        for f in [format!("{d}.pem"), format!("{d}-key.pem")] {
            let p = self.paths.certs_dir.join(f);
            if p.exists() {
                std::fs::remove_file(&p)
                    .map_err(|e| NetError::io(format!("removing {}", p.display()), e))?;
            }
        }
        Ok(())
    }

    /// CA existence, expiry and trust status.
    pub fn ca_status(&self) -> CaStatus {
        let ca = self.ca();
        let ca_path = ca.cert_path();
        match ca.info() {
            Ok(info) => {
                let pem = std::fs::read_to_string(&ca_path).unwrap_or_default();
                let env = trust::detect_env(&self.paths.user_home);
                let stores = trust::status(&env, &info, &pem);
                CaStatus {
                    ca_path,
                    exists: true,
                    ca_trusted: stores.first().is_some_and(|s| s.trusted),
                    ca_expires: Some(info.not_after),
                    common_name: Some(info.common_name),
                    stores,
                }
            }
            Err(_) => CaStatus {
                ca_path,
                exists: false,
                ca_trusted: false,
                ca_expires: None,
                common_name: None,
                stores: Vec::new(),
            },
        }
    }

    /// Plan that trusts the CA (creating it first if needed). User stores need no admin;
    /// `include_system` adds elevated system-store steps.
    pub fn trust_ca(&self, include_system: bool) -> Result<Plan> {
        let info = self.ensure_ca()?;
        let pem = std::fs::read_to_string(&info.cert_path)
            .map_err(|_| NetError::Missing(info.cert_path.clone()))?;
        let env = trust::detect_env(&self.paths.user_home);
        Ok(trust::trust_plan(
            Os::current(),
            &env,
            &info,
            &pem,
            include_system,
        ))
    }

    /// Plan that removes the CA from the trust stores.
    pub fn untrust_ca(&self, include_system: bool) -> Result<Plan> {
        let info = self.ca().info()?;
        let env = trust::detect_env(&self.paths.user_home);
        Ok(trust::untrust_plan(
            Os::current(),
            &env,
            &info,
            include_system,
        ))
    }

    // ------------------------------------------------------------------ plans

    /// Perform a confirmed plan (backups first, one elevation prompt per elevated block).
    /// Blocking: may wait on a password dialog, so call from `spawn_blocking`.
    pub fn apply(&self, plan: &Plan) -> Result<ApplyReport> {
        elevation::apply(plan, &self.paths.backups_dir)
    }

    // ------------------------------------------------------------------ DNS

    /// Is `*.tld` routed to `127.0.0.1:port` by the OS?
    pub fn dns_status(&self, tld: &str, port: u16) -> Result<DnsStatus> {
        resolver::status(tld, port)
    }

    /// Plan routing `*.tld` to `127.0.0.1:port`. Check [`DnsSetup::responder_port`]: on Windows
    /// (NRPT) the responder must listen on 53. With the hosts-file fallback, the hosts already in
    /// the managed block are kept; use [`Self::dns_configure_plan_with_hosts`] to set them.
    pub fn dns_configure_plan(&self, tld: &str, port: u16) -> Result<DnsSetup> {
        let current = self.read_hosts();
        let existing = hosts::managed_hosts(&current);
        let mut setup =
            resolver::configure_plan(&resolver::detect_env(), tld, port, &current, &existing)?;
        explain_windows_fallback(&mut setup, tld);
        Ok(setup)
    }

    /// Windows: the hosts-file plan used when the responder could not take port 53 after
    /// all (it was free when probed), with `why` as a note (P1-14).
    pub fn dns_configure_plan_hosts_fallback(
        &self,
        tld: &str,
        port: u16,
        why: &str,
    ) -> Result<DnsSetup> {
        let current = self.read_hosts();
        let existing = hosts::managed_hosts(&current);
        let env = resolver::SystemEnv::Windows {
            port53_available: false,
        };
        let mut setup = resolver::configure_plan(&env, tld, port, &current, &existing)?;
        setup.plan.notes.push(why.to_string());
        Ok(setup)
    }

    /// Like [`Self::dns_configure_plan`], listing `hosts` if the hosts-file fallback is used.
    pub fn dns_configure_plan_with_hosts(
        &self,
        tld: &str,
        port: u16,
        hosts: &[String],
    ) -> Result<DnsSetup> {
        let mut setup = resolver::configure_plan(
            &resolver::detect_env(),
            tld,
            port,
            &self.read_hosts(),
            hosts,
        )?;
        explain_windows_fallback(&mut setup, tld);
        Ok(setup)
    }

    /// Plan removing the routing for `tld` (and our hosts-file block).
    pub fn dns_unconfigure_plan(&self, tld: &str) -> Result<Plan> {
        resolver::unconfigure_plan(&resolver::detect_env(), tld, &self.read_hosts())
    }

    /// Plan adding `names` to the hosts-file managed block (empty plan if already present).
    pub fn hosts_add_plan(&self, names: &[String]) -> Plan {
        let current = self.read_hosts();
        let mut all = hosts::managed_hosts(&current);
        all.extend(names.iter().map(|n| n.to_ascii_lowercase()));
        resolver::hosts_plan(
            Os::current(),
            &hosts::hosts_path(Os::current()),
            &current,
            &all,
        )
    }

    /// Plan removing `names` from the hosts-file managed block.
    pub fn hosts_remove_plan(&self, names: &[String]) -> Plan {
        let current = self.read_hosts();
        let rm: Vec<String> = names.iter().map(|n| n.to_ascii_lowercase()).collect();
        let keep: Vec<String> = hosts::managed_hosts(&current)
            .into_iter()
            .filter(|h| !rm.contains(h))
            .collect();
        resolver::hosts_plan(
            Os::current(),
            &hosts::hosts_path(Os::current()),
            &current,
            &keep,
        )
    }

    /// Host names currently in our hosts-file block.
    pub fn managed_hosts(&self) -> Vec<String> {
        hosts::managed_hosts(&self.read_hosts())
    }

    fn read_hosts(&self) -> String {
        std::fs::read_to_string(hosts::hosts_path(Os::current())).unwrap_or_default()
    }

    /// Start the DNS responder on `127.0.0.1:port` (UDP + TCP) for `tlds`. Port 0 picks one.
    pub async fn start_dns(&self, port: u16, tlds: &[String]) -> Result<DnsHandle> {
        let cfg = DnsConfig::new(tlds)?;
        dns::start(SocketAddr::from((Ipv4Addr::LOCALHOST, port)), cfg).await
    }

    // ------------------------------------------------------------------ ports

    /// Who listens on TCP `port`.
    pub fn port_owner(&self, port: u16) -> PortOwner {
        ports::port_owner(port)
    }

    /// Who is bound to UDP `port` (e.g. 53 / the DNS port).
    pub fn udp_port_owner(&self, port: u16) -> PortOwner {
        ports::udp_port_owner(port)
    }

    /// TCP `port` free on 127.0.0.1 and 0.0.0.0.
    pub fn is_port_free(&self, port: u16) -> bool {
        ports::is_free(port)
    }

    /// First free TCP port in `range`.
    pub fn find_free_port(&self, range: RangeInclusive<u16>) -> Option<u16> {
        ports::find_free_port(range)
    }
}

/// On Windows, say why the hosts file is used instead of NRPT (port 53 taken, P1-14).
fn explain_windows_fallback(setup: &mut DnsSetup, tld: &str) {
    if Os::current() == Os::Windows && setup.backend == resolver::ResolverBackend::HostsFile {
        if let Some(holder) = resolver::port53_holder() {
            setup
                .plan
                .notes
                .push(resolver::port53_fallback_note(tld, &holder));
        }
    }
}
