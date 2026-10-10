//! `bz mcp register|unregister <client>`: write the `bozkurt-mcp` stdio entry into an AI
//! client's MCP config (JSON or Codex TOML), backing the file up first and merging
//! idempotently. See `docs/MCP.md#client-setup`.

use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::error::CliError;

/// Name of the server entry in every client config.
pub const ENTRY: &str = "bozkurt";

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum McpClient {
    #[value(name = "claude-code", alias = "claude")]
    ClaudeCode,
    Cursor,
    #[value(alias = "code", alias = "vs-code")]
    Vscode,
    Windsurf,
    #[value(alias = "gemini-cli")]
    Gemini,
    Codex,
}

impl McpClient {
    pub fn as_str(&self) -> &'static str {
        match self {
            McpClient::ClaudeCode => "claude-code",
            McpClient::Cursor => "cursor",
            McpClient::Vscode => "vscode",
            McpClient::Windsurf => "windsurf",
            McpClient::Gemini => "gemini",
            McpClient::Codex => "codex",
        }
    }

    /// Top-level key that holds servers in the JSON config.
    fn servers_key(&self) -> &'static str {
        match self {
            McpClient::Vscode => "servers",
            McpClient::Codex => "mcp_servers",
            _ => "mcpServers",
        }
    }

    fn is_toml(&self) -> bool {
        matches!(self, McpClient::Codex)
    }
}

/// Where config files live (injectable for tests).
#[derive(Debug, Clone)]
pub struct Dirs {
    /// User home directory.
    pub home: PathBuf,
    /// Project directory for `--project`.
    pub project: PathBuf,
    /// Per-user application config dir (`~/.config`, `~/Library/Application Support`, `%APPDATA%`).
    pub config: PathBuf,
    /// `$CODEX_HOME` if set.
    pub codex_home: Option<PathBuf>,
}

impl Dirs {
    pub fn from_env() -> Dirs {
        let home = user_home();
        let config = if cfg!(windows) {
            std::env::var_os("APPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join("AppData").join("Roaming"))
        } else if cfg!(target_os = "macos") {
            home.join("Library").join("Application Support")
        } else {
            std::env::var_os("XDG_CONFIG_HOME")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".config"))
        };
        Dirs {
            home,
            project: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            config,
            codex_home: std::env::var_os("CODEX_HOME")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
        }
    }
}

fn user_home() -> PathBuf {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Path of the config file for `client`.
pub fn config_path(client: McpClient, project: bool, d: &Dirs) -> Result<PathBuf, CliError> {
    Ok(match (client, project) {
        (McpClient::ClaudeCode, false) => d.home.join(".claude.json"),
        (McpClient::ClaudeCode, true) => d.project.join(".mcp.json"),
        (McpClient::Cursor, false) => d.home.join(".cursor").join("mcp.json"),
        (McpClient::Cursor, true) => d.project.join(".cursor").join("mcp.json"),
        (McpClient::Vscode, false) => d.config.join("Code").join("User").join("mcp.json"),
        (McpClient::Vscode, true) => d.project.join(".vscode").join("mcp.json"),
        (McpClient::Windsurf, false) => d
            .home
            .join(".codeium")
            .join("windsurf")
            .join("mcp_config.json"),
        (McpClient::Gemini, false) => d.home.join(".gemini").join("settings.json"),
        (McpClient::Gemini, true) => d.project.join(".gemini").join("settings.json"),
        (McpClient::Codex, false) => d
            .codex_home
            .clone()
            .unwrap_or_else(|| d.home.join(".codex"))
            .join("config.toml"),
        (McpClient::Windsurf | McpClient::Codex, true) => {
            return Err(CliError::Usage(format!(
                "{} has no project-level MCP config; drop --project",
                client.as_str()
            )))
        }
    })
}

/// What to write.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub command: String,
    pub env: Vec<(String, String)>,
}

impl Entry {
    /// JSON object for `client`.
    pub fn json(&self, client: McpClient) -> Value {
        let mut m = Map::new();
        if matches!(client, McpClient::ClaudeCode | McpClient::Vscode) {
            m.insert("type".into(), json!("stdio"));
        }
        m.insert("command".into(), json!(self.command));
        if !self.env.is_empty() {
            let env: Map<String, Value> = self
                .env
                .iter()
                .map(|(k, v)| (k.clone(), json!(v)))
                .collect();
            m.insert("env".into(), Value::Object(env));
        }
        Value::Object(m)
    }

