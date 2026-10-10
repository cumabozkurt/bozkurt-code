//! `bz` command implementations.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use bz_core::models::{
    CheckStatus, CreateSiteRequest, Job, JobStatus, RuntimeKind, Service, ServiceStatus, Site,
    SiteKind, SitePatch,
};
use clap::{CommandFactory, Parser};
use serde_json::{json, Map, Value};

use crate::cli::*;
use crate::client::{enc, Client};
use crate::daemon;
use crate::error::CliError;
use crate::events::{self, follow_job, follow_job_with, EventStream};
use crate::i18n::t;
use crate::register::{self, Dirs, Entry};
use crate::resolve::{self, not_found, pick_version, resolve_site, service_name};
use crate::ui::{human_bytes, human_duration, Ui};

type R = Result<i32, CliError>;

/// Entry point: parse `args`, run, print errors, return the exit code.
pub fn run<I, T>(args: I) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = match Cli::try_parse_from(args) {
        Ok(c) => c,
        Err(e) => {
            let _ = e.print();
            return e.exit_code();
        }
    };
    let ui = Ui::new(
        cli.global.json,
        cli.global.quiet,
        cli.global.yes,
        cli.global.no_color,
    );
    let home = daemon::home(cli.global.home.as_deref());
    let mut client = daemon::client_for(&home);
    client.set_verbose(cli.global.verbose);
    let ctx = Ctx {
        client,
        home,
        ui,
        g: cli.global.clone(),
    };
    events::install_ctrlc();
    match ctx.dispatch(cli.cmd) {
        Ok(code) => code,
        Err(e) => {
            if ctx.ui.json {
                eprintln!("{}", e.to_json());
            } else if !matches!(e, CliError::Interrupted) {
                eprintln!("{} {e}", ctx.ui.red("error:"));
            }
            e.exit_code()
        }
    }
}

pub struct Ctx {
    pub client: Client,
    pub home: PathBuf,
    pub ui: Ui,
    pub g: Global,
}

impl Ctx {
    fn ensure(&self) -> Result<(), CliError> {
        daemon::ensure_running(
            &self.client,
            &self.home,
            !self.g.no_start,
            self.ui.quiet || self.ui.json,
        )
    }

    fn bar_visible(&self) -> bool {
        self.ui.interactive && !self.ui.quiet
    }

    /// Follow a job; print the final job with `--json`.
    fn job(&self, job_id: &str) -> Result<Job, CliError> {
        let job = follow_job(&self.client, job_id, self.bar_visible())?;
        self.ui.json_out(&job);
        Ok(job)
    }

    fn dispatch(&self, cmd: Cmd) -> R {
        // Local-only commands first.
        match cmd {
            Cmd::Completions { shell } => {
                clap_complete::generate(shell, &mut Cli::command(), "bz", &mut std::io::stdout());
                return Ok(0);
            }
            Cmd::Mcp { cmd } => return self.mcp(cmd),
            Cmd::Daemon { foreground, cmd } => return self.daemon(foreground, cmd),
            _ => {}
        }
        self.ensure()?;
        match cmd {
            Cmd::Start { service } => self.service_action(service, "start"),
            Cmd::Stop { service } => self.service_action(service, "stop"),
            Cmd::Restart { service } => self.service_action(service, "restart"),
            Cmd::Status => self.status(),
            Cmd::Services => self.services(),
            Cmd::Php(a) => self.runtime(RuntimeKind::Php, a.cmd),
            Cmd::Node(a) => self.runtime(RuntimeKind::Node, a.cmd),
            Cmd::Python(a) => self.runtime(RuntimeKind::Python, a.cmd),
            Cmd::Bun(a) => self.runtime(RuntimeKind::Bun, a.cmd),
            Cmd::Go(a) => self.runtime(RuntimeKind::Go, a.cmd),
            Cmd::Runtime { cmd } => {
                let (kind, rc) = match cmd {
                    RuntimeAlias::Install {
                        kind,
                        version,
                        allow_unverified,
                    } => {
                        let k = parse_kind(&kind)?;
                        let v = version.unwrap_or_else(|| {
                            (if k == RuntimeKind::Node {
                                "lts"
                            } else {
                                "latest"
                            })
                            .to_string()
                        });
                        (
                            k,
                            RtCmd::Install {
                                version: v,
                                allow_unverified,
                            },
                        )
                    }
                    RuntimeAlias::Use { kind, version } => {
                        (parse_kind(&kind)?, RtCmd::Use { version })
                    }
                    RuntimeAlias::Uninstall { kind, version } => {
                        (parse_kind(&kind)?, RtCmd::Uninstall { version })
                    }
                    RuntimeAlias::List {
                        kind,
                        available,
                        refresh,
                    } => (parse_kind(&kind)?, RtCmd::List { available, refresh }),
                };
                self.runtime(kind, rc)
            }
            Cmd::Link(a) => self.link(a),
            Cmd::Unlink { site } => self.unlink(site.as_deref()),
            Cmd::Sites => self.sites(),
            Cmd::Open { site } => self.open(site.as_deref()),
            Cmd::Secure { site } => self.site_action(site.as_deref(), "secure"),
            Cmd::Unsecure { site } => self.site_action(site.as_deref(), "unsecure"),
            Cmd::Share { site } => self.share(site.as_deref()),
            Cmd::Site { cmd } => match cmd {
                SiteAlias::Start { site } => self.site_action(site.as_deref(), "start"),
                SiteAlias::Stop { site } => self.site_action(site.as_deref(), "stop"),
                SiteAlias::Restart { site } => self.site_action(site.as_deref(), "restart"),
                SiteAlias::Rm { site } => self.unlink(site.as_deref()),
                SiteAlias::Set { site, key, value } => self.site_set(&site, &key, &value),
                SiteAlias::List => self.sites(),
                SiteAlias::Info { site } => self.site_info(site.as_deref()),
            },
            Cmd::Logs(a) => self.logs(a.source.as_deref(), a.lines, a.follow),
            Cmd::Doctor { fix } => self.doctor(fix.as_deref()),
            Cmd::Dns { cmd } => match cmd {
                DnsCmd::Setup { tld, undo: false } => self.dns_setup(tld),
                DnsCmd::Setup { undo: true, .. } | DnsCmd::Unconfigure => self.dns_undo(),
                DnsCmd::Status => self.dns_status(),
            },
            Cmd::Trust { undo, cmd } => match (undo, cmd) {
                (_, Some(TrustSub::Status)) => self.tls_status(),
                (true, None) => self.untrust(),
                (false, None) => self.trust(),
            },
            Cmd::Tls { cmd } => match cmd {
                TlsCmd::Trust => self.trust(),
                TlsCmd::Untrust => self.untrust(),
                TlsCmd::Status => self.tls_status(),
            },
            Cmd::Db { cmd } => self.db(cmd),
            Cmd::Import { all } => self.import(all),
            Cmd::Remote { cmd } => self.remote(cmd),
            Cmd::Config { cmd } => self.config(cmd),
            Cmd::Jobs { id } => self.jobs(id.as_deref()),
            Cmd::Service { cmd } => match cmd {
                ServiceAlias::Install { name, version } => {
                    self.service_install(&name, version.as_deref())
                }
                ServiceAlias::Start { name } => self.service_action(name, "start"),
                ServiceAlias::Stop { name } => self.service_action(name, "stop"),
                ServiceAlias::Restart { name } => self.service_action(name, "restart"),
                ServiceAlias::Logs {
                    name,
                    lines,
                    follow,
                } => self.logs(Some(&name), lines, follow),
                ServiceAlias::List => self.services(),
            },
            Cmd::Completions { .. } | Cmd::Mcp { .. } | Cmd::Daemon { .. } => unreachable!(),
        }
    }

