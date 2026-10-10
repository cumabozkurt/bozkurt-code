# Changelog

All notable changes to this project are documented here.
The format follows [Keep a Changelog 1.1](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Within 1.x, these interfaces only change in backwards-compatible ways (additions, new optional fields): `bz` commands,
flags and exit codes; the HTTP API v1 (`/api/v1`); MCP tool names, tiers and arguments; the `config.toml` and
`bozkurt.toml` keys; the `$BOZKURT_HOME` layout. Breaking changes wait for 2.0 and are announced here first.

## [1.0.0] - 2026-10-09

The first public release. There was no earlier tagged version, so everything built before it is listed here.

### Highlights

- **`bozkurtd` daemon.** axum HTTP API v1 on `127.0.0.1:7878` (see `docs/API.md`), WebSocket `/events`,
  service supervisor, background jobs, remote pairing for the mobile companion, `bz share` tunnels via cloudflared,
  import from other local tools, database management on MariaDB/PostgreSQL, doctor checks with one-click fixes, and a
  graceful `POST /daemon/shutdown`.
- **`bz` CLI.** Every daemon feature from the terminal, with `--json` output, documented exit codes, daemon autostart,
  shell completions and `bz mcp register` for Claude Code, Cursor, VS Code, Windsurf, Gemini CLI and Codex.
- **`bozkurt-mcp`.** stdio MCP server with 55 tools in four safety tiers; destructive and system tools return a preview
  until called with `confirm: true`.
- **Desktop app and mobile companion** on the shared React UI, with English and Turkish translations.

### Added

- **Workspace and licensing.** Cargo workspace (`bz-core`, `bz-net`, `bz-daemon`, `bz-cli`, `bz-mcp`, desktop and mobile
  Tauri crates) and npm workspace (`@bozkurt/ui`, `@bozkurt/desktop`, `@bozkurt/mobile`). Dual license MIT OR Apache-2.0.
- **Reference docs.** `docs/ARCHITECTURE.md` (goals, home layout, config, `bozkurt.toml`, core rules) and
  `docs/API.md` (HTTP API v1, types and events).
- **bz-core: paths and state.** `$BOZKURT_HOME` layout with `BOZKURT_HOME` override, `config.toml` with validation
  (rejects HSTS-preloaded `.dev`/`.app`) and JSON merge-patch, per-project `bozkurt.toml`, and atomic `state.json`.
- **bz-core: upstream catalogs** with a 6 h disk cache:
  - PHP: static-php.dev on macOS/Linux, windows.php.net on Windows.
  - Node.js (nodejs.org index, SHASUMS256 and release schedule), Bun (GitHub), Python (python-build-standalone) and
    Go (go.dev).
  - Composer (getcomposer.org).
  - EOL tagging for PHP and Python.
- **bz-core: runtimes.** Install, uninstall and `use` with an atomic `current/` link, shims in `bin/` (POSIX sh and Windows
  cmd), and per-directory version resolution from project files.
- **bz-core: services.** Catalogs, process specs and init:
  - Caddy (GitHub releases).
  - MariaDB: Linux x64 bintar and Windows x64 zip.
  - PostgreSQL (theseus-rs binaries).
  - Valkey as `redis`: Linux only.
  - Mailpit (SMTP 1025, UI 8025).
  - PHP-FPM pools, or `php-cgi -b` on Windows.
  - `external` status when a foreign process holds the port.
  - Aliases: `mysql`→`mariadb`, `valkey`→`redis`, `postgresql`→`postgres`.
- **bz-core: supervisor.** Supervisor with restart-on-crash backoff, ring-buffer and file logs, a job system with
  progress events, and an event bus.
- **bz-core: project detection.**
  - PHP: Laravel, Symfony, WordPress (incl. Bedrock), Drupal.
  - JavaScript: Next.js, Nuxt, Astro, SvelteKit, Remix/React Router, Vite, Express/Fastify/Koa. The package manager
    (npm, pnpm, yarn, bun) is detected too.
  - Python: Django, FastAPI, Flask.
  - Go, generic PHP and static sites.
- **bz-core: Caddyfile generation.** php, proxy, static and app sites; wildcard subdomains and aliases; HTTPS redirect;
  per-site snippets. Snapshot-tested.
- **bz-net: DNS responder.** UDP and TCP responder for `*.<tld>` → `127.0.0.1` / `::1`, with live TLD changes.
- **bz-net: OS resolver plans.**
  - macOS: `/etc/resolver`.
  - Linux: systemd-resolved, then NetworkManager dnsmasq, then hosts.
  - Windows: NRPT (needs port 53), then hosts.
- **bz-net: local CA.** rcgen, ECDSA P-256, 10 years, key 0600. Leaf certificates last 825 days and are renewed within
  30 days of expiry, or on a SAN or CA change.
- **bz-net: trust plans.** macOS login keychain, Windows `CurrentUser\Root`, Linux NSS databases (Chrome and Firefox
  profiles), plus an optional system store.
- **bz-net: ports, hosts and elevation.** Port-owner lookup (PID and process name), a managed hosts-file block, and an
  elevation layer. Each elevated change is a named plan with backups and one prompt per admin block.
- **@bozkurt/ui: shared React UI.**
  - Screens: Overview with Quick setup, Sites, Runtimes, Services, Logs, Doctor, Settings with the remote-pairing QR
    code, and Connect for mobile.
  - Jobs drawer, command palette (⌘K / Ctrl K), keyboard shortcuts and toasts.
  - Light, dark and system themes; complete English and Turkish translations.
  - Typed API client with WebSocket reconnect, and an in-memory mock backend.
- **Desktop app (Tauri 2).**
  - Tray/menu-bar menu: status, Switch PHP/Node, start/stop all, palette, launch at login.
  - Starts the `bozkurtd` sidecar; `bozkurt://` deep links; single instance; open in editor or terminal.
  - Bundles: dmg, msi/nsis, deb/rpm/AppImage.
- **Mobile companion (Tauri 2, iOS and Android).** Pairing by QR code (`bozkurt://pair?host=…&code=…`) or manual entry,
  bearer token, bottom tab bar, demo mode.
- **Install scripts.** `scripts/install.sh` (macOS/Linux) and `scripts/install.ps1` (Windows) install the CLI binaries
  into `$BOZKURT_HOME/bin` with SHA-256 verification against `SHA256SUMS`, without root.
- **CI.** rustfmt; clippy and tests (`--locked`) on Linux, macOS and Windows; Node typecheck and build; desktop `cargo check`
  with the sidecar overlay; mobile shell check; a repo-lint job (actionlint, shellcheck, PSScriptAnalyzer, JSON, Markdown
  links, version drift).
- **Release workflow.** Draft release, CLI archives for 5 targets, Tauri bundles, `SHA256SUMS`, and optional
  macOS/Windows signing and notarization.
- **Mobile workflow.** Android APK/AAB and iOS builds, signed when secrets are present.
- **Documentation.** README (EN/TR), user guide (EN/TR), CLI and MCP references, comparison, roadmap, CONTRIBUTING,
  SECURITY and Code of Conduct.
- **Databases.** `GET|POST /services/{engine}/databases`, `DELETE /services/{engine}/databases/{name}`; `bz db
  list|create|drop` and the `db_*` MCP tools.
- **Undo for OS changes.** `POST /dns/unconfigure` (`bz dns setup --undo`) and `POST /tls/untrust` (`bz trust --undo`).

### Security

- **Local API token.** `bozkurtd` creates a per-install token in `$BOZKURT_HOME/api-token` (mode 0600). Clients without
  an allow-listed browser `Origin` must send it as `Authorization: Bearer <token>` (or `X-Bozkurt-Token`); `bz` and
  `bozkurt-mcp` read it automatically. Missing or wrong token → `401`.
- **Origin and Host guard.** Browser requests are accepted only from the packaged app's origins (the dev-server origins
  only with `BOZKURT_DEV=1` or in debug builds) → otherwise `403`. Mutating requests with a non-JSON `Content-Type` → `415`.
  A `Host` other than `127.0.0.1`/`localhost`/`[::1]` on the API port → `421` (DNS rebinding).
