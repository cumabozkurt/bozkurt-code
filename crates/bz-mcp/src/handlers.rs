//! Tool implementations: each maps to bozkurtd API calls (`docs/MCP.md#tools`).

use std::collections::BTreeSet;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use bz_cli::client::{enc, Client};
use bz_cli::events::EventStream;
use bz_cli::resolve::{self, not_found, pick_version, resolve_site, service_name};
use bz_cli::CliError;
use bz_core::models::{JobStatus, RuntimeKind, ServicePatch, SiteKind};
use serde_json::{json, Map, Value};

use crate::tools::Tier;

/// Successful tool output: a one-line human summary plus the raw API JSON.
#[derive(Debug, Clone, PartialEq)]
pub struct Out {
    pub summary: String,
    pub data: Value,
}

fn out(summary: impl Into<String>, data: Value) -> Result<Out, CliError> {
    Ok(Out {
        summary: summary.into(),
        data,
    })
}

fn invalid(msg: impl Into<String>) -> CliError {
    CliError::Api {
        status: 400,
        code: "invalid_input".into(),
        message: msg.into(),
    }
}

fn arg_str<'a>(a: &'a Value, k: &str) -> Result<&'a str, CliError> {
    a.get(k)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| invalid(format!("missing required string argument `{k}`")))
}

fn opt_str<'a>(a: &'a Value, k: &str) -> Option<&'a str> {
    a.get(k)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
}

fn flag(a: &Value, k: &str) -> bool {
    a.get(k).and_then(Value::as_bool).unwrap_or(false)
}

fn lines(a: &Value) -> usize {
    a.get("lines")
        .and_then(Value::as_u64)
        .map(|n| n.clamp(1, 5000) as usize)
        .unwrap_or(200)
}

/// `wait`: false/absent → 0, true → 120 s, n → n s (max 600).
fn wait_secs(a: &Value) -> u64 {
    match a.get("wait") {
        Some(Value::Bool(true)) => 120,
        Some(Value::Number(n)) => n.as_u64().unwrap_or(0).min(600),
        _ => 0,
    }
}

fn preview(
    tool: &str,
    summary: String,
    changes: Vec<String>,
    args: &Value,
) -> Result<Out, CliError> {
    let mut confirm_with = args.as_object().cloned().unwrap_or_default();
    confirm_with.insert("confirm".into(), json!(true));
    let data = json!({
        "preview": true,
        "tool": tool,
        "summary": summary,
        "changes": changes,
        "confirm_with": Value::Object(confirm_with),
    });
    out(
        format!("PREVIEW ONLY, nothing was changed. {summary} Ask the user, then call again with confirm: true."),
        data,
    )
}

/// Poll a job for up to `secs`; returns the job JSON or the still-running stub.
fn wait_job(c: &Client, job_id: &str, secs: u64) -> Result<Out, CliError> {
    if secs == 0 {
        return out(
            format!("Started job {job_id}. Poll it with job_get until status is done or failed."),
            json!({ "job_id": job_id, "status": "running" }),
        );
    }
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let job = c.job(job_id)?;
        match job.status {
            JobStatus::Done => {
                return out(
                    format!(
                        "Job done: {}{}",
                        job.title,
                        job.message
                            .as_ref()
                            .map(|m| format!(" ({m})"))
                            .unwrap_or_default()
                    ),
                    serde_json::to_value(&job).unwrap_or_default(),
                )
            }
            JobStatus::Failed => {
                return Err(CliError::Api {
                    status: 500,
                    code: "job_failed".into(),
                    message: format!("{}: {}", job.title, job.message.unwrap_or_default()),
                })
            }
            JobStatus::Running if Instant::now() >= deadline => {
                return out(
                    format!(
                        "Job {job_id} still running ({:.0}%). Poll it with job_get.",
                        job.progress * 100.0
                    ),
                    json!({ "job_id": job_id, "status": "running", "job": job }),
                )
            }
            JobStatus::Running => std::thread::sleep(Duration::from_millis(500)),
        }
    }
}

fn body_without(a: &Value, drop: &[&str]) -> Value {
    let mut m: Map<String, Value> = a.as_object().cloned().unwrap_or_default();
    for k in drop {
        m.remove(*k);
    }
    Value::Object(m)
}

