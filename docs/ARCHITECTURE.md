# BOZKURT CODE — Architecture

Open-source (MIT OR Apache-2.0), cross-platform local development environment. This document describes the design of
release 1.0; the HTTP interface is specified in [API.md](API.md).

## Product goals
1. Self-contained: everything lives under the BOZKURT home (`~/.bozkurt`, or `%LOCALAPPDATA%\Bozkurt` on Windows,
   overridable by `BOZKURT_HOME`). No Homebrew/apt/choco needed. Uninstall = remove app + home dir.
2. macOS (arm64+x64), Linux (x64+arm64), Windows (x64) desktop; iOS/Android companion (remote control over LAN).
3. One engine, four faces: desktop GUI (Tauri), `bz` CLI, `bozkurt-mcp` MCP server, mobile companion.
   All four talk to the same daemon `bozkurtd` over HTTP — GUI/CLI/MCP have feature parity.
4. Developer-first UX: zero-config detection of projects (Laravel, WordPress, Symfony, Next.js, Vite, Nuxt,
   Django, Flask, FastAPI, Go, static), per-site runtime versions, one-click HTTPS, logs, command palette.

## Feature set
- Runtimes: PHP 8.0+ (static builds from static-php.dev on mac/linux, windows.php.net zips on Windows),
  Node.js (all LTS + current, nodejs.org), Bun, Python (python-build-standalone), Go (go.dev).
  Version list discovered live from upstream indexes. Switch = atomic symlink (junction on Windows) + shims in `bin/`.
  Per-site version pinning; auto-detect from composer.json `require.php`, `.nvmrc`, `.node-version`,
  `package.json engines.node`, `.python-version`, `go.mod`.