    /// Snippet the user could paste (`--print`).
    pub fn snippet(&self, client: McpClient) -> String {
        if client.is_toml() {
            let (out, _) = merge_toml("", self).unwrap_or_default();
            out
        } else {
            let v = json!({ client.servers_key(): { ENTRY: self.json(client) } });
            serde_json::to_string_pretty(&v).unwrap_or_default() + "\n"
        }
    }
}

/// Strip `//` and `/* */` comments and trailing commas (JSONC, used by VS Code).
pub fn strip_jsonc(s: &str) -> String {
    let b: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let mut in_str = false;
    while i < b.len() {
        let c = b[i];
        if in_str {
            out.push(c);
            if c == '\\' && i + 1 < b.len() {
                out.push(b[i + 1]);
                i += 2;
                continue;
            }
            if c == '"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                out.push(c);
                i += 1;
            }
            '/' if b.get(i + 1) == Some(&'/') => {
                while i < b.len() && b[i] != '\n' {
                    i += 1;
                }
            }
            '/' if b.get(i + 1) == Some(&'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == '*' && b[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
            }
            ',' => {
                let mut j = i + 1;
                while j < b.len() && b[j].is_whitespace() {
                    j += 1;
                }
                if !matches!(b.get(j), Some('}') | Some(']')) {
                    out.push(c);
                }
                i += 1;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

fn parse_json(existing: &str) -> Result<Value, CliError> {
    if existing.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(existing)
        .or_else(|_| serde_json::from_str(&strip_jsonc(existing)))
        .map_err(|e| {
            CliError::Other(format!(
                "existing config is not valid JSON, not touching it: {e}"
            ))
        })
}

/// Merge the entry into a JSON config. Returns (new text, changed).
pub fn merge_json(
    existing: &str,
    client: McpClient,
    entry: &Entry,
) -> Result<(String, bool), CliError> {
    let mut root = parse_json(existing)?;
    let obj = root
        .as_object_mut()
        .ok_or_else(|| CliError::Other("config root is not a JSON object".into()))?;
    let servers = obj.entry(client.servers_key()).or_insert_with(|| json!({}));
    let servers = servers
        .as_object_mut()
        .ok_or_else(|| CliError::Other(format!("`{}` is not an object", client.servers_key())))?;
    let want = entry.json(client);
    if servers.get(ENTRY) == Some(&want) {
        return Ok((existing.to_string(), false));
    }
    servers.insert(ENTRY.into(), want);
    Ok((
        serde_json::to_string_pretty(&root).unwrap_or_default() + "\n",
        true,
    ))
}

/// Remove the entry from a JSON config. Returns (new text, changed).
pub fn remove_json(existing: &str, client: McpClient) -> Result<(String, bool), CliError> {
    let mut root = parse_json(existing)?;
    let removed = root
        .get_mut(client.servers_key())
        .and_then(Value::as_object_mut)
        .map(|s| s.remove(ENTRY).is_some())
        .unwrap_or(false);
    if !removed {
        return Ok((existing.to_string(), false));
    }
    Ok((
        serde_json::to_string_pretty(&root).unwrap_or_default() + "\n",
        true,
    ))
}

/// Merge `[mcp_servers.bozkurt]` into a Codex `config.toml`, preserving formatting.
pub fn merge_toml(existing: &str, entry: &Entry) -> Result<(String, bool), CliError> {
    use toml_edit::{value, DocumentMut, InlineTable, Item, Table};
    let mut doc: DocumentMut = existing.parse().map_err(|e| {
        CliError::Other(format!(
            "existing config is not valid TOML, not touching it: {e}"
        ))
    })?;
    let servers = doc
        .entry("mcp_servers")
        .or_insert_with(|| {
            let mut t = Table::new();
            t.set_implicit(true);
            Item::Table(t)
        })
        .as_table_like_mut()
        .ok_or_else(|| CliError::Other("`mcp_servers` is not a table".into()))?;
    // Compare with what is there.
    let current = servers.get(ENTRY).and_then(|i| i.as_table_like()).map(|t| {
        let cmd = t
            .get("command")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let mut env: Vec<(String, String)> = t
            .get("env")
            .and_then(|e| e.as_table_like())
            .map(|e| {
                e.iter()
                    .filter_map(|(k, v)| v.as_str().map(|v| (k.to_string(), v.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        env.sort();
        (cmd, env, t.len())
    });
    let mut want_env = entry.env.clone();
    want_env.sort();
    let want_len = 1 + usize::from(!entry.env.is_empty());
    if current == Some((Some(entry.command.clone()), want_env, want_len)) {
        return Ok((existing.to_string(), false));
    }
    let mut t = Table::new();
    t.insert("command", value(entry.command.clone()));
    if !entry.env.is_empty() {
        let mut it = InlineTable::new();
        for (k, v) in &entry.env {
            it.insert(k, v.as_str().into());
        }
        t.insert("env", value(it));
    }
    servers.insert(ENTRY, Item::Table(t));
    let mut out = doc.to_string();
    if !out.ends_with('\n') {
        out.push('\n');
    }
    Ok((out, true))
}

/// Remove `[mcp_servers.bozkurt]`.
pub fn remove_toml(existing: &str) -> Result<(String, bool), CliError> {
    let mut doc: toml_edit::DocumentMut = existing
        .parse()
        .map_err(|e| CliError::Other(format!("existing config is not valid TOML: {e}")))?;
    let removed = doc
        .get_mut("mcp_servers")
        .and_then(|i| i.as_table_like_mut())
        .map(|t| t.remove(ENTRY).is_some())
        .unwrap_or(false);
    if !removed {
        return Ok((existing.to_string(), false));
    }
    Ok((doc.to_string(), true))
}

/// Outcome of a register/unregister.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub path: PathBuf,
    pub changed: bool,
    pub backup: Option<PathBuf>,
}

fn backup(path: &Path) -> Result<Option<PathBuf>, CliError> {
    if !path.exists() {
        return Ok(None);
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut b = path.with_file_name(format!("{name}.bozkurt-backup-{stamp}"));
    let mut n = 1;
    while b.exists() {
        b = path.with_file_name(format!("{name}.bozkurt-backup-{stamp}-{n}"));
        n += 1;
    }
    std::fs::copy(path, &b)?;
    Ok(Some(b))
}

fn write_atomic(path: &Path, text: &str) -> Result<(), CliError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("bozkurt-tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Register (or, with `remove`, unregister) in the file at `path`.
pub fn apply(
    path: &Path,
    client: McpClient,
    entry: &Entry,
    remove: bool,
) -> Result<Outcome, CliError> {
    let existing = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.into()),
    };
    let (text, changed) = match (client.is_toml(), remove) {
        (true, false) => merge_toml(&existing, entry)?,
        (true, true) => remove_toml(&existing)?,
        (false, false) => merge_json(&existing, client, entry)?,
        (false, true) => remove_json(&existing, client)?,
    };
    if !changed {
        return Ok(Outcome {
            path: path.to_path_buf(),
            changed: false,
            backup: None,
        });
    }
    let b = backup(path)?;
    write_atomic(path, &text)?;
    Ok(Outcome {
        path: path.to_path_buf(),
        changed: true,
        backup: b,
    })
}

/// The command to put in the config: `bozkurt-mcp` if it is on PATH, else the absolute
/// path next to `bz` / in `<home>/bin`, else plain `bozkurt-mcp`.
pub fn default_command(bozkurt_home: &Path) -> String {
    let name = format!("bozkurt-mcp{}", std::env::consts::EXE_SUFFIX);
    if let Some(path) = std::env::var_os("PATH") {
        if std::env::split_paths(&path).any(|d| d.join(&name).is_file()) {
            return "bozkurt-mcp".into();
        }
    }
    let mut cands = Vec::new();
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    {
        cands.push(dir.join(&name));
    }
    cands.push(bozkurt_home.join("bin").join(&name));
    cands
        .into_iter()
        .find(|c| c.is_file())
        .map(|c| c.to_string_lossy().to_string())
        .unwrap_or_else(|| "bozkurt-mcp".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> Entry {
        Entry {
            command: "bozkurt-mcp".into(),
            env: vec![],
        }
    }

    #[test]
    fn json_merge_preserves_other_keys_and_is_idempotent() {
        let existing = r#"{"theme":"dark","mcpServers":{"other":{"command":"x"}},"zeta":1}"#;
        let (out, changed) = merge_json(existing, McpClient::Cursor, &entry()).unwrap();
        assert!(changed);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["theme"], "dark");
        assert_eq!(v["mcpServers"]["other"]["command"], "x");
        assert_eq!(v["mcpServers"]["bozkurt"], json!({"command":"bozkurt-mcp"}));
        // key order preserved
        assert!(out.find("theme").unwrap() < out.find("zeta").unwrap());
        let (out2, changed2) = merge_json(&out, McpClient::Cursor, &entry()).unwrap();
        assert!(!changed2);
        assert_eq!(out, out2);
    }

    #[test]
    fn vscode_and_claude_shapes() {
        let (out, _) = merge_json("", McpClient::Vscode, &entry()).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["servers"]["bozkurt"],
            json!({"type":"stdio","command":"bozkurt-mcp"})
        );
        let e = Entry {
            command: "/opt/bozkurt-mcp".into(),
            env: vec![("BOZKURT_HOME".into(), "/h".into())],
        };
        let (out, _) = merge_json("{}", McpClient::ClaudeCode, &e).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["mcpServers"]["bozkurt"],
            json!({"type":"stdio","command":"/opt/bozkurt-mcp","env":{"BOZKURT_HOME":"/h"}})
        );
    }

    #[test]
    fn jsonc_is_accepted() {
        let existing =
            "{\n // comment\n \"servers\": { /* x */ \"a\": {\"command\": \"http://x\"}, },\n}";
        let (out, changed) = merge_json(existing, McpClient::Vscode, &entry()).unwrap();
        assert!(changed);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["servers"]["a"]["command"], "http://x");
        assert!(v["servers"]["bozkurt"].is_object());
    }

    #[test]
    fn invalid_json_is_not_touched() {
        assert!(merge_json("{ nope", McpClient::Cursor, &entry()).is_err());
    }

    #[test]
    fn toml_merge_and_remove() {
        let existing =
            "# my codex config\nmodel = \"o3\"\n\n[mcp_servers.other]\ncommand = \"x\"\n";
        let (out, changed) = merge_toml(existing, &entry()).unwrap();
        assert!(changed);
        assert!(out.starts_with("# my codex config\nmodel = \"o3\""));
        let t: toml::Table = out.parse().unwrap();
        assert_eq!(
            t["mcp_servers"]["bozkurt"]["command"].as_str(),
            Some("bozkurt-mcp")
        );
        assert_eq!(t["mcp_servers"]["other"]["command"].as_str(), Some("x"));
        let (out2, changed2) = merge_toml(&out, &entry()).unwrap();
        assert!(!changed2);
        assert_eq!(out, out2);
        let (out3, removed) = remove_toml(&out).unwrap();
        assert!(removed);
        let t: toml::Table = out3.parse().unwrap();
        assert!(t["mcp_servers"].get("bozkurt").is_none());
        assert!(t["mcp_servers"].get("other").is_some());
    }

    #[test]
    fn apply_backs_up_and_is_idempotent_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let d = Dirs {
            home: dir.path().into(),
            project: dir.path().join("proj"),
            config: dir.path().join("cfg"),
            codex_home: None,
        };
        let path = config_path(McpClient::Gemini, false, &d).unwrap();
        assert_eq!(path, dir.path().join(".gemini/settings.json"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"theme":"x"}"#).unwrap();
        let o = apply(&path, McpClient::Gemini, &entry(), false).unwrap();
        assert!(o.changed);
        let b = o.backup.unwrap();
        assert_eq!(std::fs::read_to_string(&b).unwrap(), r#"{"theme":"x"}"#);
        let o2 = apply(&path, McpClient::Gemini, &entry(), false).unwrap();
        assert!(!o2.changed && o2.backup.is_none());
        let o3 = apply(&path, McpClient::Gemini, &entry(), true).unwrap();
        assert!(o3.changed);
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(v["mcpServers"].get("bozkurt").is_none());
        assert_eq!(v["theme"], "x");
        // codex file created from scratch, project scope rejected
        let cp = config_path(McpClient::Codex, false, &d).unwrap();
        let o4 = apply(&cp, McpClient::Codex, &entry(), false).unwrap();
        assert!(o4.changed && o4.backup.is_none());
        assert!(config_path(McpClient::Codex, true, &d).is_err());
        assert_eq!(
            config_path(McpClient::Vscode, true, &d).unwrap(),
            dir.path().join("proj/.vscode/mcp.json")
        );
    }
}