    // ---- daemon ----------------------------------------------------------------------

    fn daemon(&self, foreground: bool, cmd: Option<DaemonCmd>) -> R {
        let cmd = match (foreground, cmd) {
            (true, _) => DaemonCmd::Run,
            (false, None) => DaemonCmd::Start,
            (false, Some(c)) => c,
        };
        match cmd {
            DaemonCmd::Run => daemon::run_foreground(&self.home),
            DaemonCmd::Start => {
                if self.client.health().is_err() {
                    self.ui.note(t("starting_daemon"));
                    daemon::spawn_detached(&self.home)?;
                    if !daemon::wait_healthy(&self.client, Duration::from_secs(10)) {
                        return Err(CliError::Daemon(t("daemon_unreachable").into()));
                    }
                }
                self.daemon_status()
            }
            DaemonCmd::Stop => {
                let stopped = daemon::stop(&self.client, &self.home)?;
                self.ui
                    .json_out(&json!({ "running": false, "stopped": stopped }));
                self.ui.ok(if stopped {
                    t("daemon_stopped")
                } else {
                    t("daemon_not_running")
                });
                Ok(0)
            }
            DaemonCmd::Status => self.daemon_status(),
        }
    }

    fn daemon_status(&self) -> R {
        match self.client.health() {
            Ok(h) => {
                let pid = self
                    .client
                    .port_owner(self.client.port())
                    .ok()
                    .and_then(|o| o.pid);
                self.ui.json_out(&json!({
                    "running": true, "version": h.version, "port": self.client.port(), "pid": pid
                }));
                let pid = pid.map(|p| format!(", pid {p}")).unwrap_or_default();
                self.ui.ok(format!(
                    "{} ({} {}, 127.0.0.1:{}{pid})",
                    t("daemon_started"),
                    "v",
                    h.version,
                    self.client.port()
                ));
                Ok(0)
            }
            Err(_) => {
                self.ui
                    .json_out(&json!({ "running": false, "port": self.client.port() }));
                self.ui
                    .line(format!("{} {}", self.ui.red("✘"), t("daemon_not_running")));
                Ok(3)
            }
        }
    }

    // ---- services --------------------------------------------------------------------

    fn service_install(&self, name: &str, version: Option<&str>) -> R {
        let acc = self.client.install_service(&service_name(name), version)?;
        let job = self.job(&acc.job_id)?;
        self.ui.ok(job.message.unwrap_or(job.title));
        Ok(0)
    }

    fn service_action(&self, name: Option<String>, action: &str) -> R {
        let name = name
            .map(|n| service_name(&n))
            .unwrap_or_else(|| "all".into());
        if name != "all" && action != "stop" {
            let svcs = self.client.services()?;
            let svc = svcs.iter().find(|s| s.name == name);
            if svc.is_none() && !resolve::is_service_name(&name) {
                return Err(not_found(format!("unknown service `{name}`")));
            }
            if svc
                .map(|s| s.status == ServiceStatus::NotInstalled)
                .unwrap_or(false)
            {
                let acc = self.client.install_service(&name, None)?;
                follow_job(&self.client, &acc.job_id, self.bar_visible())?;
            }
        }
        let v = self.client.service_action(&name, action)?;
        if self.ui.json {
            self.ui.json_out(&v);
            return Ok(0);
        }
        let list: Vec<Service> = if v.is_array() {
            serde_json::from_value(v).unwrap_or_default()
        } else {
            serde_json::from_value(v)
                .map(|s| vec![s])
                .unwrap_or_default()
        };
        for s in &list {
            let port = s.port.map(|p| format!(" :{p}")).unwrap_or_default();
            let msg = s
                .message
                .as_ref()
                .map(|m| format!("  {}", self.ui.dim(m)))
                .unwrap_or_default();
            let st = serde_json::to_value(s.status).ok();
            let st = st.as_ref().and_then(Value::as_str).unwrap_or("");
            let mark = if matches!(s.status, ServiceStatus::Crashed) {
                self.ui.red("✘")
            } else {
                self.ui.green("✔")
            };
            self.ui.line(format!(
                "{mark} {:<12} {}{port}{msg}",
                s.name,
                self.ui.status(st)
            ));
        }
        if action == "stop" && (name == "caddy" || name == "dns") {
            self.ui.note(
                self.ui
                    .yellow("note: with caddy/dns stopped no local site resolves or serves."),
            );
        }
        Ok(0)
    }

    fn services_table(&self, svcs: &[Service]) {
        let mut tb = self
            .ui
            .table(&["SERVICE", "STATUS", "PORT", "VERSION", "UPTIME", ""]);
        for s in svcs {
            let st = serde_json::to_value(s.status).ok();
            let st = st
                .as_ref()
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            tb.add_row(vec![
                s.name.clone(),
                self.ui.status(&st),
                s.port.map(|p| p.to_string()).unwrap_or_else(|| "–".into()),
                s.version.clone().unwrap_or_else(|| "–".into()),
                s.uptime_secs
                    .map(human_duration)
                    .unwrap_or_else(|| "–".into()),
                s.message
                    .clone()
                    .map(|m| format!("({m})"))
                    .unwrap_or_default(),
            ]);
        }
        self.ui.print_table(&tb);
    }

    fn services(&self) -> R {
        let s = self.client.services()?;
        self.ui.json_out(&s);
        self.services_table(&s);
        Ok(0)
    }

