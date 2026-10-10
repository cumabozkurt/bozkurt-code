# Release fix log (1.0.0)

Fix pass over [`RELEASE_AUDIT.md`](RELEASE_AUDIT.md) (CI/CD, packaging, docs). Rust-side fixes are tracked in
[`RUST_FIX_LOG.md`](RUST_FIX_LOG.md). One row per finding; `git show <commit>` for the change.

Status: **fixed**, **partial** (what is left is stated), **left** (not done, with the reason / owner).

## P0

| Finding | Status | Commit(s) | Notes |
|---|---|---|---|
| P0-1 Desktop bundles lack the sidecars | fixed | `6e53e19` | `tauri.sidecar.conf.json` lists `bozkurtd`, `bz`, `bozkurt-mcp`; `release.yml` passes the overlay; post-build sidecar assertion. README claim kept (`13325eb`). |
| P0-2 Mobile release APK cannot reach the LAN daemon | fixed | `6e53e19` | `mobile.yml` runs `patch-android.mjs`, which now fails when nothing was patched. |
| P0-3 Local API reachable from any web page | fixed in code by the Rust pass; documented here | see `RUST_FIX_LOG.md`; docs `696d81f`, `b7eff02`, `0c449e4` | Token (`$BOZKURT_HOME/api-token`), Origin/Host/Content-Type guard. SECURITY.md "Local API protection", CLI.md/MCP.md auth notes, CHANGELOG Security. The code was still uncommitted in the working tree when these docs were written. |
| P0-4 `OWNER/…` and `example.com` placeholders | fixed | `52f648e` (Cargo `repository`), `6e53e19` (installers, issue template), `13325eb`, `86b3a7c`, `696d81f`, `5c47c3f`, `3046fc2`, `7c7c3a8` | Every doc points at `github.com/cumabozkurt/bozkurt-code`; one-liners without `BOZKURT_REPO`; security and conduct contact `info@cumabozkurt.tr` (plus GitHub private advisories). |
| P0-5 Docs say daemon/CLI/MCP are placeholders | fixed | `13325eb`, `86b3a7c`, `696d81f`, `b7eff02`, `0c449e4`, `e076d52`, `786bb55`, `5c47c3f`, `3046fc2`, `7c7c3a8` | Status blocks replaced by "1.0 — supported platforms / known limitations"; release badge; SECURITY `1.0.x ✅ / < 1.0 ❌`. `docs/API.md` "(contract)" / "no auth" wording removed in the `docs(api)` commit (see P1-7). |
| P0-6 Version strings 0.1.0; tag mismatch only warns | fixed | `52f648e`, `fad9ed8` (versions), `6e53e19` (`check-versions.mjs`, hard failure), docs `13325eb`, `86b3a7c`, `b7eff02` | Doc examples use `v1.0.0` / `BOZKURT CODE 1.0.0`. |

## P1

