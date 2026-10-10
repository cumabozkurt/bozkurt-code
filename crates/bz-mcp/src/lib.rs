//! # bz-mcp — `bozkurt-mcp`, the stdio MCP server of BOZKURT CODE
//!
//! A minimal JSON-RPC 2.0 Model Context Protocol server (newline-delimited messages on
//! stdin/stdout; logs only on stderr). Supports `initialize` (protocol 2025-06-18 with
//! negotiation down to 2025-03-26 / 2024-11-05), `ping`, `tools/list`, `tools/call`.
//! Every tool calls the local bozkurtd HTTP API; see `docs/MCP.md`.

pub mod handlers;
pub mod tools;

use std::io::{BufRead, Write};
use std::path::PathBuf;

use bz_cli::{daemon, CliError, Client};
use serde_json::{json, Value};

/// Newest protocol revision first.
pub const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "BOZKURT CODE controls the local dev environment (runtimes, services, sites, \
databases). Run `doctor` first when something is broken and follow its `fix` fields. Tools marked \
[tier: destructive] or [tier: system] return a preview unless called with confirm: true. Show the preview \
to the user and only confirm after they agree. Long operations return a job_id: poll job_get, or pass wait.";

pub struct Server {
    client: Client,
    home: PathBuf,
    autostart: bool,
    daemon_ok: bool,
    protocol: &'static str,
    tools: Vec<tools::ToolDef>,
}

impl Server {
    /// Server for the local daemon of `home` (port from `config.toml`).
    pub fn new(home: PathBuf) -> Server {
        let client = daemon::client_for(&home);
        Server::with_client(client, home)
    }

    pub fn with_client(client: Client, home: PathBuf) -> Server {
        Server {
            client,
            home,
            autostart: std::env::var_os("BOZKURT_MCP_NO_START").is_none(),
            daemon_ok: false,
            protocol: PROTOCOL_VERSIONS[0],
            tools: tools::all(),
        }
    }

    /// Handle one parsed JSON-RPC message (or batch). `None` for notifications.
    pub fn handle(&mut self, msg: Value) -> Option<Value> {
        if let Value::Array(items) = msg {
            if items.is_empty() {
                return Some(err_resp(Value::Null, -32600, "empty batch"));
            }
            let out: Vec<Value> = items.into_iter().filter_map(|m| self.handle(m)).collect();
            return if out.is_empty() {
                None
            } else {
                Some(Value::Array(out))
            };
        }
        let Some(obj) = msg.as_object() else {
            return Some(err_resp(Value::Null, -32600, "invalid request"));
        };
        let id = obj.get("id").cloned();
        let Some(method) = obj.get("method").and_then(Value::as_str) else {
            // A response to something we never send, or garbage.
            return id.map(|id| err_resp(id, -32600, "invalid request: missing method"));
        };
        let params = obj.get("params").cloned().unwrap_or(json!({}));
        let Some(id) = id else {
            // notification: notifications/initialized, notifications/cancelled, ...
            return None;
        };
        let result = match method {
            "initialize" => Ok(self.initialize(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => {
                Ok(json!({ "tools": self.tools.iter().map(|t| t.to_json()).collect::<Vec<_>>() }))
            }
            "tools/call" => self.tools_call(&params),
            "resources/list" => Ok(json!({ "resources": [] })),
            "prompts/list" => Ok(json!({ "prompts": [] })),
            "logging/setLevel" => Ok(json!({})),
            other => Err((-32601, format!("method not found: {other}"))),
        };
        Some(match result {
            Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
            Err((code, m)) => err_resp(id, code, &m),
        })
    }

    fn initialize(&mut self, params: &Value) -> Value {
        let asked = params
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or("");
        self.protocol = negotiate(asked);
        json!({
            "protocolVersion": self.protocol,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": {
                "name": "bozkurt",
                "title": "BOZKURT CODE",
                "version": env!("CARGO_PKG_VERSION")
            },
            "instructions": INSTRUCTIONS
        })
    }

    fn tools_call(&mut self, params: &Value) -> Result<Value, (i64, String)> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or((-32602, "missing tool name".to_string()))?;
        let tool = self
            .tools
            .iter()
            .find(|t| t.name == name)
            .ok_or((-32602, format!("unknown tool: {name}")))?;
        let tier = tool.tier;
        let args = match params.get("arguments") {
            None | Some(Value::Null) => json!({}),
            Some(v @ Value::Object(_)) => v.clone(),
            Some(_) => return Err((-32602, "arguments must be an object".into())),
        };
        if !self.daemon_ok {
            match daemon::ensure_running(&self.client, &self.home, self.autostart, true) {
                Ok(()) => self.daemon_ok = true,
                Err(e) => return Ok(self.tool_error(&e)),
            }
        }
        match handlers::call(&self.client, name, tier, &args) {
            Ok(o) => {
                let text = format!(
                    "{}\n\n{}",
                    o.summary,
                    serde_json::to_string_pretty(&o.data).unwrap_or_default()
                );
                let mut r =
                    json!({ "content": [{ "type": "text", "text": text }], "isError": false });
                if self.protocol >= "2025-06-18" {
                    r["structuredContent"] = if o.data.is_object() {
                        o.data
                    } else {
                        json!({ "result": o.data })
                    };
                }
                Ok(r)
            }
            Err(e) => {
                if matches!(e, CliError::Daemon(_)) {
                    self.daemon_ok = false;
                }
                Ok(self.tool_error(&e))
            }
        }
    }