    fn status(&self) -> R {
        let o = self.client.overview()?;
        if self.ui.json {
            self.ui.json_out(&o);
            return Ok(0);
        }
        let ui = &self.ui;
        ui.line(format!(
            "{} {} · {}/{} · {} · tld .{} · http :{} https :{}",
            ui.bold("BOZKURT CODE"),
            o.version,
            o.os,
            o.arch,
            tilde(&o.home),
            o.tld,
            o.http_port,
            o.https_port
        ));
        let flag = |b: bool, yes: &str, no: &str| {
            if b {
                format!("{} {yes}", ui.green("✔"))
            } else {
                format!("{} {no}", ui.red("✘"))
            }
        };
        ui.line(format!(
            "DNS {}   CA {}",
            flag(
                o.dns_configured,
                "configured",
                "not configured (bz dns setup)"
            ),
            flag(o.ca_trusted, "trusted", "not trusted (bz trust)")
        ));
        self.services_table(&o.services);
        let rts: Vec<String> = RuntimeKind::ALL
            .iter()
            .map(|k| {
                let a = o
                    .runtimes
                    .iter()
                    .find(|r| r.kind == *k)
                    .and_then(|r| r.active.clone())
                    .unwrap_or_else(|| "–".into());
                format!("{k} {a}")
            })
            .collect();
        ui.line(format!("{}  {}", ui.bold("RUNTIMES"), rts.join(" · ")));
        ui.line(format!("{}     {}", ui.bold("SITES"), o.sites_count));
        Ok(0)
    }

    // ---- runtimes --------------------------------------------------------------------

    fn runtime(&self, kind: RuntimeKind, cmd: RtCmd) -> R {
        match cmd {
            RtCmd::List { available, refresh } => {
                let mut vs = self.client.versions(kind, refresh)?;
                if !available {
                    vs.retain(|v| v.installed);
                }
                self.ui.json_out(&vs);
                let mut tb = self.ui.table(&[
                    "VERSION",
                    "CHANNEL",
                    "INSTALLED",
                    "ACTIVE",
                    "RELEASED",
                    "SIZE",
                ]);
                for v in &vs {
                    let ch = serde_json::to_value(v.channel).ok();
                    tb.add_row(vec![
                        if v.active {
                            self.ui.green(&v.version)
                        } else {
                            v.version.clone()
                        },
                        ch.as_ref()
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        if v.installed {
                            self.ui.green("✔")
                        } else {
                            String::new()
                        },
                        if v.active {
                            self.ui.green("●")
                        } else {
                            String::new()
                        },
                        v.released.clone().unwrap_or_default(),
                        v.size_bytes.map(human_bytes).unwrap_or_default(),
                    ]);
                }
                if vs.is_empty() {
                    self.ui.line(format!(
                        "{} (bz {kind} list --available)",
                        t("nothing_found")
                    ));
                } else {
                    self.ui.print_table(&tb);
                }
                Ok(0)
            }
            RtCmd::Install {
                version,
                allow_unverified,
            } => {
                let acc = self
                    .client
                    .install_runtime_with(kind, &version, allow_unverified)?;
                let job = self.job(&acc.job_id)?;
                self.ui
                    .ok(format!("{kind} {}", job.message.unwrap_or(job.title)));
                Ok(0)
            }
            RtCmd::Use { version } => {
                let vs = self.client.versions(kind, false)?;
                let resolved = match pick_version(&vs, &version, |v| v.installed) {
                    Some(v) => v,
                    None => {
                        self.ui
                            .confirm(&format!("{kind} {version} {}", t("install_first")), &[])?;
                        let acc = self.client.install_runtime(kind, &version)?;
                        follow_job(&self.client, &acc.job_id, self.bar_visible())?;
                        let vs = self.client.versions(kind, false)?;
                        pick_version(&vs, &version, |v| v.installed).unwrap_or(version.clone())
                    }
                };
                let rt = self.client.use_runtime(kind, &resolved)?;
                self.ui.json_out(&rt);
                self.ui
                    .ok(format!("{kind} {}", rt.active.unwrap_or(resolved)));
                Ok(0)
            }
            RtCmd::Uninstall { version } => {
                let vs = self.client.versions(kind, false)?;
                let resolved = pick_version(&vs, &version, |v| v.installed)
                    .ok_or_else(|| not_found(format!("{kind} {version} is not installed")))?;
                self.ui.confirm(
                    &format!("Uninstall {kind} {resolved}"),
                    &[format!("DELETE /runtimes/{kind}/{resolved}")],
                )?;
                self.client.uninstall_runtime(kind, &resolved)?;
                self.ui
                    .json_out(&json!({ "kind": kind, "version": resolved, "uninstalled": true }));
                self.ui.ok(format!("{kind} {resolved} uninstalled"));
                Ok(0)
            }
        }
    }

    // ---- sites -----------------------------------------------------------------------

    fn site_url(&self, s: &Site) -> String {
        let (scheme, port, default) = match self.client.overview() {
            Ok(o) if s.https => ("https", o.https_port, 443),
            Ok(o) => ("http", o.http_port, 80),
            Err(_) => (if s.https { "https" } else { "http" }, 0, 0),
        };
        if port == default || port == 0 {
            format!("{scheme}://{}", s.domain)
        } else {
            format!("{scheme}://{}:{port}", s.domain)
        }
    }

    fn site_line(&self, s: &Site) -> String {
        let scheme = if s.https { "https" } else { "http" };
        let target = match s.kind {
            SiteKind::Proxy => format!("127.0.0.1:{}", s.proxy_port.unwrap_or(0)),
            SiteKind::App => format!("`{}`", s.command.clone().unwrap_or_default()),
            _ => {
                let root = s.root.clone().unwrap_or_default();
                let p = if root.is_empty() {
                    s.path.clone()
                } else if Path::new(&root).is_absolute() {
                    root
                } else {
                    format!("{}/{}", s.path.trim_end_matches('/'), root)
                };
                tilde(&p)
            }
        };
        let extra = match (s.kind, &s.php) {
            (SiteKind::Php, Some(v)) => format!("  (php-fpm-{})", bz_core::version::major_minor(v)),
            _ => String::new(),
        };
        format!("{scheme}://{} → {target}{extra}", s.domain)
    }

