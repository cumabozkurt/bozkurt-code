# `bozkurt-mcp`: MCP server reference

`bozkurt-mcp` is a [Model Context Protocol](https://modelcontextprotocol.io) server that speaks over **stdio**. It gives AI coding
agents such as Claude Code, Cursor, VS Code Copilot, Windsurf, Gemini CLI and Codex the same control over your local
environment that you have in the desktop app or `bz`. Every tool calls the [bozkurtd HTTP API](API.md). The MCP server
holds no state of its own.

The tool catalog below matches `crates/bz-mcp/src/tools.rs` (55 tools). The server also answers `resources/list` and
`prompts/list` (both empty) and supports the MCP protocol revisions `2025-06-18`, `2025-03-26` and `2024-11-05`.

## How it works

```
AI client ──stdio (JSON-RPC)──▶ bozkurt-mcp ──HTTP──▶ bozkurtd 127.0.0.1:<api_port>
```

- The server reads `api_port` from `$BOZKURT_HOME/config.toml` (default `7878`). If the daemon is not running, it starts it,
  just like `bz` does. Set `BOZKURT_MCP_NO_START=1` to turn that off.
- It only ever talks to the **local** API on `127.0.0.1`, never to the LAN remote listener. It authenticates with the
  per-install token from `$BOZKURT_HOME/api-token` (`Authorization: Bearer …`), which it reads automatically; there is
  nothing to configure. If the MCP client starts `bozkurt-mcp` with a different `BOZKURT_HOME` than the running daemon
  uses, calls fail with `401`.
- Long operations return a `job_id` immediately. The agent then calls `job_get` until the job is `done` or `failed`. Tools
  that start a job also take `wait`: `true` waits up to 120 s, a number waits that many seconds (max 600). Tool results
  include a short human summary and the raw API JSON.
- Errors come back as MCP tool errors carrying the API `error.code` (for example `not_found`, `conflict`, `confirmation_required`).

## Client setup

The easiest way is `bz mcp register <client>` (see [CLI.md](CLI.md#integrations)). The manual snippets for each client are below.
If the client cannot find `bozkurt-mcp` on its `PATH`, use the absolute path instead: `~/.bozkurt/bin/bozkurt-mcp`, or
`%LOCALAPPDATA%\Bozkurt\bin\bozkurt-mcp.exe` on Windows.

| Client | Where | Snippet |
|---|---|---|
| Claude Code | CLI | `claude mcp add --scope user bozkurt -- bozkurt-mcp` |
| Claude Code | `~/.claude.json` | `{"mcpServers":{"bozkurt":{"type":"stdio","command":"bozkurt-mcp"}}}` |
| Cursor | `~/.cursor/mcp.json` or `<project>/.cursor/mcp.json` | `{"mcpServers":{"bozkurt":{"command":"bozkurt-mcp"}}}` |
| VS Code (Copilot agent mode) | `<project>/.vscode/mcp.json` or user `mcp.json` | `{"servers":{"bozkurt":{"type":"stdio","command":"bozkurt-mcp"}}}` |
| Windsurf | `~/.codeium/windsurf/mcp_config.json` | `{"mcpServers":{"bozkurt":{"command":"bozkurt-mcp"}}}` |
| Gemini CLI | `~/.gemini/settings.json` | `{"mcpServers":{"bozkurt":{"command":"bozkurt-mcp"}}}` |
| Codex CLI | `~/.codex/config.toml` | `[mcp_servers.bozkurt]`<br>`command = "bozkurt-mcp"` |

To use a non-default home, pass it through the environment, for example `"env": {"BOZKURT_HOME": "/path"}` in JSON clients or
`env = { BOZKURT_HOME = "/path" }` in Codex.

Environment variables read by `bozkurt-mcp`:

| Variable | Effect |
|---|---|
| `BOZKURT_HOME` | BOZKURT home directory (default `~/.bozkurt`) |
| `BOZKURT_MCP_NO_START=1` | do not start the daemon when it is not running |
| `BOZKURT_MCP_ALLOW_COMMANDS=1` | skip the [shell command confirmation](#shell-commands-need-confirmation) for site commands and env |

## Safety tiers

Every tool belongs to one of four tiers. The tier is stated in the tool's description, so the agent and the user can see it.

| Tier | What it covers | Behaviour |
|---|---|---|
| **read** | Listing and inspecting: no side effects | Runs immediately. |
| **change** | Reversible changes that stay inside `$BOZKURT_HOME` or affect only processes BOZKURT owns: start/stop, link a site, install a runtime, switch versions | Runs immediately. The result says what changed and how to undo it. |
| **destructive** | Deletes data or something you would need to rebuild: unlink a site, uninstall a runtime, drop a database, write config | **Requires `confirm: true`.** Without it, the tool returns a **preview** and does nothing. |
| **system** | Touches the OS outside `$BOZKURT_HOME`, may need admin rights, or exposes your machine: trust store changes, public tunnels | **Requires `confirm: true`.** Without it, the tool returns a **preview**. With it, the OS may still show its own password or confirmation dialog. |

A preview looks like this:

```json
{
  "preview": true,
  "tool": "sites_delete",
  "summary": "Remove site shop.test (/Users/me/code/shop). Project files are NOT deleted. Its certificate and Caddy entry are removed.",
  "changes": ["DELETE /sites/7f3c…", "remove certs/shop.test.pem", "reload caddy"],
  "confirm_with": { "site": "7f3c…", "confirm": true }
}
```

### Shell commands need confirmation

A site's `command` runs through `sh -c` (`cmd /C` on Windows) when an `app` site starts, and its `env` reaches that
process (`LD_PRELOAD`, `NODE_OPTIONS` and similar are code execution too). So, although these tools are in the
**change** tier, they return a **preview** (the exact command and environment in `changes`) instead of acting unless
called with `confirm: true`:

- `sites_create` with `command` or `env`;
- `sites_update` that changes `command` or sets `env`;
- `sites_start` and `sites_restart` of an `app` site whose current command was not yet confirmed in this MCP session.

A confirmed `(site, command)` pair is remembered for the lifetime of the `bozkurt-mcp` process, so starting the same
command again does not ask twice; a new command, or a new MCP session, asks again. A command that came from project
detection (not one shown in a create/update preview) is confirmed on its first start. Set
`BOZKURT_MCP_ALLOW_COMMANDS=1` (also `true` / `yes`) in the server's environment to skip these previews in a setup
you trust, for example `"env": {"BOZKURT_MCP_ALLOW_COMMANDS": "1"}` in a JSON client config.

Runtime installs over MCP never pass `allow_unverified`: a download without a published checksum (static-php.dev
PHP builds on macOS and Linux) fails with `unverified_download` unless it was pinned by an earlier install from the
GUI or `bz … install --allow-unverified`, or `allow_unverified = true` is set in `config.toml`.

Clients should show the preview to the user and call again only after the user agrees. `confirm` is an ordinary boolean
argument, so a well-behaved agent asks first. Your MCP client's own tool-approval prompt is a second safety net, not a replacement.

**Deliberately not exposed over MCP:**

- Remote access: `/remote/*`, which covers enabling the LAN listener, generating pairing codes, pairing and revoking devices.
  An agent should never be able to open your machine to the network or mint credentials.
- Import from other tools (`/import/scan`, `/import`). It turns other tools' vhosts into sites (their DNS settings are only reported, never changed) and needs per-item human review in the GUI or CLI.
- DNS routing setup (`POST /dns/configure`). It is a one-time privileged OS change, so run `bz dns setup` yourself.
  `dns_status` is available to read the current state.

## Tools

55 tools in total. Arguments are JSON. `site` accepts a site id, a domain (`shop.test`) or a bare name (`shop`).
`version` may be partial (`8.3`, `22`) or `lts`/`latest` where it makes sense.

### Sites and projects (12)

| Tool | Tier | Arguments | API |
|---|---|---|---|
| `detect_project` | read | `path` | `POST /detect` |
| `sites_list` | read | — | `GET /sites` |
| `sites_get` | read | `site` | `GET /sites/{id}` |
| `sites_logs` | read | `site`, `lines?` (200) | `GET /sites/{id}/logs` |
| `sites_create` | change | `path`, `domain?`, `kind?`, `root?`, `php?`, `node?`, `proxy_port?`, `command?`, `https?`, `wildcard?`, `aliases?`, `env?`, `confirm?` | `POST /sites`; preview first when `command` or `env` is set ([why](#shell-commands-need-confirmation)) |
| `sites_update` | change | `site`, `path?`, any of the `sites_create` fields, `confirm?` | `PATCH /sites/{id}`; preview first when `command` changes or `env` is set |
| `sites_start` | change | `site` (kind `app`), `confirm?` | `POST /sites/{id}/start`; preview first for an app command not yet confirmed in this session |
| `sites_stop` | change | `site` | `POST /sites/{id}/stop` |
| `sites_restart` | change | `site`, `confirm?` | `POST /sites/{id}/restart`; preview as for `sites_start` |
| `sites_secure` | change | `site` | `POST /sites/{id}/secure` |
| `sites_unsecure` | change | `site` | `POST /sites/{id}/unsecure` |
| `sites_delete` | destructive | `site`, `confirm` | `DELETE /sites/{id}` (project files untouched) |

### Runtimes (25)

The same five tools exist for each of `php`, `node`, `python`, `bun` and `go`:

| Tool | Tier | Arguments | API |
|---|---|---|---|
| `<rt>_list` | read | `available?` (false), `refresh?` | `GET /runtimes/{kind}/versions` |
| `<rt>_active` | read | `path?` (resolve for a project dir) | `GET /runtimes` (+ `POST /detect` when `path` is given) |
| `<rt>_install` | change | `version`, `wait?` | `POST /runtimes/{kind}/install` → job |
| `<rt>_use` | change | `version` | `POST /runtimes/{kind}/use` |
| `<rt>_uninstall` | destructive | `version`, `confirm` | `DELETE /runtimes/{kind}/{version}` |

That is `php_list`, `php_active`, `php_install`, `php_use`, `php_uninstall`, `node_list` … `go_uninstall`.

### Services (7)

| Tool | Tier | Arguments | API |
|---|---|---|---|
| `services_list` | read | — | `GET /services` |
| `services_logs` | read | `name`, `lines?` | `GET /services/{name}/logs` |
| `services_install` | change | `name`, `version?`, `wait?` | `POST /services/{name}/install` → job |
| `services_start` | change | `name` or `"all"` | `POST /services/{name}/start` |
| `services_stop` | change | `name` or `"all"` | `POST /services/{name}/stop` |
| `services_restart` | change | `name` or `"all"` | `POST /services/{name}/restart` |
| `services_configure` | change | `name`, `autostart?`, `port?` | `PATCH /services/{name}` |

`services_stop` on `caddy` or `dns` stops **every** local site from resolving or serving. The result says so explicitly.

### Databases (3)

| Tool | Tier | Arguments | API |
|---|---|---|---|
| `db_list` | read | `engine?` (`mariadb`) | `GET /services/{engine}/databases` |
| `db_create` | change | `name`, `engine?` | `POST /services/{engine}/databases` → `201 {name}` |
| `db_drop` | destructive | `name`, `engine?`, `confirm` | `DELETE /services/{engine}/databases/{name}` |

`engine` is `mariadb` (default), `postgres`, or an `external` MySQL on the MariaDB port. `name` must match
`^[A-Za-z0-9_-]{1,64}$`. Credentials work as described in [CLI.md → Databases](CLI.md#databases).

### Environment and diagnostics (8)

| Tool | Tier | Arguments | API |
|---|---|---|---|
| `doctor` | read | — | `GET /doctor` |
| `dns_status` | read | — | `GET /dns` |
| `config_get` | read | `key?` | `GET /config` |
| `job_get` | read | `job_id` | `GET /jobs/{id}` |
| `config_set` | destructive | `patch` (object), `confirm` | `PATCH /config`. Changing `tld` or ports affects every site, so a preview is required. |
| `tls_trust` | system | `confirm`, `system?` (true: also the system-wide store), `wait?` | `POST /tls/trust {system}` → job |
| `share_start` | system | `site`, `confirm`, `wait?` (at least 30 s) | `POST /share/{id}` → job. The result includes the public URL from the `share.url` event. |
| `share_stop` | change | `site` | `DELETE /share/{id}` |

**Total: 55 tools.** Stopping a share is `change`, because it only reduces exposure.

## Example session

```text
user  › Set up this Laravel repo with HTTPS and a database.
agent → detect_project {"path": "/Users/me/code/shop"}
      ← {"framework":"laravel","kind":"php","root":"public","php":"8.3","suggested_domain":"shop.test"}
agent → php_list {}                      ← 8.3 not installed
agent → php_install {"version":"8.3"}    ← {"job_id":"…"}
agent → job_get {"job_id":"…"}           ← {"status":"done"}
agent → sites_create {"path":"/Users/me/code/shop","https":true}
      ← {"domain":"shop.test","https":true,…}
agent → db_create {"name":"shop"}
agent › Done: https://shop.test (PHP 8.3), database `shop` on MariaDB 127.0.0.1:3306 (user root).
```

## Writing good prompts for agents

- Ask the agent to run `doctor` first when something is broken. Its `fix` fields are written to be followed as-is.
- Tell the agent which project directory to use. Tools never guess a directory outside the one you give them.
- Expect previews for anything that deletes data, touches the OS or exposes a site publicly. That is how the server is designed.