- **Caddyfile hardening.** Sites bind to `127.0.0.1` and `::1` only unless `lan_access = true`; every value written to the
  Caddyfile is quoted so site fields cannot inject directives or blocks, and invalid site fields are rejected.
- **Downloads fail closed without a checksum.** A download whose upstream publishes no checksum (static-php.dev PHP on
  macOS/Linux; Windows PHP, Go or cloudflared when the index drops its hash) fails with `unverified_download` (422)
  unless allowed once (`"allow_unverified": true` on `POST /runtimes/{kind}/install`, `bz … install
  --allow-unverified`, "Install anyway" in the app) or always (new config key `allow_unverified`, default `false`). The
  first allowed download is pinned by sha256 in `state.json` (`download_pins`); a later download that differs fails with
  `checksum_mismatch`. MCP has no bypass.
- **Pairing brute-force limits.** Pairing codes are 8 digits (were 6). On top of 5 attempts per address per minute, all
  addresses together get 30 per minute (`429 rate_limited`), and a code is revoked after 10 wrong guesses
  (`429 pairing_locked`) until a new one is generated. The mobile app accepts 8-digit codes.
- **Name-constrained local CA.** New CAs can only sign for `test` and `localhost` (plus a configured TLD: changing the TLD
  regenerates the CA, re-issues site certificates, and the CA must be trusted again). CAs created before 1.0.0 are kept.
- **Private Caddy admin API.** Caddy's admin API moved off `127.0.0.1:2019` to the unix socket
  `$BOZKURT_HOME/caddy/run/admin.sock` (directory 0700), or a random loopback port on Windows.