    fn link(&self, a: LinkArgs) -> R {
        let path = match &a.path {
            Some(p) => p.clone(),
            None => std::env::current_dir()?,
        };
        let path = std::fs::canonicalize(&path)
            .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
        let path_s = path.to_string_lossy().to_string();
        let det = self.client.detect(&path_s)?;
        if !self.ui.json {
            let mut bits = vec![];
            if let Some(v) = &det.php {
                bits.push(format!("php {v}"));
            }
            if let Some(v) = &det.node {
                bits.push(format!("node {v}"));
            }
            if let Some(r) = &det.root {
                bits.push(format!("root {r}/"));
            }
            if let Some(c) = &det.command {
                bits.push(format!("`{c}`"));
            }
            let fw = det
                .framework
                .clone()
                .unwrap_or_else(|| det.kind.as_str().to_string());
            let extra = if bits.is_empty() {
                String::new()
            } else {
                format!(" ({})", bits.join(", "))
            };
            self.ui.note(format!("{} {fw}{extra}", t("detected")));
        }
        let tld = det
            .suggested_domain
            .rsplit_once('.')
            .map(|(_, t)| t.to_string())
            .unwrap_or_else(|| "test".into());
        let domain = a.domain.as_ref().map(|d| {
            let d = d.trim().trim_end_matches('.').to_ascii_lowercase();
            if d.contains('.') {
                d
            } else {
                format!("{d}.{tld}")
            }
        });
        let kind = if a.proxy.is_some() {
            Some(SiteKind::Proxy)
        } else if a.command.is_some() {
            Some(SiteKind::App)
        } else {
            a.kind
                .as_deref()
                .map(|k| k.parse::<SiteKind>())
                .transpose()
                .map_err(CliError::Usage)?
        };
        let aliases = if a.aliases.is_empty() {
            None
        } else {
            Some(a.aliases.clone())
        };
        let sites = self.client.sites()?;
        let existing = sites.iter().find(|s| {
            std::fs::canonicalize(&s.path)
                .map(|p| p == path)
                .unwrap_or(s.path == path_s)
        });
        let site = if let Some(s) = existing {
            let patch = SitePatch {
                domain,
                aliases,
                kind,
                root: a.root.clone(),
                php: a.php.clone(),
                node: a.node.clone(),
                proxy_port: a.proxy,
                command: a.command.clone(),
                https: a.https.then_some(true),
                wildcard: a.wildcard.then_some(true),
                ..Default::default()
            };
            self.client.patch_site(&s.id, &patch)?
        } else {
            let req = CreateSiteRequest {
                path: path_s,
                domain,
                kind,
                root: a.root.clone(),
                php: a.php.clone(),
                node: a.node.clone(),
                proxy_port: a.proxy,
                command: a.command.clone(),
                https: a.https.then_some(true),
                wildcard: a.wildcard.then_some(true),
                aliases,
                env: None,
            };
            self.client.create_site(&req)?
        };
        self.ui.json_out(&site);
        self.ui.ok(self.site_line(&site));
        Ok(0)
    }

    fn unlink(&self, site: Option<&str>) -> R {
        let s = resolve_site(&self.client, site)?;
        self.ui.confirm(
            &format!(
                "Remove site {} ({}). Project files are NOT deleted.",
                s.domain,
                tilde(&s.path)
            ),
            &[
                format!("DELETE /sites/{}", s.id),
                format!("remove certs/{}.pem", s.domain),
                "reload caddy".into(),
            ],
        )?;
        self.client.delete_site(&s.id)?;
        self.ui
            .json_out(&json!({ "id": s.id, "domain": s.domain, "removed": true }));
        self.ui.ok(format!(
            "{} {} ({})",
            t("unlinked"),
            s.domain,
            t("files_untouched")
        ));
        Ok(0)
    }

    fn sites(&self) -> R {
        let sites = self.client.sites()?;
        self.ui.json_out(&sites);
        if sites.is_empty() {
            self.ui.line(format!("{} (bz link)", t("nothing_found")));
            return Ok(0);
        }
        let mut tb = self
            .ui
            .table(&["DOMAIN", "KIND", "RUNTIME", "HTTPS", "PATH", "FRAMEWORK"]);
        for s in &sites {
            let rt = match s.kind {
                SiteKind::Php => s.php.clone().map(|v| format!("php {v}")),
                SiteKind::Proxy => s.proxy_port.map(|p| format!(":{p}")),
                SiteKind::App => {
                    let st = s.app_status.and_then(|x| serde_json::to_value(x).ok());
                    Some(format!(
                        "{}{}",
                        s.node
                            .clone()
                            .map(|v| format!("node {v} "))
                            .unwrap_or_default(),
                        st.as_ref()
                            .and_then(Value::as_str)
                            .map(|x| self.ui.status(x))
                            .unwrap_or_default()
                    ))
                }
                SiteKind::Static => None,
            };
            tb.add_row(vec![
                s.domain.clone(),
                s.kind.as_str().to_string(),
                rt.unwrap_or_else(|| "–".into()),
                if s.https {
                    self.ui.green("✔")
                } else {
                    "–".into()
                },
                tilde(&s.path),
                s.framework.clone().unwrap_or_default(),
            ]);
        }
        self.ui.print_table(&tb);
        Ok(0)
    }

    fn site_info(&self, site: Option<&str>) -> R {
        let s = resolve_site(&self.client, site)?;
        self.ui.json_out(&s);
        if !self.ui.json {
            let v = serde_json::to_value(&s).unwrap_or_default();
            if let Some(o) = v.as_object() {
                for (k, v) in o {
                    let val = match v {
                        Value::String(x) => x.clone(),
                        other => other.to_string(),
                    };
                    self.ui.line(format!("{:<12} {val}", self.ui.dim(k)));
                }
            }
        }
        Ok(0)
    }

    fn site_set(&self, site: &str, key: &str, value: &str) -> R {
        let s = resolve_site(&self.client, Some(site))?;
        let mut body = Map::new();
        body.insert(key.to_string(), parse_value(value));
        let v = self
            .client
            .patch(&format!("/sites/{}", enc(&s.id)), Value::Object(body))?;
        self.ui.json_out(&v);
        if let Ok(site) = serde_json::from_value::<Site>(v) {
            self.ui.ok(self.site_line(&site));
        }
        Ok(0)
    }

    fn open(&self, site: Option<&str>) -> R {
        let s = resolve_site(&self.client, site)?;
        let s = self.client.site(&s.id)?;
        let url = self.site_url(&s);
        self.ui.json_out(&json!({ "url": url }));
        self.ui.note(format!("{} {url}", t("opening")));
        open::that_detached(&url)
            .map_err(|e| CliError::Other(format!("cannot open browser: {e}")))?;
        Ok(0)
    }