// ---- P1-18: shell commands need the user's confirmation ---------------------------------

/// Env var that lets an MCP client run site commands without a confirm round-trip.
pub const ALLOW_COMMANDS_ENV: &str = "BOZKURT_MCP_ALLOW_COMMANDS";

/// `(site id, command)` pairs the user confirmed in this MCP session.
static CONFIRMED_COMMANDS: Mutex<BTreeSet<(String, String)>> = Mutex::new(BTreeSet::new());

fn commands_allowed() -> bool {
    std::env::var(ALLOW_COMMANDS_ENV)
        .map(|v| matches!(v.trim(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

fn command_confirmed(site_id: &str, command: &str) -> bool {
    commands_allowed()
        || CONFIRMED_COMMANDS
            .lock()
            .expect("confirmed")
            .contains(&(site_id.to_string(), command.to_string()))
}

fn remember_command(site_id: &str, command: Option<&str>) {
    if let Some(c) = command.filter(|c| !c.trim().is_empty()) {
        CONFIRMED_COMMANDS
            .lock()
            .expect("confirmed")
            .insert((site_id.to_string(), c.to_string()));
    }
}

/// Does this create/update body set something that ends up in a shell (`command`) or in
/// the app's environment (`env`, e.g. `LD_PRELOAD` / `NODE_OPTIONS`)?
fn sets_command_or_env(a: &Value) -> bool {
    opt_str(a, "command").is_some() || a.get("env").is_some_and(|e| !e.is_null())
}

fn command_changes(a: &Value) -> Vec<String> {
    let mut v = vec![];
    if let Some(c) = opt_str(a, "command") {
        v.push(format!("command (runs via sh -c / cmd /C on start): {c}"));
    }
    if let Some(e) = a.get("env").and_then(Value::as_object) {
        for (k, val) in e {
            v.push(format!(
                "env {k}={}",
                val.as_str().unwrap_or(&val.to_string())
            ));
        }
    }
    v
}

fn kind_of(tool: &str) -> Option<(RuntimeKind, &str)> {
    let (k, op) = tool.split_once('_')?;
    Some((k.parse().ok()?, op))
}

/// Dispatch one tool call. `tier` comes from the catalog.
pub fn call(c: &Client, tool: &str, tier: Tier, a: &Value) -> Result<Out, CliError> {
    let confirmed = flag(a, "confirm");
    if let Some((kind, op)) = kind_of(tool) {
        return runtime(c, tool, kind, op, a, confirmed);
    }
    let _ = tier;
    match tool {
        "detect_project" => {
            let d = c.detect(arg_str(a, "path")?)?;
            out(
                format!(
                    "{} project ({}), suggested domain {}",
                    d.framework.clone().unwrap_or_else(|| "generic".into()),
                    d.kind.as_str(),
                    d.suggested_domain
                ),
                serde_json::to_value(d).unwrap_or_default(),
            )
        }
        "sites_list" => {
            let s = c.sites()?;
            let names: Vec<&str> = s.iter().map(|x| x.domain.as_str()).collect();
            out(
                format!("{} site(s): {}", s.len(), names.join(", ")),
                serde_json::to_value(&s).unwrap_or_default(),
            )
        }
        "sites_get" => {
            let s = resolve_site(c, Some(arg_str(a, "site")?))?;
            let s = c.site(&s.id)?;
            out(
                format!("{} ({}) → {}", s.domain, s.kind.as_str(), s.path),
                serde_json::to_value(&s).unwrap_or_default(),
            )
        }
        "sites_logs" => {
            let s = resolve_site(c, Some(arg_str(a, "site")?))?;
            let l = c.site_logs(&s.id, lines(a))?;
            out(
                format!("{} log line(s) of {}", l.lines.len(), s.domain),
                serde_json::to_value(&l).unwrap_or_default(),
            )
        }
        "sites_create" => {
            let path = arg_str(a, "path")?;
            if sets_command_or_env(a) && !confirmed && !commands_allowed() {
                return preview(
                    tool,
                    format!("Create a site for {path} that runs a shell command / custom environment when started."),
                    command_changes(a),
                    a,
                );
            }
            let v = c.post("/sites", body_without(a, &["confirm"]))?;
            // only a command the user saw in the preview counts as confirmed (a detected
            // one is confirmed on first start)
            if let (Some(id), Some(cmd)) = (v["id"].as_str(), opt_str(a, "command")) {
                if v["command"].as_str() == Some(cmd) {
                    remember_command(id, Some(cmd));
                }
            }
            let scheme = if v["https"].as_bool().unwrap_or(false) {
                "https"
            } else {
                "http"
            };
            out(
                format!(
                    "Created {scheme}://{} for {}. Undo with sites_delete.",
                    v["domain"].as_str().unwrap_or("?"),
                    v["path"].as_str().unwrap_or("?")
                ),
                v,
            )
        }
        "sites_update" => {
            let s = resolve_site(c, Some(arg_str(a, "site")?))?;
            let changes_cmd =
                opt_str(a, "command").is_some_and(|n| s.command.as_deref() != Some(n));
            if (changes_cmd || a.get("env").is_some_and(|e| !e.is_null()))
                && !confirmed
                && !commands_allowed()
            {
                return preview(
                    tool,
                    format!(
                        "Change the command / environment of {} (a running app restarts with it).",
                        s.domain
                    ),
                    command_changes(a),
                    a,
                );
            }
            let v = c.patch(
                &format!("/sites/{}", enc(&s.id)),
                body_without(a, &["site", "confirm"]),
            )?;
            if changes_cmd && v["command"].as_str() == opt_str(a, "command") {
                remember_command(&s.id, opt_str(a, "command"));
            }
            out(
                format!("Updated {}", v["domain"].as_str().unwrap_or(&s.domain)),
                v,
            )
        }
        "sites_start" | "sites_stop" | "sites_restart" | "sites_secure" | "sites_unsecure" => {
            let action = &tool["sites_".len()..];
            let s = resolve_site(c, Some(arg_str(a, "site")?))?;
            if matches!(action, "start" | "restart") && s.kind == SiteKind::App {
                let cmd = s.command.clone().unwrap_or_default();
                if !cmd.is_empty() && !command_confirmed(&s.id, &cmd) {
                    if !confirmed {
                        return preview(
                            tool,
                            format!(
                                "{action} app site {}: runs `{cmd}` in {}.",
                                s.domain, s.path
                            ),
                            vec![format!("sh -c {cmd:?} (cwd {})", s.path)],
                            a,
                        );
                    }
                    remember_command(&s.id, Some(&cmd));
                }
            }
            let s = c.site_action(&s.id, action)?;
            let undo = match action {
                "secure" => " Undo with sites_unsecure.",
                "unsecure" => " Undo with sites_secure.",
                "start" => " Undo with sites_stop.",
                "stop" => " Undo with sites_start.",
                _ => "",
            };
            out(
                format!("{action}: {} done.{undo}", s.domain),
                serde_json::to_value(&s).unwrap_or_default(),
            )
        }
        "sites_delete" => {
            let s = resolve_site(c, Some(arg_str(a, "site")?))?;
            if !confirmed {
                return preview(
                    tool,
                    format!(
                        "Remove site {} ({}). Project files are NOT deleted. Its certificate and Caddy entry are removed.",
                        s.domain, s.path
                    ),
                    vec![
                        format!("DELETE /sites/{}", s.id),
                        format!("remove certs/{}.pem", s.domain),
                        "reload caddy".into(),
                    ],
                    &json!({ "site": s.id }),
                );
            }
            c.delete_site(&s.id)?;
            out(
                format!(
                    "Removed site {}. Project files at {} were not touched.",
                    s.domain, s.path
                ),
                json!({ "id": s.id, "domain": s.domain, "removed": true }),
            )
        }
        "services_list" => {
            let s = c.services()?;
            let running = s
                .iter()
                .filter(|x| x.status == bz_core::models::ServiceStatus::Running)
                .count();
            out(
                format!("{} service(s), {running} running", s.len()),
                serde_json::to_value(&s).unwrap_or_default(),
            )
        }
        "services_logs" => {
            let n = service_name(arg_str(a, "name")?);
            let l = c.service_logs(&n, lines(a))?;
            out(
                format!("{} log line(s) of {n}", l.lines.len()),
                serde_json::to_value(&l).unwrap_or_default(),
            )
        }
        "services_install" => {
            let n = service_name(arg_str(a, "name")?);
            let acc = c.install_service(&n, opt_str(a, "version"))?;
            wait_job(c, &acc.job_id, wait_secs(a))
        }
        "services_start" | "services_stop" | "services_restart" => {
            let action = &tool["services_".len()..];
            let raw = arg_str(a, "name")?;
            let n = if raw.eq_ignore_ascii_case("all") {
                "all".to_string()
            } else {
                service_name(raw)
            };
            if n != "all" && action != "stop" {
                let svcs = c.services()?;
                match svcs.iter().find(|s| s.name == n) {
                    Some(s) if s.status == bz_core::models::ServiceStatus::NotInstalled => {
                        return Err(CliError::Api {
                            status: 409,
                            code: "not_installed".into(),
                            message: format!("{n} is not installed; call services_install {{\"name\":\"{n}\"}} first"),
                        })
                    }
                    None if !resolve::is_service_name(&n) => return Err(not_found(format!("unknown service `{n}`"))),
                    _ => {}
                }
            }
            let v = c.service_action(&n, action)?;
            let mut summary = format!("{action} {n}: ok.");
            if action == "stop" && matches!(n.as_str(), "caddy" | "dns" | "all") {
                summary.push_str(" WARNING: with caddy/dns stopped, every local site stops resolving or serving. Undo with services_start.");
            } else if action == "start" {
                summary.push_str(" Undo with services_stop.");
            }
            out(summary, v)
        }
        "services_configure" => {
            let n = service_name(arg_str(a, "name")?);
            let patch = ServicePatch {
                autostart: a.get("autostart").and_then(Value::as_bool),
                port: a
                    .get("port")
                    .and_then(Value::as_u64)
                    .and_then(|p| u16::try_from(p).ok()),
            };
            if patch.autostart.is_none() && patch.port.is_none() {
                return Err(invalid("give autostart and/or port"));
            }
            let s = c.configure_service(&n, &patch)?;
            out(
                format!("Configured {n}"),
                serde_json::to_value(&s).unwrap_or_default(),
            )
        }
        "db_list" | "db_create" | "db_drop" => {
            let engine = match opt_str(a, "engine")
                .unwrap_or("mariadb")
                .to_ascii_lowercase()
                .as_str()
            {
                "postgresql" | "pg" => "postgres".to_string(),
                o => o.to_string(),
            };
            let base = format!("/services/{}/databases", enc(&engine));
            match tool {
                "db_list" => {
                    let v = c.get(&base)?;
                    let n = v["databases"].as_array().map(|x| x.len()).unwrap_or(0);
                    out(format!("{n} database(s) on {engine}"), v)
                }
                "db_create" => {
                    let name = arg_str(a, "name")?;
                    let v = c.post(&base, json!({ "name": name }))?;
                    out(
                        format!("Created database `{name}` on {engine}. Undo with db_drop."),
                        v,
                    )
                }
                _ => {
                    let name = arg_str(a, "name")?;
                    if !confirmed {
                        return preview(
                            tool,
                            format!("Drop database `{name}` on {engine}. All its tables and data are deleted permanently."),
                            vec![format!("DELETE {base}/{name}")],
                            &json!({ "name": name, "engine": engine }),
                        );
                    }
                    c.delete(&format!("{base}/{}", enc(name)))?;
                    out(
                        format!("Dropped database `{name}` on {engine}."),
                        json!({ "engine": engine, "name": name, "dropped": true }),
                    )
                }
            }
        }
        "doctor" => {
            let d = c.doctor()?;
            let fails = d
                .iter()
                .filter(|x| x.status == bz_core::models::CheckStatus::Fail)
                .count();
            let warns = d
                .iter()
                .filter(|x| x.status == bz_core::models::CheckStatus::Warn)
                .count();
            out(
                format!("{} checks: {fails} fail, {warns} warn", d.len()),
                serde_json::to_value(&d).unwrap_or_default(),
            )
        }
        "dns_status" => {
            let d = c.dns()?;
            out(
                format!(
                    ".{} resolver on :{}, configured={}, running={}",
                    d.tld, d.port, d.configured, d.running
                ),
                serde_json::to_value(&d).unwrap_or_default(),
            )
        }
        "config_get" => {
            let cfg = c.get("/config")?;
            match opt_str(a, "key") {
                Some(k) => {
                    let v = bz_cli::commands::dotted_get(&cfg, k)
                        .ok_or_else(|| not_found(format!("no config key `{k}`")))?;
                    out(format!("{k} = {v}"), json!({ "key": k, "value": v }))
                }
                None => out("config.toml", cfg),
            }
        }
        "job_get" => {
            let j = c.job(arg_str(a, "job_id")?)?;
            let st = serde_json::to_value(j.status).unwrap_or_default();
            out(
                format!(
                    "{}: {} ({:.0}%){}",
                    j.title,
                    st.as_str().unwrap_or(""),
                    j.progress * 100.0,
                    j.message
                        .as_ref()
                        .map(|m| format!(" {m}"))
                        .unwrap_or_default()
                ),
                serde_json::to_value(&j).unwrap_or_default(),
            )
        }
        "config_set" => {
            let patch = a
                .get("patch")
                .filter(|p| p.is_object())
                .ok_or_else(|| invalid("`patch` must be an object"))?;
            if !confirmed {
                let cur = c.get("/config").unwrap_or(Value::Null);
                let changes: Vec<String> = patch
                    .as_object()
                    .into_iter()
                    .flatten()
                    .map(|(k, v)| {
                        format!("{k}: {} → {v}", cur.get(k).cloned().unwrap_or(Value::Null))
                    })
                    .collect();
                let global = patch
                    .as_object()
                    .map(|o| o.keys().any(|k| k == "tld" || k.ends_with("port")))
                    .unwrap_or(false);
                let mut summary = "Change config.toml.".to_string();
                if global {
                    summary.push_str(" Changing tld or ports affects every site.");
                }
                return preview(tool, summary, changes, &json!({ "patch": patch }));
            }
            let v = c.patch("/config", patch.clone())?;
            out("config.toml updated", v)
        }
        "tls_trust" => {
            if !confirmed {
                return preview(
                    tool,
                    "Add the BOZKURT local CA to the OS trust store. The OS may ask for an admin password.".into(),
                    vec!["POST /tls/trust".into(), "system trust store change (undo: bz trust --undo)".into()],
                    &json!({}),
                );
            }
            let system = a.get("system").and_then(Value::as_bool).unwrap_or(true);
            let acc = c.post_job("/tls/trust", json!({ "system": system }))?;
            wait_job(c, &acc.job_id, wait_secs(a))
        }
        "share_start" => {
            let s = resolve_site(c, Some(arg_str(a, "site")?))?;
            if !confirmed {
                return preview(
                    tool,
                    format!("Expose {} on a public internet URL; anyone with the link can reach it until share_stop.", s.domain),
                    vec![format!("POST /share/{}", s.id)],
                    &json!({ "site": s.id }),
                );
            }
            share_start(c, &s.id, &s.domain, wait_secs(a).max(30))
        }
        "share_stop" => {
            let s = resolve_site(c, Some(arg_str(a, "site")?))?;
            c.delete(&format!("/share/{}", enc(&s.id)))?;
            out(
                format!("Stopped sharing {}", s.domain),
                json!({ "site_id": s.id, "shared": false }),
            )
        }
        other => Err(not_found(format!("unknown tool `{other}`"))),
    }
}

fn share_start(c: &Client, site_id: &str, domain: &str, secs: u64) -> Result<Out, CliError> {
    let mut ws = EventStream::connect(c);
    let acc = c.post_job(&format!("/share/{}", enc(site_id)), json!({}))?;
    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut last_poll = Instant::now() - Duration::from_secs(5);
    let mut job_val = Value::Null;
    while Instant::now() < deadline {
        if let Some(w) = ws.as_mut() {
            match w.poll() {
                Ok(Some(ev))
                    if ev["type"] == "share.url" && ev["site_id"].as_str() == Some(site_id) =>
                {
                    let url = ev["url"].as_str().unwrap_or_default().to_string();
                    return out(
                        format!("{domain} is public at {url}. Stop with share_stop."),
                        json!({ "site_id": site_id, "url": url, "job_id": acc.job_id }),
                    );
                }
                Ok(_) => {}
                Err(_) => ws = None,
            }
        } else {
            std::thread::sleep(Duration::from_millis(300));
        }
        if last_poll.elapsed() >= Duration::from_secs(1) {
            last_poll = Instant::now();
            let j = c.job(&acc.job_id)?;
            if j.status == JobStatus::Failed {
                return Err(CliError::Api {
                    status: 500,
                    code: "job_failed".into(),
                    message: j.message.unwrap_or_else(|| "share failed".into()),
                });
            }
            if let Some(url) = j
                .message
                .as_deref()
                .and_then(|m| m.split_whitespace().find(|w| w.starts_with("https://")))
            {
                return out(
                    format!("{domain} is public at {url}. Stop with share_stop."),
                    json!({ "site_id": site_id, "url": url, "job": j }),
                );
            }
            job_val = serde_json::to_value(&j).unwrap_or_default();
        }
    }
    out(
        format!(
            "Share job {} started but no URL yet; poll job_get.",
            acc.job_id
        ),
        json!({ "site_id": site_id, "job_id": acc.job_id, "job": job_val }),
    )
}

fn runtime(
    c: &Client,
    tool: &str,
    kind: RuntimeKind,
    op: &str,
    a: &Value,
    confirmed: bool,
) -> Result<Out, CliError> {
    match op {
        "list" => {
            let mut vs = c.versions(kind, flag(a, "refresh"))?;
            if !flag(a, "available") {
                vs.retain(|v| v.installed);
            }
            let names: Vec<String> = vs.iter().take(15).map(|v| v.version.clone()).collect();
            out(
                format!(
                    "{} {kind} version(s): {}{}",
                    vs.len(),
                    names.join(", "),
                    if vs.len() > 15 { ", …" } else { "" }
                ),
                serde_json::to_value(&vs).unwrap_or_default(),
            )
        }
        "active" => {
            let rts = c.runtimes()?;
            let rt = rts.into_iter().find(|r| r.kind == kind);
            let active = rt.as_ref().and_then(|r| r.active.clone());
            let installed = rt.map(|r| r.installed).unwrap_or_default();
            let mut data = json!({ "kind": kind, "active": active, "installed": installed });
            let mut summary = format!(
                "{kind} active: {}",
                active.clone().unwrap_or_else(|| "none".into())
            );
            if let Some(p) = opt_str(a, "path") {
                let d = c.detect(p)?;
                let req = match kind {
                    RuntimeKind::Php => d.php.clone(),
                    RuntimeKind::Node => d.node.clone(),
                    _ => None,
                };
                let resolved = req
                    .as_deref()
                    .and_then(|r| bz_core::version::best_match(installed.iter(), r))
                    .or_else(|| active.clone());
                data["project"] = json!({ "path": p, "requires": req, "resolved": resolved });
                summary.push_str(&format!(
                    "; project requires {}, resolves to {}",
                    req.unwrap_or_else(|| "any".into()),
                    resolved.unwrap_or_else(|| format!("nothing installed (call {kind}_install)"))
                ));
            }
            out(summary, data)
        }
        "install" => {
            let acc = c.install_runtime(kind, arg_str(a, "version")?)?;
            wait_job(c, &acc.job_id, wait_secs(a))
        }
        "use" => {
            let want = arg_str(a, "version")?;
            let vs = c.versions(kind, false)?;
            let v = pick_version(&vs, want, |v| v.installed).unwrap_or_else(|| want.to_string());
            let rt = c.use_runtime(kind, &v)?;
            out(
                format!(
                    "{kind} {} is now the default",
                    rt.active.clone().unwrap_or(v)
                ),
                serde_json::to_value(&rt).unwrap_or_default(),
            )
        }
        "uninstall" => {
            let want = arg_str(a, "version")?;
            let vs = c.versions(kind, false)?;
            let v = pick_version(&vs, want, |v| v.installed)
                .ok_or_else(|| not_found(format!("{kind} {want} is not installed")))?;
            if !confirmed {
                return preview(
                    tool,
                    format!("Delete {kind} {v} from runtimes/{kind}/{v}. Sites pinned to it stop working until another version is set."),
                    vec![format!("DELETE /runtimes/{kind}/{v}")],
                    &json!({ "version": v }),
                );
            }
            c.uninstall_runtime(kind, &v)?;
            out(
                format!("Uninstalled {kind} {v}. Reinstall with {kind}_install."),
                json!({ "kind": kind, "version": v, "uninstalled": true }),
            )
        }
        _ => Err(not_found(format!("unknown tool `{tool}`"))),
    }
}
