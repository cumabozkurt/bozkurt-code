# Roadmap

This is a living document. Dates are intentions, not promises. Ideas and votes go in
[Discussions](https://github.com/cumabozkurt/bozkurt-code/discussions).

Legend: ✅ done in the repository · 🚧 in progress · ⏳ planned · 💡 idea, under discussion

## 1.0.0: first public release (2026-10-09)

**Goal:** a daily driver for PHP and JavaScript projects on macOS, Linux and Windows, usable from the desktop app, the CLI,
an MCP client and a phone.

| Area | Item | State |
|---|---|---|
| Reference docs | `ARCHITECTURE.md`, `API.md` v1, `CLI.md`, `MCP.md` | ✅ |
| Core (`bz-core`) | Paths, config, `bozkurt.toml`, state, events, jobs | ✅ |
| | Live catalogs: PHP (static-php.dev / windows.php.net), Node, Bun, Python, Go, Composer | ✅ |
| | Runtime install / use / uninstall, shims, per-project version detection | ✅ |
| | Service catalogs and process specs: Caddy, MariaDB, PostgreSQL, Valkey, Mailpit, PHP-FPM / php-cgi | ✅ |
| | Supervisor with restart backoff and log capture | ✅ |
| | Project detection (PHP, JS, Python, Go, static) and Caddyfile generation | ✅ |
| Net (`bz-net`) | DNS responder, OS resolver plans, local CA and leaf certs, trust plans, ports, hosts, elevation | ✅ |
| UI | Shared React UI (EN/TR, themes, palette, mock backend) | ✅ |
| | Desktop shell (tray, sidecars, deep links) | ✅ |
| | Mobile shell (QR pairing, remote mode) | ✅ |
| Daemon | `bozkurtd`: axum HTTP API v1 + `/events` WebSocket; local API token, Origin/Host checks; remote listener with pairing and bearer tokens; mDNS `_bozkurt._tcp` | ✅ |
| CLI | `bz`: every command in [CLI.md](CLI.md), `--json`, exit codes, completions, `mcp register` | ✅ |
| MCP | `bozkurt-mcp`: the 55 tools in [MCP.md](MCP.md) with confirm/preview tiers | ✅ |
| Databases | List, create and drop on MariaDB/PostgreSQL (API, CLI, MCP) | ✅ |
| Import | Scan and import from Valet, Herd, MAMP, XAMPP, Laragon, Homebrew | ✅ |
| Share | cloudflared download on demand, `share.url` event | ✅ |
| Distribution | Release workflow (5 desktop targets with sidecars + CLI archives + `SHA256SUMS` + provenance + updater artifacts), install scripts, mobile builds attached to the release | ✅ |
| Docs | README (EN/TR), user guide (EN/TR), CLI, MCP, API, comparison, contributing, security | ✅ |

### Known limitations in 1.0

- Bundled MariaDB covers Linux x64 and Windows x64 only; bundled Redis (Valkey) covers Linux only. Elsewhere an existing
  server on the default port is used as `external`.
- Windows on ARM runs the x64 build under emulation. Linux builds need glibc ≥ 2.35 (no musl/Alpine).
- Desktop-only installs keep `bz` and `bozkurt-mcp` inside the app bundle; to use them from a terminal or an MCP client,
  install the CLI with the install script as well.
- The Windows and macOS process, DNS and shim code was compile-checked and unit-tested but not yet run on real Windows
  or macOS hosts for this release.
- Windows: `npm`, `npx` and `corepack` are `.cmd` files, so a `^` inside a quoted argument arrives doubled. A native
  `bz-shim.exe` will remove this.
- Windows: when port 53 is taken, DNS falls back to the hosts file (no wildcard subdomains). A dedicated loopback
  responder on `127.53.0.1:53` is planned. VPN clients and WSL2 can bypass the NRPT rule.
- Process cleanup: on Windows a grandchild started in the instant before its parent joins the Job Object can escape;
  on Linux grandchildren get no parent-death signal; macOS has no death signal at all. In these cases leftovers are
  reaped only at the next daemon start (on macOS only while the recorded leader is still alive).
- Windows: MariaDB with a root password you set yourself cannot use the graceful shutdown and is force-stopped.
- Windows PHP: the 4 `php-cgi` processes are not recycled after N requests, and the pool size is fixed at 4.
- The remote (mobile) listener uses plain HTTP on the LAN; TLS with a certificate pinned through the pairing QR code is
  planned.
- Downloads without a published checksum (PHP from static-php.dev) are protected by trust-on-first-use pinning only; a
  signed manifest or our own PHP builds are planned.

## 1.1: broader coverage

- ⏳ **In-app updates** in the desktop app: an update prompt with **Install and restart**. Since 1.0, signed builds
  already check the signed `latest.json` at startup (`tauri-plugin-updater`) and emit `bozkurt://update-available`.
- ⏳ **"Install command-line tools"** action in the desktop app (links the bundled `bz` and `bozkurt-mcp` into
  `$BOZKURT_HOME/bin`).
- ⏳ **MariaDB on macOS** (arm64 and x64) and **Linux arm64**. Today the bundled MariaDB covers Linux x64 and Windows x64 only.
- ⏳ **Redis/Valkey on macOS and Windows.** Today it covers Linux only. Until then, an existing server on :6379 is used as `external`.
- ⏳ **Windows on ARM** native builds. The installer currently falls back to x64 emulation.
- ⏳ `db` management in the GUI: create/drop databases, credentials, and an "open in client" action.
- ⏳ Per-site PHP extensions and `php.ini` overrides from the UI and `bozkurt.toml`.
- ⏳ Xdebug toggle per site.
- ⏳ "Park" a folder: every subfolder becomes a site automatically.
- 💡 `bz share` providers beyond cloudflared.

## Later

- 💡 More runtimes: Ruby, Java, Deno, Rust toolchains.
- 💡 More services: MySQL (Oracle builds), MongoDB, Meilisearch, MinIO, Memcached.
- 💡 Snapshots of site files and database.
- 💡 More UI and documentation languages. The i18n is type-checked, so adding a language means adding one file.
- 💡 Team presets: share a `bozkurt.toml` plus required services across a team.
- 💡 LAN preview from the UI: a toggle for `lan_access` (sites on every interface) plus DNS for other devices.

## Non-goals

- Running dev servers **on** the phone. iOS and Android do not allow it, so the mobile app is a remote control.
- Containers and VMs. Use Docker if you need production-identical images. BOZKURT CODE is about fast native binaries.
- Production hosting or deployment.
- Parsing Apache `.htaccess`. Framework presets and per-site Caddy snippets cover these needs instead.