    fn tool_error(&self, e: &CliError) -> Value {
        let code = match e {
            CliError::Usage(_) => "invalid_input",
            other => other.code(),
        };
        let body = json!({ "error": { "code": code, "message": e.message() } });
        let mut r = json!({
            "content": [{ "type": "text", "text": format!("Error [{code}]: {}\n\n{body}", e.message()) }],
            "isError": true
        });
        if self.protocol >= "2025-06-18" {
            r["structuredContent"] = body;
        }
        r
    }
}

/// Pick the protocol version: the client's if we support it, else our newest.
pub fn negotiate(asked: &str) -> &'static str {
    PROTOCOL_VERSIONS
        .iter()
        .find(|v| **v == asked)
        .copied()
        .unwrap_or(PROTOCOL_VERSIONS[0])
}

fn err_resp(id: Value, code: i64, msg: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": msg } })
}

/// Serve newline-delimited JSON-RPC on the given streams until EOF.
pub fn serve<R: BufRead, W: Write>(
    server: &mut Server,
    input: R,
    mut output: W,
) -> std::io::Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => server.handle(msg),
            Err(e) => Some(err_resp(Value::Null, -32700, &format!("parse error: {e}"))),
        };
        if let Some(r) = reply {
            writeln!(output, "{r}")?;
            output.flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> Server {
        // Points at a closed port; protocol-level tests never reach it.
        let mut s = Server::with_client(Client::local(1), std::env::temp_dir());
        s.autostart = false;
        s
    }

    #[test]
    fn negotiation() {
        assert_eq!(negotiate("2025-06-18"), "2025-06-18");
        assert_eq!(negotiate("2024-11-05"), "2024-11-05");
        assert_eq!(negotiate("1999-01-01"), "2025-06-18");
    }

    #[test]
    fn initialize_ping_list() {
        let mut s = server();
        let r = s
            .handle(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}))
            .unwrap();
        assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(r["result"]["serverInfo"]["name"], "bozkurt");
        assert!(s
            .handle(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .is_none());
        assert_eq!(
            s.handle(json!({"jsonrpc":"2.0","id":"p","method":"ping"}))
                .unwrap()["result"],
            json!({})
        );
        let l = s
            .handle(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}))
            .unwrap();
        assert_eq!(l["result"]["tools"].as_array().unwrap().len(), 55);
        let e = s
            .handle(json!({"jsonrpc":"2.0","id":3,"method":"nope"}))
            .unwrap();
        assert_eq!(e["error"]["code"], -32601);
        let e = s
            .handle(json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"nope"}}))
            .unwrap();
        assert_eq!(e["error"]["code"], -32602);
    }

    #[test]
    fn daemon_down_is_a_tool_error() {
        let mut s = server();
        let r = s
            .handle(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"sites_list","arguments":{}}}))
            .unwrap();
        assert_eq!(r["result"]["isError"], true);
        assert_eq!(
            r["result"]["structuredContent"]["error"]["code"],
            "daemon_unreachable"
        );
    }

    #[test]
    fn serve_handles_parse_errors() {
        let mut s = server();
        let input = b"not json\n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n";
        let mut out = Vec::new();
        serve(&mut s, &input[..], &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        let lines: Vec<Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines[0]["error"]["code"], -32700);
        assert_eq!(lines[1]["result"], json!({}));
    }
}