    fn site_action(&self, site: Option<&str>, action: &str) -> R {
        let s = resolve_site(&self.client, site)?;
        let s = self.client.site_action(&s.id, action)?;
        self.ui.json_out(&s);
        let st = s.app_status.and_then(|x| serde_json::to_value(x).ok());
        let st = st
            .as_ref()
            .and_then(Value::as_str)
            .map(|x| format!("  [{}]", self.ui.status(x)))
            .unwrap_or_default();
        self.ui.ok(format!("{}{st}", self.site_line(&s)));
        Ok(0)
    }

    fn share(&self, site: Option<&str>) -> R {
        let s = resolve_site(&self.client, site)?;
        self.ui.confirm(
            &format!(
                "Share {} on a public URL? Anyone with the link can reach it.",
                s.domain
            ),
            &[format!("POST /share/{}", s.id)],
        )?;
        let acc = self
            .client
            .post_job(&format!("/share/{}", enc(&s.id)), json!({}))?;
        let mut url: Option<String> = None;
        let site_id = s.id.clone();
        let mut on_ev = |ev: &Value| {
            if ev["type"] == "share.url" && ev["site_id"].as_str() == Some(site_id.as_str()) {
                url = ev["url"].as_str().map(str::to_string);
            }
        };
        let res = follow_job_with(&self.client, &acc.job_id, self.bar_visible(), &mut on_ev);
        let stop = |c: &Client| {
            let _ = c.delete(&format!("/share/{}", enc(&s.id)));
        };
        let job = match res {
            Ok(j) => j,
            Err(CliError::Interrupted) => {
                stop(&self.client);
                self.ui.note(t("share_stopped"));
                return Err(CliError::Interrupted);
            }
            Err(e) => return Err(e),
        };
        let url = url.or_else(|| {
            job.message.as_deref().and_then(|m| {
                m.split_whitespace()
                    .find(|w| w.starts_with("https://"))
                    .map(str::to_string)
            })
        });
        let url = match url {
            Some(u) => u,
            None => wait_share_url(&self.client, &s.id)
                .ok_or_else(|| CliError::Other("no share URL received".into()))?,
        };
        if self.ui.json {
            println!("{}", json!({ "site_id": s.id, "url": url, "job": job }));
        } else {
            self.ui
                .ok(format!("{}: {}", t("share_url"), self.ui.bold(&url)));
            self.ui.note(t("share_stop_hint"));
        }
        while !events::interrupted() {
            std::thread::sleep(Duration::from_millis(200));
        }
        stop(&self.client);
        self.ui.note(t("share_stopped"));
        Ok(0)
    }

    // ---- logs ------------------------------------------------------------------------

    fn logs(&self, source: Option<&str>, lines: usize, follow: bool) -> R {
        // Resolve the source to (event source filter, fetch fn).
        enum Src {
            All,
            Service(String),
            Site(Box<Site>),
        }
        let src = match source {
            Some("all") => Src::All,
            Some(x) if resolve::is_service_name(x) => Src::Service(service_name(x)),
            Some(x) => Src::Site(Box::new(resolve_site(&self.client, Some(x))?)),
            None => match resolve_site(&self.client, None) {
                Ok(s) => Src::Site(Box::new(s)),
                Err(_) => Src::All,
            },
        };
        let (filter, initial): (Option<String>, Vec<String>) = match &src {
            Src::Service(n) => (
                Some(format!("service:{n}")),
                self.client.service_logs(n, lines)?.lines,
            ),
            Src::Site(s) => (
                Some(format!("site:{}", s.domain)),
                self.client.site_logs(&s.id, lines)?.lines,
            ),
            Src::All => {
                let mut out = vec![];
                for s in self.client.services()? {
                    if matches!(s.status, ServiceStatus::NotInstalled) {
                        continue;
                    }
                    if let Ok(l) = self.client.service_logs(&s.name, lines) {
                        out.extend(l.lines.into_iter().map(|x| format!("[{}] {x}", s.name)));
                    }
                }
                (None, out)
            }
        };
        if self.ui.json && !follow {
            self.ui.json_out(&json!({ "lines": initial }));
            return Ok(0);
        }
        for l in &initial {
            if self.ui.json {
                println!("{}", json!({ "type": "log", "source": filter, "line": l }));
            } else {
                println!("{l}");
            }
        }
        if !follow {
            return Ok(0);
        }
        let mut ws = EventStream::connect(&self.client)
            .ok_or_else(|| CliError::Daemon("cannot open the /events stream".into()))?;
        loop {
            if events::interrupted() {
                return Ok(0);
            }
            match ws.poll() {
                Ok(Some(ev)) if ev["type"] == "log" => {
                    let s = ev["source"].as_str().unwrap_or("");
                    if filter.as_deref().map(|f| f == s).unwrap_or(true) {
                        if self.ui.json {
                            println!("{ev}");
                        } else if filter.is_none() {
                            println!(
                                "{} {}",
                                self.ui.dim(&format!("[{s}]")),
                                ev["line"].as_str().unwrap_or("")
                            );
                        } else {
                            println!("{}", ev["line"].as_str().unwrap_or(""));
                        }
                    }
                }
                Ok(_) => {}
                Err(events::StreamClosed) => {
                    return Err(CliError::Daemon("event stream closed".into()))
                }
            }
        }
    }

    // ---- doctor / dns / tls ----------------------------------------------------------

    fn doctor(&self, fix: Option<&str>) -> R {
        if let Some(id) = fix {
            self.ui.confirm(
                &format!("Run doctor fix `{id}`"),
                &[format!("POST /doctor/fix/{id}")],
            )?;
            let v = self
                .client
                .post(&format!("/doctor/fix/{}", enc(id)), json!({}))?;
            if let Some(job_id) = v.get("job_id").and_then(Value::as_str) {
                self.job(job_id)?;
            } else {
                self.ui.json_out(&v);
            }
            self.ui.ok(id);
            return Ok(0);
        }
        let checks = self.client.doctor()?;
        self.ui.json_out(&checks);
        for c in &checks {
            let mark = match c.status {
                CheckStatus::Ok => self.ui.green("✔"),
                CheckStatus::Warn => self.ui.yellow("!"),
                CheckStatus::Fail => self.ui.red("✘"),
            };
            self.ui.line(format!(
                "{mark} {}  {}",
                self.ui.bold(&c.title),
                self.ui.dim(&c.detail)
            ));
            if c.status != CheckStatus::Ok {
                if let Some(f) = &c.fix {
                    self.ui.line(format!("    → {f}"));
                }
            }
        }
        Ok(if checks.iter().any(|c| c.status == CheckStatus::Fail) {
            1
        } else {
            0
        })
    }