- **MCP confirmation for shell commands.** `sites_create` / `sites_update` that set a site `command` or `env`, and
  `sites_start` / `sites_restart` of an app site with an unconfirmed command, return a preview unless `confirm: true`;
  `BOZKURT_MCP_ALLOW_COMMANDS=1` opts out.
- **Config validation.** `http_port`, `https_port`, `dns_port`, `api_port` and `remote.port` must be 1–65535 and distinct
  (`400 invalid_input`; an invalid `config.toml` stops the daemon from starting). A paired device cannot change `api_port`
  or `remote.port` (`403`). Enabling remote access binds the listener before saving, so a busy port answers `409` and
  leaves `remote.enabled` off.
- **Release integrity.** `SHA256SUMS` for every artifact, GitHub build-provenance attestations, third-party action pins by
  commit SHA, and `THIRD_PARTY_LICENSES.md` shipped in archives and app bundles.

### Changed

- **Release pipeline.** Desktop bundles now contain the `bozkurtd`, `bz` and `bozkurt-mcp` sidecars (checked after each
  build); signed updater artifacts (`.sig`) and `latest.json` are published with the release, and signed desktop
  builds register `tauri-plugin-updater` and check that `latest.json` at startup (the result is sent to the UI as the
  `bozkurt://update-available` event; installing from the app comes in 1.1); a tag that does not match
  the workspace version fails the release. Android release builds allow cleartext HTTP to the LAN daemon; APK/AAB/IPA
  files are attached to the GitHub release.
- **App identifiers** are `tr.cumabozkurt.bozkurtcode` (desktop) and `tr.cumabozkurt.bozkurtcode.mobile` (mobile).
- **Versions** are 1.0.0 across Cargo, npm and Tauri; minimum Rust is 1.89 for the
  CLI, daemon and MCP crates and 1.90 for the Tauri apps (Tauri 2.12 needs it).

### Fixed

- **UI.** Results from API calls are applied immediately; offline and error states on every screen; load errors shown
  instead of empty lists; trust and DNS status reflect what the daemon verified; port-aware site URLs; port validation;
  share confirmation; PHP version warnings; StrictMode-safe event stream; WCAG AA colour contrast; Turkish wording.
- **Mobile.** The mobile Tauri shell compiles again (`serde_json` for `generate_context!`).
- **Caddy autostart.** Installing Caddy while it is in `autostart` (the default) now starts it right away ("Installed and
  started caddy X").
- **`lan_access`** changes reload Caddy immediately (Settings → Network has a switch for it).
- **Caddy reloads over the admin socket** no longer get `403` from Caddy (no `Origin` header on the unix socket), so
  config changes are applied through the admin API instead of falling back to `caddy reload`.
- **Process supervision.** Two concurrent starts of the same process no longer spawn it twice; a child printing invalid
  UTF-8 or very long lines no longer stalls (output is decoded lossily, lines capped at 64 KiB). Children are tied to
  the daemon: Windows Job Objects (kill on close), Linux parent-death signal, and a record in
  `$BOZKURT_HOME/run/children.json` from which leftovers of a crashed daemon are reaped at the next start.
- **Windows stop.** Caddy, MariaDB, PostgreSQL and Valkey stop through their own shutdown command instead of waiting for
  the full timeout; other processes are ended at once.
- **Windows PHP.** Each PHP version runs 4 `php-cgi` processes behind Caddy's `least_conn` balancer, so requests no
  longer queue behind one process and pages that call their own site no longer deadlock. PHP minors ≥ 10 no longer
  collide with the next major's port.
- **Windows shims and commands.** `!` and `^` in arguments survive the `php`, `composer`, `node`, `bun`, `python`, `pip`
  and `go` shims; app commands reach `cmd.exe` verbatim (`/d /s /c`); `bz <runtime> use` switches with a native,
  near-atomic junction instead of `cmd mklink`.
- **Windows ports.** Port owners are detected on non-English Windows too (`GetExtendedTcpTable` instead of parsing
  localized `netstat` output).
- **Windows DNS.** When port 53 is taken, DNS setup falls back to the hosts file and names the program holding the port,
  instead of failing.
- **Vite / SvelteKit / Astro sites** behind Caddy: no more "host not allowed" (the site's domains are added to Vite's
  allowed hosts) or 502 from a dev server listening on `::1` only (Caddy proxies to `localhost:<port>`; new sites run with
  `--host 127.0.0.1`).
- **Runtime installs.** Installing the same runtime version twice at once no longer fails; the second install waits.
  Removing a version while it is being installed answers `409`.
- **Import.** `POST /import` no longer drops DNS entries silently: a DNS-only import answers `400`, and the job message
  lists the entries that were not imported (`; DNS not imported: …`). `bz import` explains this instead of sending a
  DNS-only selection.

[1.0.0]: https://github.com/cumabozkurt/bozkurt-code/releases/tag/v1.0.0