- Web server: bundled **Caddy** (single static binary, Apache-2.0) as the front proxy for every OS.
  Sites: `php` (php_fastcgi → per-version PHP-FPM pool; on Windows php-cgi pool), `proxy` (reverse proxy to a port),
  `static`, `app` (BOZKURT runs the dev command, e.g. `npm run dev`, assigns PORT, proxies to it).
  `app` and `proxy` sites are proxied to `localhost:<port>` (Caddy dials every address `localhost` resolves to, so a
  dev server bound to `::1` only still works). For app sites the daemon sets `__VITE_ADDITIONAL_SERVER_ALLOWED_HOSTS`
  to `.<domain>` plus `.<alias>` for every alias (a value from the site env is kept and extended), so Vite accepts the
  site's Host header; newly detected Vite, SvelteKit and Astro commands get `--host 127.0.0.1` (existing sites keep
  their stored command).
  Windows PHP pool: per PHP minor, 4 `php-cgi -b 127.0.0.1:<port>` processes (`php-fpm-<mm>`, `php-fpm-<mm>#1` … `#3`)
  behind `php_fastcgi … { lb_policy least_conn }`. The first keeps `php_cgi_port` (`8.3` → 9083); the others use
  `15000 + 8 × (major×100 + minor) + 1..3` (`8.3` → 21425–21427); a minor ≥ 10 uses slot 0 of that block for the first
  port too. `/services` still lists the pool as one `php-fpm-<mm>` entry (instance 0's status and pid). macOS/Linux
  run one php-fpm per version.
  Wildcard subdomains, HTTP→HTTPS redirect, per-site custom Caddy snippet. `.htaccess` is NOT parsed;
  framework presets (WordPress, Laravel, Symfony, generic front-controller) generate the equivalent rules.
- Databases & services: MariaDB (official tarballs) or detect existing MySQL/MariaDB on :3306 and use as-is,
  PostgreSQL (embedded binaries), Redis/Valkey, Mailpit (SMTP catcher + web UI). Each service: install,
  start, stop, restart, status, logs, port, autostart.
- Local DNS: in-process UDP/TCP responder on 127.0.0.1:`dns_port` (default 5390) answering `*.<tld>` (default `test`) with A 127.0.0.1
  and AAAA ::1. OS routing: macOS `/etc/resolver/<tld>`; Linux systemd-resolved drop-in or NetworkManager dnsmasq,
  fallback /etc/hosts management; Windows NRPT rule (`Add-DnsClientNrptRule`) with hosts-file fallback. NRPT needs the
  responder on 127.0.0.1:53, so it is chosen only when UDP **and** TCP 53 are free (or ours); otherwise, or when the
  bind fails before the elevation prompt, the hosts-file plan is used with a note naming the process holding port 53.
  Every OS change outside the home is a named `bz-net` plan (resolver files, NRPT rule, hosts block, system trust
  store). The caller shows the plan and asks first; each block that needs admin rights triggers one OS prompt, and
  the files it touches are backed up. There is no separate privileged helper process.
- TLS: own local CA generated with rcgen (no external mkcert). Trust per OS without admin where possible:
  macOS login keychain (`security add-trusted-cert -k login.keychain`), Windows CurrentUser\Root
  (`certutil -user -addstore Root`), Linux user NSS db (`certutil -d sql:$HOME/.pki/nssdb`) + optional system store.
  Per-site leaf certs (SAN: domain + *.domain), auto-renew before expiry. A CA created by 1.0 is name-constrained
  to the DNS subtrees `test` and `localhost`; a site on another TLD regenerates it with that TLD added (old leaves
  deleted, CA must be trusted again). A CA from before 1.0 has no constraints and is kept.
- Process supervisor: owns every child process, restarts on crash with backoff, captures stdout/stderr to
  ring buffer + rotating log files, exposes live log streaming. Output is decoded lossily (invalid UTF-8 never stops
  the reader; lines are capped at 64 KiB). Children are tied to the daemon:
  - every supervised process is recorded (pid, process group, start time, program) in `$BOZKURT_HOME/run/children.json`;
    at startup, right after the API port is bound and before autostart, the daemon kills leftovers whose pid **and**
    start time still match (`reaped orphaned processes: …` in the log);
  - Windows: each child runs in its own kill-on-close Job Object held only by the daemon, so a daemon crash ends the
    tree. Stop uses the service's own shutdown first (Caddy admin `/stop`, `mariadb-admin shutdown`,
    `pg_ctl stop -m fast`, `valkey-cli shutdown`); anything without such a hook (php-cgi, Mailpit, app dev servers), or
    whose hook fails or times out, is killed at once with its job;
  - Linux: children get `PR_SET_PDEATHSIG` (SIGTERM); macOS and Linux stop with SIGTERM to the process group, then SIGKILL
    after the timeout. Limits are listed in [ROADMAP](ROADMAP.md#known-limitations-in-10).
- Ports: detect who holds a port (PID, process name) cross-platform and report it; a busy port is shown as a conflict
  (or the service as `external`), and the fallback ports 8080/8443 are surfaced in every URL.
- Migration/import: detect Homebrew/XAMPP/MAMP/Valet/Herd/Laragon vhosts & DNS, import vhosts with confirmation and
  backups. DNS entries are only reported (`; DNS not imported: …`), never applied; a DNS-only import is refused (400).
- Share: optional public tunnel via cloudflared (downloaded on demand) — `bz share mysite.test`.
- Doctor: `bz doctor` diagnoses ports, DNS resolution, trust, PATH, permissions with fix suggestions.
- Remote/mobile: daemon can enable a LAN listener (off by default) secured with a pairing code + bearer token,
  advertised via mDNS `_bozkurt._tcp`. Mobile app = same React UI in remote mode.
- Local API protection: the local listener only answers `Host: 127.0.0.1|localhost|[::1]:<api_port>`, rejects
  browser `Origin`s other than the packaged app's, refuses non-JSON writes, and requires the per-install token in
  `$BOZKURT_HOME/api-token` from every other client (see [SECURITY.md](../SECURITY.md#local-api-protection)).
- Network exposure: Caddy binds the sites it serves to `127.0.0.1`/`::1` (`default_bind`) unless `lan_access = true`
  (changing it reloads Caddy).
- Caddy admin API: never on `:2019`. Unix: `admin unix//$BOZKURT_HOME/caddy/run/admin.sock` (`caddy/run/` is 0700, a
  stale socket is removed before start); the daemon talks to it over the socket. Windows, or a socket path of 100
  bytes or more: a free loopback port chosen at each Caddy start. Config is pushed with `POST /load` on that endpoint
  (fallback `caddy reload --address <same endpoint>`).

## Repository layout
```
Cargo.toml                 cargo workspace
crates/bz-core             library: paths, config, state, catalogs, installers, runtimes, services, sites,
                           supervisor, project detection, Caddyfile generation, events
crates/bz-net              library: dns responder, CA/certs, trust stores, OS resolver setup, port inspection
crates/bz-daemon           bin `bozkurtd`: axum HTTP API (docs/API.md) + WebSocket events
crates/bz-cli              bin `bz`: CLI over the HTTP API (starts daemon if not running)
crates/bz-mcp              bin `bozkurt-mcp`: stdio MCP server over the HTTP API
apps/desktop               Tauri 2 shell + React/TS/Vite UI (tray menu, bundles bozkurtd as sidecar)
apps/mobile                Tauri 2 mobile shell (iOS/Android) reusing packages/ui in remote mode
packages/ui                shared React UI + typed API client (TypeScript)
docs/                      ARCHITECTURE, API, CLI, MCP, USER GUIDE (EN + TR), COMPARISON, ROADMAP
scripts/                   install.sh / install.ps1, release and repo-lint helpers
.github/workflows          ci.yml (fmt, clippy, tests on 3 OS, repo lint), release.yml (CLI archives, desktop
                           bundles via tauri-action, SHA256SUMS), mobile.yml (Android/iOS, attached to the release)
```

## Home directory layout
```
$BOZKURT_HOME/
  bin/                runtime shims (php, composer, node, npm, npx, corepack, bun, bunx, python, python3, pip, pip3,
                      go, gofmt); the CLI installer also puts bz, bozkurtd and bozkurt-mcp here
  runtimes/php/<ver>/ runtimes/node/<ver>/ runtimes/python/<ver>/ runtimes/bun/<ver>/ runtimes/go/<ver>/
  current/<runtime>   symlink/junction → active global version
  services/<name>/    binaries + data dirs (mariadb/data, postgres/data, redis, mailpit, cloudflared)
  caddy/Caddyfile     generated
  caddy/bin/          caddy binary
  caddy/data/         Caddy's own storage
  caddy/snippets/     optional per-site snippets (<domain>.caddy)
  caddy/run/          (0700) admin.sock, Caddy's admin API socket (macOS/Linux)
  ca/                 rootCA.pem, rootCA-key.pem (0600)
  certs/<domain>.pem, <domain>-key.pem
  logs/<component>.log
  run/children.json   supervised processes (pid, pgid, start time, program), used to reap orphans at startup
  cache/              upstream catalog cache (6 h)
  tmp/                download and extraction staging (same filesystem → atomic rename)
  config.toml         user config (tld, ports, autostart, remote, lan_access, allow_unverified)
  state.json          sites, installed versions, service states, download_pins (written atomically)
  api-token           per-install local API token (0600), created by bozkurtd
  bozkurtd.pid        single-instance guard of the running daemon
  backups/<stamp>/    anything changed outside the home dir is backed up here first
```

## Config (`config.toml`) defaults
```toml
tld = "test"
dns_port = 5390
http_port = 80          # falls back to 8080 if unprivileged and 80 is unavailable
https_port = 443        # falls back to 8443
api_port = 7878         # daemon API, 127.0.0.1 only
autostart = ["caddy", "dns"]
lan_access = false      # true: Caddy serves sites on every interface, not only 127.0.0.1/::1
allow_unverified = false  # true: accept every download without a published checksum (pinned on first use)
[remote]
enabled = false
port = 7879
```

The five ports (`http_port`, `https_port`, `dns_port`, `api_port`, `remote.port`) must be in 1–65535 and pairwise
distinct; a `config.toml` that breaks this stops `bozkurtd` from starting, and `PATCH /config` refuses it with 400.

## Per-project file `bozkurt.toml` (optional, committed to the repo)
```toml
domain = "shop"           # → shop.test
type = "php"              # php | proxy | static | app
root = "public"
php = "8.3"
node = "22"
command = "npm run dev"   # for type=app
aliases = ["admin.shop"]
services = ["mariadb", "redis", "mailpit"]
[env]
APP_ENV = "local"
```

## Core rules
- Every download goes over HTTPS to `tmp/`, is checked against the upstream SHA-256 when the source publishes one, and
  is extracted and then atomically renamed into place. Checksum sources: nodejs.org `SHASUMS256.txt`, the windows.php.net
  and go.dev indexes, the MariaDB release API, Valkey `.sha256` files, Composer's `.sha256`, and for GitHub-hosted
  downloads (Caddy, Bun, python-build-standalone, PostgreSQL, Mailpit, cloudflared) the asset digest or the release's
  checksums file. **Downloads without a published checksum fail closed** (`unverified_download`): static-php.dev
  publishes none, so the macOS/Linux PHP builds (and a GitHub asset with neither a digest nor a checksums file, or a
  Windows PHP / Go / cloudflared asset whose index drops its hash) are refused unless the install passes
  `allow_unverified` or the config key `allow_unverified = true` is set. The first allowed download is pinned (trust on
  first use): `state.json` `download_pins` maps the URL to its sha256, later downloads must match it
  (`checksum_mismatch` otherwise, even when allowed), and a pinned URL needs no opt-in again. An upstream checksum
  always takes precedence over a pin.
- Every file written outside $BOZKURT_HOME is first backed up to `backups/<stamp>/` and listed in the action log.
- Elevation only through named `bz-net` plans, each confirmed by the caller (GUI dialog / CLI prompt / MCP `confirm`).
- MCP never runs a shell command the user has not seen: setting a site's `command`/`env`, or starting an app site
  with an unconfirmed command, returns a preview unless `confirm: true` (`BOZKURT_MCP_ALLOW_COMMANDS=1` opts out).
- All long operations are jobs with progress events (`job.progress`, `job.done`, `job.failed`) on the event bus.