    fn dns_setup(&self, tld: Option<String>) -> R {
        let cur = self.client.dns()?;
        let tld = tld.map(|t| t.trim_start_matches('.').to_string());
        let shown = tld.clone().unwrap_or(cur.tld.clone());
        self.ui.confirm(
            &format!(
                "Route *.{shown} to the BOZKURT resolver (127.0.0.1:{}).",
                cur.port
            ),
            &[
                "update the OS resolver configuration (backed up first)".into(),
                t("needs_admin").into(),
            ],
        )?;
        let body = match tld {
            Some(t) => json!({ "tld": t }),
            None => json!({}),
        };
        let acc = self.client.post_job("/dns/configure", body)?;
        self.job(&acc.job_id)?;
        self.ui.ok(format!("DNS: *.{shown} → 127.0.0.1"));
        Ok(0)
    }

    fn dns_undo(&self) -> R {
        self.ui.confirm(
            "Restore the OS resolver configuration from the backup.",
            &["POST /dns/unconfigure".into(), t("needs_admin").into()],
        )?;
        let acc = self.client.post_job("/dns/unconfigure", json!({}))?;
        self.job(&acc.job_id)?;
        self.ui.ok("DNS configuration restored");
        Ok(0)
    }

    fn dns_status(&self) -> R {
        let d = self.client.dns()?;
        self.ui.json_out(&d);
        let yn = |b: bool| {
            if b {
                self.ui.green("yes")
            } else {
                self.ui.red("no")
            }
        };
        self.ui.line(format!("tld         .{}", d.tld));
        self.ui.line(format!("port        {}", d.port));
        self.ui.line(format!("configured  {}", yn(d.configured)));
        self.ui.line(format!("running     {}", yn(d.running)));
        Ok(0)
    }

    fn trust(&self) -> R {
        self.ui.confirm(
            "Add the BOZKURT local CA to the system trust store.",
            &["POST /tls/trust".into(), t("needs_admin").into()],
        )?;
        let acc = self
            .client
            .post_job("/tls/trust", json!({ "system": true }))?;
        self.job(&acc.job_id)?;
        self.ui.ok("CA trusted");
        Ok(0)
    }

    fn untrust(&self) -> R {
        self.ui.confirm(
            "Remove the BOZKURT local CA from the trust stores it was added to.",
            &["POST /tls/untrust".into(), t("needs_admin").into()],
        )?;
        let acc = self.client.post_job("/tls/untrust", json!({}))?;
        self.job(&acc.job_id)?;
        self.ui.ok("CA removed from trust stores");
        Ok(0)
    }

    fn tls_status(&self) -> R {
        let s = self.client.tls()?;
        self.ui.json_out(&s);
        self.ui.line(format!("ca          {}", tilde(&s.ca_path)));
        self.ui.line(format!(
            "trusted     {}",
            if s.ca_trusted {
                self.ui.green("yes")
            } else {
                self.ui.red("no (bz trust)")
            }
        ));
        if let Some(e) = &s.ca_expires {
            self.ui.line(format!("expires     {e}"));
        }
        Ok(0)
    }

    // ---- databases -------------------------------------------------------------------

    fn db(&self, cmd: DbCmd) -> R {
        match cmd {
            DbCmd::List { engine } => {
                let e = db_engine(&engine);
                let v = self
                    .client
                    .get(&format!("/services/{}/databases", enc(&e)))?;
                self.ui.json_out(&v);
                for d in v["databases"].as_array().into_iter().flatten() {
                    self.ui.line(d.as_str().unwrap_or_default());
                }
                Ok(0)
            }
            DbCmd::Create { name, engine } => {
                let e = db_engine(&engine);
                let v = self.client.post(
                    &format!("/services/{}/databases", enc(&e)),
                    json!({ "name": name }),
                )?;
                self.ui.json_out(&v);
                self.ui.ok(format!("database `{name}` created on {e}"));
                Ok(0)
            }
            DbCmd::Drop { name, engine } => {
                let e = db_engine(&engine);
                self.ui.confirm(
                    &format!("Drop database `{name}` on {e}. All its data is deleted."),
                    &[format!("DELETE /services/{e}/databases/{name}")],
                )?;
                let v = self.client.delete(&format!(
                    "/services/{}/databases/{}",
                    enc(&e),
                    enc(&name)
                ))?;
                self.ui.json_out(
                    &json!({ "engine": e, "name": name, "dropped": true, "response": v }),
                );
                self.ui.ok(format!("database `{name}` dropped"));
                Ok(0)
            }
        }
    }

    // ---- import ----------------------------------------------------------------------

    fn import(&self, all: bool) -> R {
        let scan = self.client.get("/import/scan")?;
        let vhosts: Vec<Value> = scan["vhosts"].as_array().cloned().unwrap_or_default();
        let dns: Vec<Value> = scan["dns"].as_array().cloned().unwrap_or_default();
        if vhosts.is_empty() && dns.is_empty() {
            self.ui.json_out(&scan);
            self.ui.line(t("nothing_found"));
            return Ok(0);
        }
        let mut items: Vec<(bool, Value)> = vec![];
        for v in &vhosts {
            items.push((true, v.clone()));
        }
        for d in &dns {
            items.push((false, d.clone()));
        }
        if !self.ui.json {
            for (i, (is_vhost, v)) in items.iter().enumerate() {
                let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
                let line = if *is_vhost {
                    format!(
                        "vhost  {:<24} {}  [{}]",
                        s("domain"),
                        tilde(&s("path")),
                        s("source")
                    )
                } else {
                    format!(
                        "dns    .{:<23} {}  [{}]",
                        s("tld"),
                        tilde(&s("file")),
                        s("source")
                    )
                };
                eprintln!("{:>3}. {line}", i + 1);
            }
        }
        let selected: Vec<usize> = if all {
            (0..items.len()).collect()
        } else {
            let ans = self.ui.ask(t("select_items"))?;
            parse_selection(&ans, items.len()).map_err(CliError::Usage)?
        };
        if selected.is_empty() {
            self.ui.line(t("aborted"));
            return Ok(0);
        }
        // DNS entries are never imported (the daemon answers 400 to a DNS-only body)
        if !selected.iter().any(|i| items[*i].0) {
            self.ui.line(
                "DNS settings of other tools are not imported; remove their resolver file yourself if it conflicts.",
            );
            return Ok(0);
        }
        let body = json!({
            "vhosts": selected.iter().filter(|i| items[**i].0).map(|i| items[*i].1.clone()).collect::<Vec<_>>(),
            "dns": selected.iter().filter(|i| !items[**i].0).map(|i| items[*i].1.clone()).collect::<Vec<_>>(),
        });
        self.ui.confirm(
            &format!(
                "Import {} item(s). Everything changed is backed up first.",
                selected.len()
            ),
            &["POST /import".into()],
        )?;
        let acc = self.client.post_job("/import", body)?;
        self.job(&acc.job_id)?;
        self.ui.ok(format!("imported {} item(s)", selected.len()));
        Ok(0)
    }