| Finding | Status | Commit(s) | Notes |
|---|---|---|---|
| P1-1 iOS simulator job | fixed | `6e53e19` | Built via `tauri ios build`. Not verified on a macOS runner. |
| P1-2 Mobile artifacts never reach the release | fixed | `6e53e19` | APK/AAB/IPA attached, `SHA256SUMS` recomputed. CONTRIBUTING updated in `696d81f`. |
| P1-3 App identifiers | fixed | `6e53e19` | `tr.cumabozkurt.bozkurtcode` / `.mobile`. `apps/desktop/README.md:21` still says `dev.bozkurt.code` (outside this docs pass's scope: `apps/`). |
| P1-4 Auto-updater | fixed (check); install UI in 1.1 | `6e53e19`; docs `696d81f`; client: `feat(desktop): register tauri-plugin-updater` (`git log --grep tauri-plugin-updater`) | Release side: `createUpdaterArtifacts`, `.sig`, `latest.json`, pubkey from `vars.TAURI_UPDATER_PUBKEY`, secrets in CONTRIBUTING. Client side (final verification, 2026-10-10): `tauri-plugin-updater` 2.13.2 in `apps/desktop/src-tauri` only, registered in `setup` only when `plugins.updater` is configured (the plugin refuses to start without a pubkey, so dev builds and forks without a signing key skip it); checks `latest.json` at startup and emits `bozkurt://update-available` `{version, current_version, notes}`; nothing is installed automatically. `tauri-plugin-process` not needed (no relaunch yet). The Tauri crates declare `rust-version = "1.90"` (Tauri 2.12 and the plugin need it); the workspace and core crates stay on 1.89. Install/restart prompt in the UI: ROADMAP 1.1. |
| P1-5 `docs/CLI.md` stale | fixed | `b7eff02` | Regenerated from `crates/bz-cli/src/cli.rs` and `commands.rs`: database and undo routes, `site rm/set/list/info`, `daemon start/stop/status/run` (`POST /daemon/shutdown`), `doctor --fix`, `mcp register --command`, `-a`, `config set` JSON parsing, hidden aliases, auth. Gaps section removed. |
| P1-6 `docs/MCP.md` stale | fixed | `0c449e4`, `13325eb`, `86b3a7c`, `786bb55` | Matches `crates/bz-mcp/src/tools.rs` (55 tools): database routes, `wait` on `<rt>_install`/`services_install`/`tls_trust`/`share_start`, `system` on `tls_trust`; "55" everywhere. |
| P1-7 `docs/API.md` vs `api.rs` | fixed | `docs(api)` commit (`git log --grep P1-7`) | Rewritten as the "HTTP API v1 reference" with the 1.x stability promise. Every route of `api::router` documented (incl. `GET /services/{name}`, `GET /share/{site_id}`, `POST /daemon/shutdown`, database, undo and doctor-fix routes); "v1 additions" appendix folded in; 201 for `POST /sites` and `POST /services/{engine}/databases`. |
| P1-20 / P1-21 (RUST_AUDIT, doc side) | fixed (docs); code items open | same commit | API.md: Authentication (`$BOZKURT_HOME/api-token` 0600, `Authorization: Bearer` / `X-Bozkurt-Token`, `?token=` on `/events`, `/health` exempt, allowed Origins incl. `tauri://localhost`, dev origins only in debug or `BOZKURT_DEV=1`, Host check, JSON content type, 401/403/415/421 envelopes), remote-listener differences, `lan_access` + loopback-only Caddy bind, site validation rules, `{token, device_id}` from `/remote/pair`, `running` in `GET /remote`, `TlsStatus.ca_expires` optional. `packages/ui/src/api/types.ts`: `TlsInfo.ca_expires` made optional (typecheck green). P1-21 (CLI.md/MCP.md) was already fixed by P1-5/P1-6. Still open in code: `POST /import` ignores `dns` (documented as ignored); types.ts is still hand-mirrored (no codegen). |
| P1-8 ARCHITECTURE claims | fixed | `e076d52`, `696d81f` (CONTRIBUTING core rule) | Exact checksum sources vs HTTPS-only (static-php.dev); `bz-net` plans instead of a helper; real shim list (no `bz` shim). |
| P1-9 Wording mirroring another product's marketing | fixed | `13325eb`, `86b3a7c`, `696d81f`, `b7eff02`, `e076d52`, `5c47c3f`, `3046fc2`, `7c7c3a8` | "Reference product", "never grabs/takes a port silently", "one admin prompt, ever", "asks for your password once", "you tick the ones you want" rewritten; competitor lists removed from README intros and README comparison tables (only the neutral, sourced `docs/COMPARISON.md` remains). DNS default port **kept at 5390** to match `crates/bz-core/src/config.rs`. The UI label "Quick setup" is a UI string (`packages/ui`), not changed here. |
| P1-10 Agent `STUB.md` files | fixed | `6e53e19` | Deleted. |
| P1-11 Third-party licence notices | fixed | `6e53e19` | `scripts/third-party-licenses.py`, shipped in archives and as a bundle resource. Mentioned in README (`13325eb`, `86b3a7c`). |
| P1-12 CHANGELOG / ROADMAP / SECURITY not 1.0-ready | fixed | `696d81f`, `7c7c3a8` | `## [1.0.0] - 2026-10-09` (Highlights, Added, Security, Changed, Fixed) with 0.1 work folded in and the 1.x stability promise; link ref `v1.0.0`. No `[Unreleased]` section: open one after publishing (release checklist). ROADMAP re-based on 1.0 with known limitations. |
| P1-13 Desktop-only users have no `bz`/`bozkurt-mcp` on PATH | left (documented) | docs `696d81f`, `5c47c3f`, `3046fc2` | Needs a desktop action (code). Docs now tell desktop users to run the CLI installer too; ROADMAP 1.1 lists "Install command-line tools". |

## P2

| Finding | Status | Commit(s) | Notes |
|---|---|---|---|
| P2-1 Pin actions by SHA | fixed | `6e53e19` | |
| P2-2 Reproducibility (`--locked`, `rust-version`) | fixed | `6e53e19`, `52f648e` | `rust-version = 1.89`. No `rust-toolchain.toml`. |
| P2-3 Secrets via `$GITHUB_ENV` | fixed | `6e53e19` | Notarization secrets scoped to the step. |
| P2-4 Provenance / signed sums | partial | `6e53e19`; README `13325eb` | Build-provenance attestations added and `gh attestation verify` documented. `SHA256SUMS` itself is not signed (minisign/cosign). |
| P2-5 CI coverage gaps | fixed | `6e53e19` | Mobile shell check, repo-lint job (actionlint, shellcheck, PSScriptAnalyzer, JSON, Markdown links, version drift). |
| P2-6 Linux deps (ayatana) | fixed | `6e53e19` | |
| P2-7 Runner / glibc notes | fixed | `6e53e19` (workflow comment, `install.sh` header), `13325eb`, `86b3a7c`, `696d81f` (ROADMAP) | glibc ≥ 2.35, no musl, public-repo-only arm runner. |
| P2-8 `install.sh` shellcheck | fixed | `6e53e19` | |
| P2-9 Tauri `copyright` | fixed | `6e53e19` | |
| P2-10 Capabilities / CSP | fixed | `6e53e19` | `opener:default` dropped; CSP as described in `6e53e19`. |
| P2-11 Single source of version | left | — | Both `tauri.conf.json` still carry `"version": "1.0.0"`; drift is now caught by `scripts/check-versions.mjs` in CI and on tag. |
| P2-12 SPDX licence headers | partial | `6e53e19` | Present in `scripts/*` (sh, ps1, mjs, py) only; 0 of 76 `.rs` and 0 of 34 `.ts/.tsx` files carry one. No enforcement. |
| P2-13 Mobile toolchain / Play target API | left | — | `platforms;android-34` unchanged; Play target-API and Play App Signing note still to check. |
| P2-14 CLI help text | left | — | Code change in `crates/bz-cli` (out of scope here). |
| P2-15 ARCHITECTURE layout nits | fixed | `e076d52`, `5c47c3f`, `3046fc2` | Home layout lists `cache/`, `tmp/`, `bozkurtd.pid`, `api-token`, `caddy/bin`, `caddy/data`, `caddy/snippets`; repo layout fixed (CONTRIBUTING at root, mobile in its own workflow). |
| P2-16 Comparison consistency | fixed | `786bb55`, `13325eb`, `86b3a7c` | MCP count 55; Herd Node cell "via nvm; free or Pro tier ❓"; README tables replaced by a link, so they cannot drift. Fetch date stays 2026-10-09 (= release date). |
| P2-17 Turkish terminology | fixed | `86b3a7c`, `3046fc2` | `bozkurtd` = "arka plan hizmeti (daemon)", "servis" only for services; "Türkçe ve İngilizce arayüz"; "önde çalışan ters vekil sunucu (reverse proxy)"; "--target HEDEF (ör. aarch64-apple-darwin)"; "e-posta yakalayıcı (SMTP)". |

## Daemon security follow-up (UI + docs)

Reflects the daemon changes `ba9499d`..`9b04d4c` (see [`RUST_FIX_LOG.md`](RUST_FIX_LOG.md): D1, P1-1, P1-2, P1-3, P1-4,
P1-18, remote bind-before-save, `lan_access` reload, Caddy autostart) in the UI and the docs. No Rust touched.

| Item | Status | Commit(s) | Notes |
|---|---|---|---|
| UI: new daemon error codes / messages | fixed | `c071404` | `lib/daemonText.ts` maps (EN/TR) `unverified download: …`, `checksum mismatch … (differs from the sha256 pinned …)`, plain `checksum mismatch`, `<key> must be between 1 and 65535`, `<key> (<n>) is already used by <other>`, `<key> can only be changed locally`, `cannot listen on 0.0.0.0:<port>: …`, `Too many pairing attempts`, `Too many wrong pairing codes`; `store.errorMessage` maps the codes `pairing_locked` / `rate_limited`; job messages `Installed and started caddy X`, `Installed caddy X (could not start it: …)`, `Unverified download: … pinned`. |
| UI: PHP (any runtime) install on `unverified_download` | fixed | `c071404` | New `store.installRuntime()` used by Runtimes, Quick setup and the site dialog: a failed job whose message starts with `unverified download:` opens a dialog (static-php.dev publishes no checksums; the first download is pinned; how to allow always) with **Install anyway**, which resends `POST /runtimes/{kind}/install {version, allow_unverified: true}`; the generic error toast is suppressed for that job. `api.installRuntime(kind, version, allowUnverified?)`. Detection is by message prefix because job failures carry no `code`. |
| UI: 8-digit pairing | fixed | `c071404` | `PAIRING_CODE_DIGITS = 8` in `api/client.ts` (QR comment fixed); Connect screen input `maxLength`/`pattern`/validation/auto-pair use it; desktop shows the code as `1234 5678`; hint about single use / revocation after 10 wrong tries. |
| UI: remote enable 409 | fixed | `c071404` | Settings shows the localized bind error inline (`callout danger`) and re-reads `GET /remote`, so the switch reflects that `remote.enabled` was not saved. |
| UI: port errors inline | fixed | `c071404` | A 400 port error from `PATCH /config` is shown under the matching field (or the editable field it clashes with) instead of a toast. |
| UI: `lan_access` / `allow_unverified` | fixed | `c071404` | Settings → Network: `lan_access` switch (confirm dialog when turning on, warning while on; read-only on a paired device); `allow_unverified` shown read-only with an explanation (change it in `config.toml`). After a TLD change the UI checks `GET /tls` and offers **Trust CA** when the regenerated CA is not trusted; the TLD hint says a non-`test` TLD renews the CA. |
| UI: mock backend | fixed | `c071404` | Port validation with daemon messages, PHP fail-closed + pins (file name `php-<v>-cli-macos-aarch64.tar.gz`), pairing limits (30/min, lock after 10, 8 digits, 401 `bad_code`), remote bind 409 on ports 3306/5432/6379/8080, `lan_access` / `allow_unverified` keys. |
| Docs: API.md | fixed | `7c924da` | Error table (`unverified_download` 422, `pairing_locked` 429, 403 config keys, 409 bind message), remote listener `api_port`/`remote.port` 403, `PairingCode` 8 digits, `Config.allow_unverified`, install body `allow_unverified`, new "Unverified downloads" subsection (fail closed, TOFU pins in `state.json` `download_pins`, `checksum_mismatch` on pin mismatch), port rules + startup failure, bind-before-save for `/remote/enable`/`disable`/`PATCH /config`, `lan_access` reloads Caddy now, local CA name constraints, Caddy admin socket, pairing error table. |
| Docs: CLI.md, MCP.md | fixed | `f948e61` | `--allow-unverified` on `bz <runtime> install` and `bz runtime install` (`use` does not pass it); `config set` validation note. MCP: "Shell commands need confirmation" (which tools, per-session memory, detected commands), `confirm?` in the site tool table, env var table incl. `BOZKURT_MCP_ALLOW_COMMANDS`, no `allow_unverified` over MCP. |
| Docs: SECURITY.md, ARCHITECTURE.md | fixed | `1c58215` | SECURITY: scope bullets updated; new sections Caddy admin API, Remote access and pairing (limits + cleartext limitation), Local CA (constraints, re-trust, legacy CA kept), Unverified downloads, MCP confirmation; hardening tips. ARCHITECTURE: CA constraints, admin socket, `caddy/run/`, `download_pins`, `allow_unverified`, port rule, fail-closed download rule, MCP rule. |
| Docs: USER_GUIDE.md / .tr.md, READMEs | fixed | `4685cd4` | PHP "Install anyway" prompt + `--allow-unverified`, re-trust after a TLD change, 8-digit code, pairing limits, busy remote port, Settings → Network, `allow_unverified` in the config sample, troubleshooting rows. README.md / README.tr.md / `apps/mobile/README.md` said "6-digit". |
| Docs: CHANGELOG 1.0.0 | fixed | `3f57177` | Security: downloads fail closed, pairing limits, name-constrained CA, private Caddy admin API, MCP confirmation, config validation. Fixed: Caddy autostart after install, `lan_access` reload. |
| Browser smoke of the mock UI | left | — | `work/ui-fix/smoke-mock.js` needs `/usr/bin/chromium`; `apt-get` is not available to this user (no root) and `playwright install` failed, so no screenshot run. Instead `work/ui-daemon-sync/smoke.ts` (esbuild + node) exercised the mock and `daemonText` end to end: PHP refused → localized EN/TR → allowed + pinned; three port errors mapped to fields; remote 409 leaves `enabled: false`; 8-digit code, lock after 10 wrong guesses, new code unlocks. |
| Open | left | — | Job failures carry no error `code`, so the UI recognises `unverified download:` / `checksum mismatch` by message prefix; a `code` field on failed jobs would make this robust. `allow_unverified` is not editable in the UI by design. The desktop does not learn that a code was revoked by a phone's wrong guesses (shown code stays until it expires). Services/cloudflared installs that hit `unverified_download` show the localized error but no "Install anyway" (the API has no per-install flag for services). |

## Validation

- `python3 scripts/check-md-links.py` → "no broken internal links (25 files)".
- `grep -E "OWNER|[Pp]laceholder|0\.1\.0|v0\.1\b|[Uu]nreleased|example\.com|reference product"` over every tracked Markdown
  file outside `crates/`, `apps/`, `packages/`, `.github/`, `docs/audit/` and `docs/API.md` → no matches.
- `cargo` was not run (docs-only pass).
- Daemon security follow-up: `npm run typecheck` and `npm run build` at the repo root are green;
  `python3 scripts/check-md-links.py` → "no broken internal links (26 files)".

## Final verification (2026-10-10)

Run on a Linux aarch64 sandbox (2 cores, 7 GB), rustc 1.99.0, Node 22, `CARGO_BUILD_JOBS=1`. Tauri system libraries
(`libwebkit2gtk-4.1-dev`, `libayatana-appindicator3-dev`, `librsvg2-dev`, `libssl-dev`) installed, so the desktop and
mobile crates were built, linted and tested too.

| Check | Result |
|---|---|
| P1-4 client side | `tauri-plugin-updater` 2.13.2 added to `apps/desktop/src-tauri` and registered when `plugins.updater` is configured (`feat(desktop): register tauri-plugin-updater`); desktop + mobile `cargo check`/`clippy` green. Tauri crates declare `rust-version = "1.90"`; workspace stays 1.89. Rustfmt diffs in `apps/desktop/src-tauri/src/lib.rs` fixed; unused `RunEvent` import on non-macOS removed |
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean (0 warnings), all 7 members incl. desktop and mobile |
| `cargo test --workspace --no-fail-fast` | 26 test binaries/doc-test sets, **194 passed, 0 failed, 6 ignored** (5 network `live` tests in bz-cli, 1 doc-test). Incl. bz-net `dns_e2e` (1), `tls_e2e` (2), `service` (1); bz-mcp `mcp` (3); bz-daemon `api` (7); bz-core 97; desktop lib 3 |
| `cargo check --target x86_64-pc-windows-gnu` (bz-core, bz-net, bz-daemon, bz-cli, bz-mcp) | clean (0 warnings) |
| `npm run typecheck` / `npm run build` | green (ui, desktop, mobile) |
| `node scripts/check-versions.mjs` | all version strings = 1.0.0 |
| `python3 scripts/check-md-links.py` | no broken internal links |

**Live e2e** (debug `bozkurtd`/`bz`/`bozkurt-mcp`, isolated `BOZKURT_HOME`, ports http 18080, https 18443, dns 15353,
api 17878, remote 17879; full transcript kept outside the repo in `work/e2e-final/TRANSCRIPT.md`):

- `/health` without token 200; `/sites` without token 401, with Bearer or `X-Bozkurt-Token` 200, wrong token 401.
- Cross-origin `text/plain` POST with `Origin: http://evil.example` → 403; `text/plain` with token → 415;
  `Host: evil.example` → 421. `api-token` is mode 0600, 64 hex.
- PHP 8.4 install without `allow_unverified` fails closed (`unverified download: php-8.4.23-cli-linux-aarch64.tar.gz has
  no published checksum …`, the `unverified_download` error, reported as the job message; `bz php install` prints it
  too); with `allow_unverified: true` → installed, both static-php.dev URLs pinned in `download_pins`.
- Caddy 2.11.7 installed and started; PHP site → HTTPS with the local CA 200 (`php-ok 8.4.23`), HTTP 308 → https,
  `/.env` 404; DNS `demo.test` A/AAAA → 127.0.0.1/::1, `example.com` REFUSED.
- Caddyfile: `default_bind 127.0.0.1 [::1]`, `admin unix/…/caddy/run/admin.sock`; nothing listens on :2019; Caddy's
  only TCP listeners are loopback :18080/:18443.
- `PATCH /config` with duplicate ports (`https_port` = `http_port`, `remote.port` = `api_port`) or `dns_port` 0 → 400,
  `config.toml` unchanged.
- MCP over stdio: 55 tools, `sites_list` → `demo.test`.
- `POST /daemon/shutdown` → 401 without token, 202 with; bozkurtd, Caddy and 3 php-fpm processes gone, no zombies,
  pid file removed, all listeners closed.
- **Bug found and fixed:** Caddy answered admin reloads over the unix socket with `403 client is not allowed to access
  from origin 'http://127.0.0.1'`, so every reload fell back to `caddy reload`. Fix `f7d7136` (no `Origin` on the
  socket, like Caddy's CLI; unit test asserts it); the rerun showed no fallback.
- Not covered live: remote listener/pairing (0.1 e2e + `api` tests), real ports 80/443/53, trust-store install,
  Windows/macOS hosts.
