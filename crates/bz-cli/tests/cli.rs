//! `bz` end-to-end against a mock bozkurtd.

mod common;

use std::process::{Command, Output};

use common::mock::{routes, Mock};
use serde_json::Value;

fn bz(m: &Mock, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bz"))
        .args(args)
        .env("BOZKURT_HOME", m.home.path())
        .env("HOME", m.home.path())
        .env("NO_COLOR", "1")
        .env("LANG", "C")
        .env_remove("LC_ALL")
        .output()
        .unwrap()
}

fn stdout_json(o: &Output) -> Value {
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not JSON ({e}): {}\nstderr: {}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        )
    })
}

#[test]
fn sites_and_status_json() {
    let m = Mock::start(routes());
    let o = bz(&m, &["--json", "sites"]);
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(stdout_json(&o)[0]["domain"], "shop.test");

    let o = bz(&m, &["status", "--json"]);
    assert_eq!(o.status.code(), Some(0));
    let v = stdout_json(&o);
    assert_eq!(v["tld"], "test");
    assert_eq!(v["services"][0]["name"], "caddy");

    // human output works too
    let o = bz(&m, &["status"]);
    assert_eq!(o.status.code(), Some(0));
    let text = String::from_utf8_lossy(&o.stdout);
    assert!(text.contains("caddy") && text.contains("2h 11m"), "{text}");
}

#[test]
fn install_follows_job() {
    let m = Mock::start(routes());
    let o = bz(&m, &["--json", "php", "install", "8.3"]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let v = stdout_json(&o);
    assert_eq!(v["status"], "done");
    assert!(m.requests().iter().any(|(meth, p, b)| meth == "POST"
        && p == "/api/v1/runtimes/php/install"
        && serde_json::from_str::<Value>(b).unwrap()["version"] == "8.3"));
    assert!(m.saw("GET", "/api/v1/jobs/j1"));
}

#[test]
fn runtime_alias_and_list() {
    let m = Mock::start(routes());
    let o = bz(&m, &["--json", "runtime", "install", "php"]);
    assert_eq!(o.status.code(), Some(0));
    assert!(m
        .requests()
        .iter()
        .any(|(_, p, b)| p == "/api/v1/runtimes/php/install" && b.contains("latest")));
    let o = bz(&m, &["--json", "php", "list"]);
    let v = stdout_json(&o);
    assert_eq!(
        v.as_array().unwrap().len(),
        1,
        "only installed versions without --available"
    );
    let o = bz(&m, &["--json", "php", "list", "--available"]);
    assert_eq!(stdout_json(&o).as_array().unwrap().len(), 2);
}

#[test]
fn unlink_needs_confirmation() {
    let m = Mock::start(routes());
    let o = bz(&m, &["--json", "unlink", "shop"]);
    assert_eq!(o.status.code(), Some(6));
    let err: Value = serde_json::from_slice(&o.stderr).unwrap();
    assert_eq!(err["error"]["code"], "confirmation_required");
    assert!(!m.saw("DELETE", "/api/v1/sites/s1"));

    let o = bz(&m, &["--json", "-y", "unlink", "shop.test"]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(m.saw("DELETE", "/api/v1/sites/s1"));
}

#[test]
fn exit_codes() {
    let m = Mock::start(routes());
    // unknown site → 4 with the API error code on stderr
    let o = bz(&m, &["--json", "secure", "nope"]);
    assert_eq!(o.status.code(), Some(4));
    let err: Value = serde_json::from_slice(&o.stderr).unwrap();
    assert_eq!(err["error"]["code"], "not_found");
    // doctor with a failing check → 1, still prints the checks
    let o = bz(&m, &["--json", "doctor"]);
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(stdout_json(&o).as_array().unwrap().len(), 2);
    // usage error → 2
    let o = bz(&m, &["frobnicate"]);
    assert_eq!(o.status.code(), Some(2));
}

#[test]
fn daemon_unreachable_with_no_start() {
    let home = tempfile::tempdir().unwrap();
    // a port nobody listens on
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    std::fs::write(
        home.path().join("config.toml"),
        format!("api_port = {port}\n"),
    )
    .unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_bz"))
        .args(["--json", "--no-start", "sites"])
        .env("BOZKURT_HOME", home.path())
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(3));
    let err: Value = serde_json::from_slice(&o.stderr).unwrap();
    assert_eq!(err["error"]["code"], "daemon_unreachable");
    // daemon status: not running → 3, no auto-start
    let o = Command::new(env!("CARGO_BIN_EXE_bz"))
        .args(["--json", "daemon", "status"])
        .env("BOZKURT_HOME", home.path())
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(3));
    assert_eq!(stdout_json(&o)["running"], false);
}

#[test]
fn mcp_register_writes_and_is_idempotent() {
    let m = Mock::start(routes());
    let cursor = m.home.path().join(".cursor/mcp.json");
    std::fs::create_dir_all(cursor.parent().unwrap()).unwrap();
    std::fs::write(&cursor, r#"{"mcpServers":{"other":{"command":"x"}}}"#).unwrap();
    let o = bz(
        &m,
        &[
            "--json",
            "mcp",
            "register",
            "cursor",
            "--command",
            "bozkurt-mcp",
        ],
    );
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let v = stdout_json(&o);
    assert_eq!(v["changed"], true);
    assert!(v["backup"].is_string());
    let cfg: Value = serde_json::from_str(&std::fs::read_to_string(&cursor).unwrap()).unwrap();
    assert_eq!(cfg["mcpServers"]["bozkurt"]["command"], "bozkurt-mcp");
    assert_eq!(cfg["mcpServers"]["other"]["command"], "x");
    let o = bz(
        &m,
        &[
            "--json",
            "mcp",
            "register",
            "cursor",
            "--command",
            "bozkurt-mcp",
        ],
    );
    assert_eq!(stdout_json(&o)["changed"], false);
    // completions are local-only
    let o = bz(&m, &["completions", "bash"]);
    assert_eq!(o.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&o.stdout).contains("_bz"));
}