    // ---- remote ----------------------------------------------------------------------

    fn remote(&self, cmd: Option<RemoteCmd>) -> R {
        match cmd {
            None => {
                let r = self.client.get("/remote")?;
                self.ui.json_out(&r);
                let on = r["enabled"].as_bool().unwrap_or(false);
                self.ui.line(format!(
                    "remote      {}",
                    if on {
                        self.ui.green("enabled")
                    } else {
                        self.ui.dim("disabled")
                    }
                ));
                self.ui.line(format!("port        {}", r["port"]));
                let addrs: Vec<String> = r["addresses"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|a| a.as_str().map(str::to_string))
                    .collect();
                self.ui.line(format!("addresses   {}", addrs.join(", ")));
                self.ui.line(format!(
                    "devices     {}",
                    r["paired_devices"].as_array().map(|a| a.len()).unwrap_or(0)
                ));
                Ok(0)
            }
            Some(RemoteCmd::Enable) => {
                self.ui.confirm(
                    "Enable the LAN listener so paired phones can control this machine.",
                    &["POST /remote/enable".into()],
                )?;
                let r = self.client.post("/remote/enable", json!({}))?;
                self.ui.json_out(&r);
                self.ui.ok(format!("remote enabled on port {}", r["port"]));
                Ok(0)
            }
            Some(RemoteCmd::Disable) => {
                let r = self.client.post("/remote/disable", json!({}))?;
                self.ui.json_out(&r);
                self.ui.ok("remote disabled");
                Ok(0)
            }
            Some(RemoteCmd::Pair) => {
                let c = self.client.post("/remote/pairing-code", json!({}))?;
                let r = self.client.get("/remote")?;
                let code = c["code"].as_str().unwrap_or_default().to_string();
                let host = format!(
                    "{}:{}",
                    r["addresses"][0].as_str().unwrap_or("127.0.0.1"),
                    r["port"].as_u64().unwrap_or(7879)
                );
                let uri = format!("bozkurt://pair?host={host}&code={code}");
                if self.ui.json {
                    println!(
                        "{}",
                        json!({ "code": code, "expires_at": c["expires_at"], "uri": uri })
                    );
                    return Ok(0);
                }
                self.ui
                    .line(format!("{}: {}", t("pair_code"), self.ui.bold(&code)));
                if let Ok(q) = qrcode::QrCode::new(uri.as_bytes()) {
                    let img = q
                        .render::<qrcode::render::unicode::Dense1x2>()
                        .dark_color(qrcode::render::unicode::Dense1x2::Light)
                        .light_color(qrcode::render::unicode::Dense1x2::Dark)
                        .build();
                    self.ui.line(img);
                }
                self.ui.line(self.ui.dim(&uri));
                if !r["enabled"].as_bool().unwrap_or(false) {
                    self.ui.note(
                        self.ui
                            .yellow("remote access is disabled: run `bz remote enable`"),
                    );
                }
                Ok(0)
            }
            Some(RemoteCmd::Devices) => {
                let r = self.client.get("/remote")?;
                let d = r["paired_devices"].clone();
                self.ui.json_out(&d);
                let mut tb = self.ui.table(&["ID", "NAME", "LAST SEEN"]);
                for x in d.as_array().into_iter().flatten() {
                    tb.add_row(vec![
                        x["id"].as_str().unwrap_or("").to_string(),
                        x["name"].as_str().unwrap_or("").to_string(),
                        x["last_seen"].as_str().unwrap_or("–").to_string(),
                    ]);
                }
                self.ui.print_table(&tb);
                Ok(0)
            }
            Some(RemoteCmd::Revoke { device_id }) => {
                self.ui.confirm(
                    &format!("Revoke device {device_id}; it will lose access."),
                    &[format!("DELETE /remote/devices/{device_id}")],
                )?;
                self.client
                    .delete(&format!("/remote/devices/{}", enc(&device_id)))?;
                self.ui
                    .json_out(&json!({ "id": device_id, "revoked": true }));
                self.ui.ok(format!("device {device_id} revoked"));
                Ok(0)
            }
        }
    }

    // ---- config / jobs ---------------------------------------------------------------

