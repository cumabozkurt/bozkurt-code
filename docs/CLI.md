# `bz` CLI reference

`bz` is a thin client over the [bozkurtd HTTP API](API.md). Anything `bz` can do, the desktop app, the MCP server and
the mobile companion can do as well. Every command below lists the API call(s) behind it (paths are relative to
`/api/v1`). The source of truth is the clap definition in `crates/bz-cli/src/cli.rs`; `bz <command> --help` prints the
same options.

## Conventions

- **Daemon auto-start.** If `GET /health` fails, `bz` starts `bozkurtd` in the background, waits for it (up to 10 s), then
  runs the command. `--no-start` turns this off. `bz` reads the API port from `$BOZKURT_HOME/config.toml` (`api_port`,
  default `7878`).
- **Authentication.** `bz` sends the per-install token from `$BOZKURT_HOME/api-token` as `Authorization: Bearer <token>`
  with every request. The daemon creates that file (mode 0600) on start, and `bz` reads it on every request, so nothing
  needs configuring. See [SECURITY.md → Local API protection](../SECURITY.md#local-api-protection).
- **Jobs.** Long operations (installs, DNS setup, trust, import, share) return `202 {job_id}`. `bz` follows them through
  `GET /events` (falling back to polling `GET /jobs/{id}`) and shows a progress bar. Pressing Ctrl-C detaches from the job;
  the job itself keeps running.
- **Site argument.** Wherever `[site]` appears you may pass a site id, a domain (`shop.test`), an alias, a bare name
  (`shop`), or nothing. With nothing, `bz` uses the site whose `path` contains the current directory.
- **Service names.** `caddy`, `dns`, `mariadb` (alias `mysql`), `postgres` (aliases `postgresql`, `pg`), `redis` (alias
  `valkey`), `mailpit`, `php-fpm-<major.minor>`.
- **Confirmation.** Anything that needs admin rights, deletes data or exposes a site asks `Proceed? [y/N]` and lists what
  will change. `--yes` / `-y` skips the prompt. With `--json` and no `--yes`, the command fails with exit code `6` instead
  of prompting.

### Global options

These work before or after the subcommand.

| Option | Meaning |
|---|---|
| `--json` | Print the raw API response as JSON on stdout, with no colours, spinners or prompts. Errors go to stderr as `{"error":{"code","message"}}`. |
| `-y`, `--yes` | Assume "yes" to confirmation prompts. |
| `-q`, `--quiet` | Print only errors. |
| `-v`, `--verbose` | Log the HTTP requests made (repeat for more detail). |
| `--no-start` | Do not auto-start the daemon. Exit `3` if it isn't running. |
| `--home <DIR>` | Use this `BOZKURT_HOME` (same as the env var). |
| `--no-color` | Disable colours (also honours `NO_COLOR`). |
| `-h`, `--help` / `-V`, `--version` | Help / version. |

### Environment variables

| Variable | Meaning |
|---|---|
| `BOZKURT_HOME` | Root of everything (default `~/.bozkurt`, Windows `%LOCALAPPDATA%\Bozkurt`). |
| `NO_COLOR` | Disable colours. |
| `BOZKURT_DEV` | Read by `bozkurtd`, not `bz`: `1` lets the daemon accept the desktop/UI dev-server origins (`bz daemon run` passes the environment through). |

## Commands

### Services

| Command | API |
|---|---|
| `bz start [service\|all]` | `POST /services/{name}/start`, or `POST /services/all/start` |
| `bz stop [service\|all]` | `POST /services/{name}/stop`, or `POST /services/all/stop` |
| `bz restart [service\|all]` | `POST /services/{name}/restart`, or `POST /services/all/restart` |
| `bz services` | `GET /services` |
| `bz status` | `GET /overview` |

With no argument, `start` / `stop` / `restart` act on `all`. A service that is not installed yet is installed first when you
start it (`POST /services/{name}/install` → job). Stopping `caddy` or `dns` prints a reminder that no local site resolves
or serves until they run again.

```console
$ bz status
BOZKURT CODE 1.0.0 · macos/aarch64 · ~/.bozkurt · tld .test · http :80 https :443
DNS ✔ configured   CA ✔ trusted
SERVICE       STATUS     PORT  VERSION  UPTIME
caddy         running    80    2.10.0   2h 11m
dns           running    5390  –        2h 11m
mariadb       external   3306  –        –        (port held by mysqld, pid 412)
php-fpm-8.3   running    –     8.3.14   2h 11m
RUNTIMES  php 8.3.14 · node 22.11.0 · python – · bun – · go –
SITES     4
```

### Runtimes

`<runtime>` is one of `php`, `node`, `python`, `bun`, `go`.

| Command | API |
|---|---|
| `bz <runtime> list [-a\|--available] [--refresh]` (alias `ls`) | `GET /runtimes/{kind}/versions[?refresh=true]`. Without `--available`, only installed versions are shown. |
| `bz <runtime> install <VERSION> [--allow-unverified]` | `POST /runtimes/{kind}/install {version[, allow_unverified: true]}` → job |
| `bz <runtime> use <VERSION>` | `POST /runtimes/{kind}/use {version}` |
| `bz <runtime> uninstall <VERSION>` (aliases `remove`, `rm`) | `DELETE /runtimes/{kind}/{version}` (asks to confirm) |

`VERSION` may be partial (`8.3`, `22`, `3.12`, `1.23`) or `lts` / `latest`, and resolves to the newest match. `use` installs the
version first if it is missing (and asks before doing so).

```sh
bz php list --available
bz node install lts
bz php use 8.4
bz python uninstall 3.11 -y
```

**`--allow-unverified`.** Some downloads have no published checksum: the static-php.dev PHP builds used on macOS and
Linux, and on Windows PHP, Go or cloudflared when the upstream index drops its hash. bozkurtd refuses them by
default and the install fails with `unverified download: <file> has no published checksum …`. Read the message,
then re-run with `--allow-unverified` to trust that file once: its sha256 is pinned in `state.json`
(`download_pins`), and every later download of the same file must match the pin or fail with `checksum_mismatch`.
A pinned file installs without the flag. To allow it for every download, set `allow_unverified = true` in
`config.toml`. `bz <runtime> use` installs a missing version without the flag, so for an unpinned PHP build run
`install --allow-unverified` first.

```sh
bz php install 8.4 --allow-unverified
bz runtime install php --allow-unverified
```

### Sites

| Command | API |
|---|---|
| `bz link [PATH] [--domain NAME] [--php VER] [--node VER] [--proxy PORT] [--https] [--wildcard] [--alias NAME]… [--kind php\|proxy\|static\|app] [--root DIR] [--command CMD]` | `POST /detect {path}`, then `POST /sites {...}` (or `PATCH /sites/{id}` if the directory is already linked) |
| `bz unlink [site]` | `DELETE /sites/{id}` (asks to confirm; the project files are never touched) |
| `bz sites` | `GET /sites` |
| `bz open [site]` | `GET /sites/{id}` → opens `http(s)://<domain>[:port]` in the default browser |
| `bz secure [site]` | `POST /sites/{id}/secure` |
| `bz unsecure [site]` | `POST /sites/{id}/unsecure` |
| `bz share [site]` | Asks to confirm, then `POST /share/{id}` → job → event `share.url`; prints the public URL and stays attached. Ctrl-C runs `DELETE /share/{id}`. |

Notes on `bz link`:

- `PATH` defaults to the current directory.
- `--domain shop` gives `shop.test`. A full name (`shop.test`) is also accepted.
- Anything you leave out is filled in by project detection and by `bozkurt.toml` in the project root.
- `--proxy PORT` implies `--kind proxy`. `--command` implies `--kind app`. `--alias` can be repeated.
- To change an existing site, run `bz link` again in the same directory with the options to change.

```console
$ cd ~/code/shop && bz link --https
Detected laravel (php 8.3 from composer.json, root public/)
✔ https://shop.test → ~/code/shop/public  (php-fpm-8.3)
```

#### `bz site …`

| Command | API |
|---|---|
| `bz site start\|stop\|restart [site]` | `POST /sites/{id}/start\|stop\|restart` (the dev command of `kind = app` sites) |
| `bz site info [site]` (aliases `show`, `get`) | `GET /sites` (resolve) → prints every field of the site |
| `bz site list` (alias `ls`) | `GET /sites` (same as `bz sites`) |
| `bz site rm [site]` (aliases `remove`, `unlink`) | `DELETE /sites/{id}` (same as `bz unlink`) |
| `bz site set <site> <key> <value>` | `PATCH /sites/{id} {key: value}`. `value` is parsed as JSON when possible (`true`, `8080`, `["a.test"]`), otherwise sent as a string. Example: `bz site set shop php 8.4`. |

### Logs

| Command | API |
|---|---|
| `bz logs [SOURCE] [-n\|--lines N] [-f\|--follow]` | `GET /services/{name}/logs?lines=N` or `GET /sites/{id}/logs?lines=N`. With `-f`, `bz` then streams `log` events from `GET /events` (WebSocket) filtered to that source. |

`SOURCE` is a service name, a site, or `all`. If you leave it out, `bz` uses the current directory's site, or `all` when
there isn't one. `-n` defaults to 200.

### Diagnostics, DNS and TLS

| Command | API |
|---|---|
| `bz doctor` | `GET /doctor`. Prints every check as ok/warn/fail with its `fix`. Exit code `1` if any check fails. |
| `bz doctor --fix <ID>` | `POST /doctor/fix/{id}` (asks to confirm; follows the job if one is returned). IDs come from the checks, e.g. `dns.configure`, `tls.trust`, `services.caddy.start`. |
| `bz dns setup [--tld TLD]` (alias `bz dns configure`) | `POST /dns/configure {tld?}` → job. Needs an admin prompt; every OS file it changes is backed up first. |
| `bz dns setup --undo` | `POST /dns/unconfigure` → job. Restores the OS resolver configuration from the backup. |
| `bz dns status` | `GET /dns` |
| `bz trust` | `POST /tls/trust {"system": true}` → job. Adds the local CA to your user trust stores and the system store (the OS may ask for your password). |
| `bz trust --undo` | `POST /tls/untrust` → job. Removes the CA from the stores it was added to. |
| `bz trust status` | `GET /tls` |

### Databases

| Command | API |
|---|---|
| `bz db list [--engine ENGINE]` (alias `ls`) | `GET /services/{engine}/databases` |
| `bz db create <NAME> [--engine ENGINE]` | `POST /services/{engine}/databases {name}` → `201 {name}` |
| `bz db drop <NAME> [--engine ENGINE]` (aliases `rm`, `delete`) | `DELETE /services/{engine}/databases/{name}` (asks to confirm) |

`--engine` is `mariadb` (default; `mysql` works too) or `postgres` (`postgresql`, `pg`). The daemon runs the engine's own
client over TCP `127.0.0.1:<port>`: MariaDB/MySQL as `root` with an empty password, PostgreSQL as `postgres`. This also works
for an `external` server; set other credentials in `config.toml` under `[databases.mariadb]` / `[databases.postgres]`
(`user`, `password`).

### Import from other tools

| Command | API |
|---|---|
| `bz import` | `GET /import/scan`. Lists the vhosts and DNS entries found (Valet, Herd, MAMP, XAMPP, Laragon, Homebrew) as a numbered list and asks which to import (numbers or ranges). After confirmation: `POST /import {vhosts, dns}` → job. Everything changed is backed up first. DNS entries are never imported: the job message lists them under `DNS not imported`, and a selection with only DNS entries is not sent (`bz` prints "DNS settings of other tools are not imported; remove their resolver file yourself if it conflicts."). |
| `bz import --all -y` | Imports everything found, without prompting. |

### Remote access (mobile companion)

| Command | API |
|---|---|
| `bz remote` | `GET /remote` |
| `bz remote enable` / `bz remote disable` | `POST /remote/enable` / `POST /remote/disable` |
| `bz remote pair` | `POST /remote/pairing-code` (+ `GET /remote`) → prints the 8-digit code and a terminal QR code of `bozkurt://pair?host=<ip:port>&code=<code>` (valid 5 min) |
| `bz remote devices` | `GET /remote` (`paired_devices`) |
| `bz remote revoke <DEVICE_ID>` | `DELETE /remote/devices/{id}` |

`POST /remote/pair` is called by the phone, not by `bz`.

### Configuration

| Command | API |
|---|---|
| `bz config get [KEY]` | `GET /config`; with a dotted `KEY` (`remote.port`) only that value. |
| `bz config set <KEY> <VALUE>` | `PATCH /config` with the dotted key expanded (`bz config set remote.port 7880` → `{"remote":{"port":7880}}`). `VALUE` is parsed as JSON when possible, otherwise sent as a string. |

The daemon validates the result: every port (`http_port`, `https_port`, `dns_port`, `api_port`, `remote.port`) must be in
1–65535 and different from the others (`invalid input: https_port (80) is already used by http_port`), and enabling or
moving the remote listener fails with `409 conflict` (nothing saved) when its port cannot be bound. `allow_unverified`
(default `false`) lets every download without a published checksum through and pins it; prefer the per-install
`--allow-unverified` flag.

### Jobs

| Command | API |
|---|---|
| `bz jobs` | `GET /jobs` |
| `bz jobs <ID>` | `GET /jobs/{id}` |

### Daemon

| Command | What it does |
|---|---|
| `bz daemon` / `bz daemon start` | Starts `bozkurtd` in the background if `GET /health` fails, waits up to 10 s, then prints its status. |
| `bz daemon stop` | `POST /daemon/shutdown` (graceful: services stop in order), waits up to 30 s. If that fails, stops the process that holds the API port (or the one in `bozkurtd.pid`). |
| `bz daemon status` | `GET /health` (+ `GET /ports/{api_port}` for the PID). Exit `3` if it is not running. |
| `bz daemon run` / `bz daemon --foreground` | Runs `bozkurtd` in the foreground (logs to the terminal). |

`bozkurtd` is looked up next to `bz`, then in `$BOZKURT_HOME/bin`, then on `PATH`.

### Integrations

| Command | API |
|---|---|
| `bz mcp register <CLIENT> [--project] [--print] [--command PATH]` | Local only. Writes the `bozkurt-mcp` stdio entry into the client's config file, after making a backup. `CLIENT`: `claude-code` (or `claude`), `cursor`, `vscode`, `windsurf`, `gemini`, `codex`. `--project` writes the project-level file in the current directory where the client supports one. `--print` only prints the snippet. `--command` sets the command to register; the default is `bozkurt-mcp` when it is on `PATH`, otherwise its absolute path (next to `bz` or in `$BOZKURT_HOME/bin`). See [MCP.md](MCP.md#client-setup). |
| `bz mcp unregister <CLIENT> [--project]` | Local only. Removes that entry. |
| `bz completions <SHELL>` | Local only. Prints a completion script for `bash`, `zsh`, `fish`, `powershell` or `elvish`. |

```sh
bz completions zsh > "${fpath[1]}/_bz"
bz completions powershell | Out-String | Invoke-Expression
```

### Alternative spellings

Hints printed by the core and the UI use these forms, so `bz` accepts them too:

| Command | Same as |
|---|---|
| `bz runtime install <KIND> [VERSION] [--allow-unverified]` | `bz <KIND> install <VERSION> [--allow-unverified]`; without `VERSION`: `lts` for node, `latest` otherwise. `bz runtime use\|uninstall\|list <KIND> …` work the same way. |
| `bz service install <NAME> [VERSION]` | `POST /services/{name}/install` → job (hidden from `--help`) |
| `bz service start\|stop\|restart [NAME]`, `bz service logs <NAME> [-n N] [-f]`, `bz service list` | `bz start\|stop\|restart`, `bz logs`, `bz services` (hidden) |
| `bz tls trust\|untrust\|status` | `bz trust`, `bz trust --undo`, `bz trust status` (hidden) |
| `bz dns unconfigure` | `bz dns setup --undo` (hidden) |

## Exit codes

| Code | Meaning |
|---|---|
| `0` | Success |
| `1` | Generic failure (API returned an error not listed below, `bz doctor` found a `fail`) |
| `2` | Usage error (unknown command or flag, invalid value) |
| `3` | Daemon unreachable (or `--no-start` and not running; `bz daemon status` when stopped) |
| `4` | Not found (unknown site, service, runtime version, config key) — API `404` |
| `5` | Conflict (port in use, domain already linked, version in use by a site) — API `409` |
| `6` | Confirmation required and not given (`--json` without `--yes`, or the user answered "no"), or elevation/permission was denied |
| `7` | A job finished with `job.failed` |
| `130` | Interrupted (Ctrl-C) while waiting in the foreground |

With `--json`, the error body on stderr always carries the API's `error.code` string, so scripts can branch on it. A
missing or wrong local API token is reported as `401` with exit code `1`; this only happens when `--home` points at a
different home than the running daemon's.