    fn config(&self, cmd: ConfigCmd) -> R {
        match cmd {
            ConfigCmd::Get { key } => {
                let cfg = self.client.get("/config")?;
                let v = match &key {
                    Some(k) => dotted_get(&cfg, k)
                        .ok_or_else(|| not_found(format!("no config key `{k}`")))?,
                    None => cfg,
                };
                if self.ui.json {
                    self.ui.json_out(&v);
                } else {
                    match &v {
                        Value::String(s) => println!("{s}"),
                        Value::Object(_) => {
                            println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default())
                        }
                        other => println!("{other}"),
                    }
                }
                Ok(0)
            }
            ConfigCmd::Set { key, value } => {
                let patch = dotted_patch(&key, parse_value(&value));
                let v = self.client.patch("/config", patch)?;
                self.ui.json_out(&v);
                let now = dotted_get(&v, &key).unwrap_or(Value::Null);
                self.ui.ok(format!("{key} = {now}"));
                Ok(0)
            }
        }
    }

    fn jobs(&self, id: Option<&str>) -> R {
        match id {
            Some(id) => {
                let j = self.client.job(id)?;
                self.ui.json_out(&j);
                self.ui.line(format!(
                    "{}  {}",
                    self.ui.bold(&j.title),
                    self.ui.dim(&j.id)
                ));
                self.ui.line(format!(
                    "status      {} ({:.0}%)",
                    self.ui.status(job_status(&j)),
                    j.progress * 100.0
                ));
                if let Some(m) = &j.message {
                    self.ui.line(format!("message     {m}"));
                }
                self.ui.line(format!("started     {}", j.started_at));
                if let Some(f) = &j.finished_at {
                    self.ui.line(format!("finished    {f}"));
                }
                Ok(0)
            }
            None => {
                let js = self.client.jobs()?;
                self.ui.json_out(&js);
                let mut tb = self
                    .ui
                    .table(&["ID", "TITLE", "STATUS", "PROGRESS", "MESSAGE"]);
                for j in &js {
                    tb.add_row(vec![
                        j.id.chars().take(8).collect::<String>(),
                        j.title.clone(),
                        self.ui.status(job_status(j)),
                        format!("{:.0}%", j.progress * 100.0),
                        j.message.clone().unwrap_or_default(),
                    ]);
                }
                if js.is_empty() {
                    self.ui.line(t("nothing_found"));
                } else {
                    self.ui.print_table(&tb);
                }
                Ok(0)
            }
        }
    }

    // ---- mcp register ----------------------------------------------------------------

    fn mcp(&self, cmd: McpCmd) -> R {
        let dirs = Dirs::from_env();
        let mut env = vec![];
        if self.g.home.is_some() {
            env.push((
                "BOZKURT_HOME".to_string(),
                self.home.to_string_lossy().to_string(),
            ));
        }
        match cmd {
            McpCmd::Register {
                client,
                project,
                print,
                command,
            } => {
                let entry = Entry {
                    command: command.unwrap_or_else(|| register::default_command(&self.home)),
                    env,
                };
                if print {
                    if self.ui.json {
                        let v = json!({ "client": client.as_str(), "entry": entry.json(client) });
                        self.ui.json_out(&v);
                    } else {
                        print!("{}", entry.snippet(client));
                    }
                    return Ok(0);
                }
                let path = register::config_path(client, project, &dirs)?;
                let o = register::apply(&path, client, &entry, false)?;
                self.report_register(&o, true);
                Ok(0)
            }
            McpCmd::Unregister { client, project } => {
                let path = register::config_path(client, project, &dirs)?;
                let entry = Entry {
                    command: String::new(),
                    env,
                };
                let o = register::apply(&path, client, &entry, true)?;
                self.report_register(&o, false);
                Ok(0)
            }
        }
    }

    fn report_register(&self, o: &register::Outcome, add: bool) {
        self.ui.json_out(&json!({
            "path": o.path, "changed": o.changed, "backup": o.backup
        }));
        let p = o.path.display().to_string();
        match (o.changed, add) {
            (true, true) => self.ui.ok(format!("{} {p}", t("registered"))),
            (true, false) => self.ui.ok(format!("{} {p}", t("unregistered"))),
            (false, true) => self.ui.ok(format!("{} {p}", t("already_registered"))),
            (false, false) => self.ui.line(format!("{} {p}", t("not_registered"))),
        }
        if let Some(b) = &o.backup {
            self.ui
                .note(self.ui.dim(&format!("{} {}", t("backup"), b.display())));
        }
    }
}

fn wait_share_url(client: &Client, site_id: &str) -> Option<String> {
    let mut ws = EventStream::connect(client)?;
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(30) && !events::interrupted() {
        if let Ok(Some(ev)) = ws.poll() {
            if ev["type"] == "share.url" && ev["site_id"].as_str() == Some(site_id) {
                return ev["url"].as_str().map(str::to_string);
            }
        }
    }
    None
}

fn job_status(j: &Job) -> &'static str {
    match j.status {
        JobStatus::Running => "running",
        JobStatus::Done => "done",
        JobStatus::Failed => "failed",
    }
}

fn parse_kind(s: &str) -> Result<RuntimeKind, CliError> {
    s.parse().map_err(CliError::Usage)
}

fn db_engine(e: &str) -> String {
    match e.to_ascii_lowercase().as_str() {
        "postgresql" | "pg" => "postgres".into(),
        other => other.to_string(),
    }
}

/// `~` for the user's home in displayed paths.
pub fn tilde(p: &str) -> String {
    let home =
        std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).unwrap_or_default();
    if !home.is_empty() && p.starts_with(&home) {
        format!("~{}", &p[home.len()..])
    } else {
        p.to_string()
    }
}

/// Parse a CLI value: JSON when it parses (`8080`, `true`, `["a"]`), else a string.
pub fn parse_value(s: &str) -> Value {
    serde_json::from_str(s).unwrap_or_else(|_| Value::String(s.to_string()))
}

/// `{"remote":{"port":7880}}` from `remote.port` + `7880`.
pub fn dotted_patch(key: &str, value: Value) -> Value {
    key.split('.').rev().fold(value, |acc, k| {
        let mut m = Map::new();
        m.insert(k.to_string(), acc);
        Value::Object(m)
    })
}

pub fn dotted_get(v: &Value, key: &str) -> Option<Value> {
    key.split('.').try_fold(v, |cur, k| cur.get(k)).cloned()
}

/// `1,3-4` / `all` / `` → zero-based indexes.
pub fn parse_selection(s: &str, n: usize) -> Result<Vec<usize>, String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(vec![]);
    }
    if s.eq_ignore_ascii_case("all") || s == "*" {
        return Ok((0..n).collect());
    }
    let mut out = vec![];
    for part in s.split([',', ' ']).filter(|p| !p.is_empty()) {
        let (a, b) = match part.split_once('-') {
            Some((a, b)) => (a, b),
            None => (part, part),
        };
        let a: usize = a
            .trim()
            .parse()
            .map_err(|_| format!("bad selection `{part}`"))?;
        let b: usize = b
            .trim()
            .parse()
            .map_err(|_| format!("bad selection `{part}`"))?;
        if a == 0 || b > n || a > b {
            return Err(format!("selection `{part}` out of range 1..{n}"));
        }
        out.extend((a - 1)..b);
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotted() {
        assert_eq!(
            dotted_patch("remote.port", json!(7880)),
            json!({"remote":{"port":7880}})
        );
        assert_eq!(dotted_patch("tld", json!("dev")), json!({"tld":"dev"}));
        let v = json!({"remote":{"port":7879},"tld":"test"});
        assert_eq!(dotted_get(&v, "remote.port"), Some(json!(7879)));
        assert_eq!(dotted_get(&v, "nope.x"), None);
        assert_eq!(parse_value("8080"), json!(8080));
        assert_eq!(parse_value("dev"), json!("dev"));
        assert_eq!(parse_value("true"), json!(true));
    }

    #[test]
    fn selection() {
        assert_eq!(parse_selection("1,3-4", 5).unwrap(), vec![0, 2, 3]);
        assert_eq!(parse_selection("all", 2).unwrap(), vec![0, 1]);
        assert!(parse_selection("", 2).unwrap().is_empty());
        assert!(parse_selection("9", 2).is_err());
    }
}
