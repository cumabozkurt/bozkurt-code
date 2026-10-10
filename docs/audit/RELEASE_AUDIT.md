# BOZKURT CODE — 1.0.0 release audit (CI/CD, packaging, docs)

Audited commit: `397f5cd` (main) · Date: 2026-10-09 · Scope: `.github/workflows`, Tauri configs, install scripts,
all Markdown docs, version strings, licensing. Read-only outside `docs/audit/`. `cargo` was **not** run (shared, memory-tight
sandbox); the existing debug binaries in `work/target-daemon/debug/` were used for `bz --help` and `bozkurt-mcp` `tools/list`.

**Summary:** 6 × P0 (release blockers), 13 × P1 (fix before tagging 1.0.0), 17 × P2 (polish / hardening).

| Priority | Meaning |
|---|---|
| **P0** | 1.0.0 cannot ship: the shipped artifact is broken, unsafe, or the docs tell users to run commands that cannot work. |
| **P1** | Ship-quality problem: wrong/stale docs, irreversible decisions, missing release plumbing. |
| **P2** | Hardening, reproducibility, consistency, nits. |

## How this was checked

| Check | Tool / method | Result |
|---|---|---|
| Workflow lint | `actionlint 1.7.7` + `shellcheck 0.11` (for `run:` blocks) on `ci.yml`, `release.yml`, `mobile.yml` | **0 findings** |
| Installer lint | `shellcheck -s sh scripts/install.sh` | 3 × SC2088, 1 × SC2086 (cosmetic, P2-8) |
| CLI docs | `bz --help` + every subcommand `--help` (75 help pages) vs `docs/CLI.md`; script cross-check of every `bz …` invocation in README/README.tr/USER_GUIDE(.tr)/CLI/MCP/CONTRIBUTING/ARCHITECTURE | All invoked commands/flags exist; reference doc is stale/incomplete (P1-5) |
| MCP docs | `bozkurt-mcp` over stdio: `initialize` → `tools/list`, `resources/list`, `prompts/list` | 55 tools, names/tiers match `docs/MCP.md`; args/notes stale (P1-6) |
| API docs | `crates/bz-daemon/src/api.rs` router (30 routes) vs `docs/API.md` | 2 routes undocumented, additions section unmerged (P1-7) |
| Links | custom Markdown link + anchor checker over 22 `.md` files; `curl` of all 49 external URLs | 0 broken internal links/anchors; every non-200 is an `OWNER/…` placeholder (P0-4) |
| Comparison claims | Re-fetched herd.laravel.com, laragon.org/pricing, kettlecode.org, servbay.com on 2026-10-09 | Spot-checked claims hold (P2-16 for one ambiguity) |

What is already correct (no action needed): Rust target triples and runner labels in `release.yml` (5 desktop targets);
sidecar naming `name-<target-triple>[.exe]` in `release.yml:144-151` and `ci.yml:111-118`; CLI archive names
`bozkurt-cli-<triple>.tar.gz|.zip` match `scripts/install.sh:102` and `scripts/install.ps1:52`; `SHA256SUMS` format matches
both installers' parsers; every `npm run …` script referenced by the workflows/docs exists; Tauri v2 mobile commands
(`android init --ci`, `android build --apk --aab --ci`, `ios init --ci`, `ios build --export-method`) are valid; CI
`permissions: contents: read`; `concurrency` groups; npm + `Swatinem/rust-cache` caching; dependabot covers actions, cargo, npm.

---

## P0 — release blockers

### P0-1 Desktop bundles do not contain the daemon sidecar
- `.github/workflows/release.yml:226` and `:237` — `tauri-action` `args` never pass the sidecar overlay
  (`--config src-tauri/tauri.sidecar.conf.json`), and `apps/desktop/src-tauri/tauri.conf.json` has no `bundle.externalBin`.
  The binaries are staged into `src-tauri/binaries/` (`release.yml:144-151`) but never bundled. Result: in an installed app
  `app.shell().sidecar("bozkurtd")` (`apps/desktop/src-tauri/src/lib.rs:102`) fails and the fallback looks next to the exe /
  on `PATH` (`lib.rs:111-122`), so a desktop-only user gets a permanently "daemon offline" app.
- `apps/desktop/src-tauri/tauri.sidecar.conf.json:4` lists only `binaries/bozkurtd`, while `README.md:117`,
  `README.tr.md:113` and `CONTRIBUTING.md:66-69` say the app bundles `bozkurtd`, `bz` **and** `bozkurt-mcp`.
- **Fix:**
  1. `tauri.sidecar.conf.json`: `"externalBin": ["binaries/bozkurtd", "binaries/bz", "binaries/bozkurt-mcp"]` (or drop the
     claim in the READMEs if only the daemon should ship).
  2. `release.yml` both build steps: `args: --target ${{ matrix.target }} --bundles ${{ matrix.bundles }} --config src-tauri/tauri.sidecar.conf.json ${{ env.TAURI_EXTRA_ARGS }}`
     (Tauri v2 CLI accepts repeated `--config`; paths are relative to `projectPath: apps/desktop`).
  3. Add a post-build assertion per OS (e.g. `ls "…/BOZKURT CODE.app/Contents/MacOS/bozkurtd"`, `dpkg -c *.deb | grep bozkurtd`,
     `7z l *.msi`) so this cannot regress silently.
  4. Keep the `ci.yml:111-118` stubs (they become required once `externalBin` is real) and add a CI `cargo check` with
     `TAURI_CONFIG` set to the overlay so the overlay itself is exercised.

### P0-2 Mobile release APK cannot reach the LAN daemon (patch script never runs)
- `.github/workflows/mobile.yml:76-83` runs `android init` then `android build` but never runs
  `node apps/mobile/scripts/patch-android.mjs`. The generated release manifest keeps `usesCleartextTraffic=false`, so the
  companion's plain-HTTP calls to `http://<lan-ip>:7879` fail in every CI-built APK/AAB. `docs/ROADMAP.md:39-40` already
  lists this as a gap.
- **Fix:** insert after "Generate Android project":
  ```yaml
  - name: Allow cleartext HTTP to the LAN daemon
    run: node apps/mobile/scripts/patch-android.mjs
  ```
  and make the script fail loudly when the regex matched nothing on a fresh template (today it prints "no change needed"
  and exits 0 — `patch-android.mjs:16`), e.g. assert the final file contains `usesCleartextTraffic"] = "true"`.
  Longer term, prefer a scoped `network_security_config.xml` (cleartext for private ranges only) over a global flag.

### P0-3 Local API is reachable from any web page (CSRF + DNS rebinding) — found while diffing API.md vs api.rs
- `crates/bz-daemon/src/api.rs:44-58`: the `Body<T>` extractor parses JSON **regardless of `Content-Type`**, and the router
  (`api.rs:89-133`) has no `Origin` or `Host` validation. CORS (`api.rs:147-161`) only stops a page from *reading*
  responses; a "simple" cross-site request (`POST`, `Content-Type: text/plain`, `mode: "no-cors"`) is still delivered and
  executed. Any website the user visits can therefore e.g. `POST /api/v1/services/all/stop`, `POST /api/v1/sites` with an
  arbitrary `path`, or start DNS/TLS jobs that pop OS admin prompts. With DNS rebinding (no `Host` check) a page can also read
  responses. `SECURITY.md:41-44` declares exactly this in scope. *Established by code reading; not exploited live.*
- **Fix (all three, cheap):** reject state-changing requests whose `Content-Type` is not `application/json` (forces a CORS
  preflight); reject any request carrying an `Origin` header not in `CORS_ORIGINS`; on the local listener accept only
  `Host: 127.0.0.1:<api_port>` / `localhost:<api_port>`. Add integration tests for each. Optional defence-in-depth: a
  per-install local token in `$BOZKURT_HOME` (0600) sent by `bz`, `bozkurt-mcp` and the desktop shell.

### P0-4 `OWNER/…` and `example.com` placeholders in shipped files
Every documented install one-liner 404s today, and both installers refuse to run without `BOZKURT_REPO`.
- `Cargo.toml:10` (`repository`), `scripts/install.sh:4,18`, `scripts/install.ps1:10,17,25`,
  `README.md:13,109,129,136-137,141`, `README.tr.md:105,125,132-133,137`, `docs/USER_GUIDE.md:63,67-68`,
  `docs/USER_GUIDE.tr.md:70,74-75`, `CHANGELOG.md:86-87`, `docs/ROADMAP.md:4`, `CONTRIBUTING.md:175`,
  `.github/ISSUE_TEMPLATE/config.yml:4,7`, `SECURITY.md:20`.
- Contacts: `SECURITY.md:21` (`SECURITY_CONTACT@example.com`), `CODE_OF_CONDUCT.md:39` (`CONDUCT_CONTACT@example.com`).
- **Fix:** pick the final repo slug (e.g. `cumabozkurt/bozkurt-code`) and replace globally; drop the `BOZKURT_REPO=…`
  prefix from the one-liners and the "placeholder" notes (`README.md:141-142`, `README.tr.md:137-138`); keep the
  installers' `OWNER/*` guard only as a dead-code safety net or remove it. Use a real, monitored address for security and
  conduct reports (or GitHub private advisories only, and say so).

### P0-5 User-facing docs still say the daemon/CLI/MCP are unbuilt placeholders
- `README.md:15` (badge "0.1 unreleased"), `README.md:40-44`, `README.tr.md:34-38`, `docs/CLI.md:6`, `docs/MCP.md:8`,
  `docs/USER_GUIDE.md:8-9`, `docs/USER_GUIDE.tr.md:9-10`, `docs/COMPARISON.md:14-15,81`, `CHANGELOG.md:80-84`,
  `docs/ROADMAP.md:23-29` (🚧 rows for daemon/CLI/MCP/import/share), `docs/ARCHITECTURE.md:1` and `docs/API.md:1`
  ("contract"), `SECURITY.md:10-13` (supported version "main (pre-release 0.1.x)").
- All three binaries exist and answer (`bz 0.1.0`, `bozkurt-mcp` serverInfo `0.1.0` with 55 tools, `bozkurtd` with 30 routes).
- **Fix:** replace the status blocks with a short "1.0 — supported platforms / known limitations" note (MariaDB only
  Linux x64 + Windows x64, Valkey only Linux, Windows x64 only); change the badge to a release badge
  (`https://img.shields.io/github/v/release/<owner>/bozkurt-code`); SECURITY.md table → `1.0.x ✅`, `< 1.0 ❌`.

### P0-6 Version strings are 0.1.0 everywhere; tag/version mismatch only warns
Files to bump to `1.0.0`: `Cargo.toml:7` (all crates inherit), `package.json:4`, `packages/ui/package.json:3`,
`apps/desktop/package.json:3` and `:17` (`@bozkurt/ui` dep), `apps/mobile/package.json:3` and `:17`,
`apps/desktop/src-tauri/tauri.conf.json:5`, `apps/mobile/src-tauri/tauri.conf.json:5`; regenerate `package-lock.json`
(`:21,:24,:40,:43,:2590`) and `Cargo.lock`. Docs examples: `README.md:130,138`, `README.tr.md:126,134`,
`scripts/install.sh:11`, `scripts/install.ps1:13`, `docs/CLI.md:1,59` (`v0.1 contract`, sample output).
- `release.yml:42-48` emits `::warning::` on mismatch and continues → a `v1.0.0` tag would publish 0.1.0 binaries.
- **Fix:** make it `::error::` + `exit 1`, and also compare against `apps/desktop/package.json` and both `tauri.conf.json`
  (or remove `version` from the Tauri configs so they inherit the Cargo version — see P2-11). Android `versionCode` will
  derive to `1000000` from `1.0.0`; record that in the release notes for Play.

---

## P1 — fix before tagging 1.0.0

### P1-1 iOS "unsigned simulator" job very likely fails
`mobile.yml:160-174` calls `xcodebuild` directly on the generated project. Tauri's generated Xcode project runs a
"Build Rust Code" phase (`tauri ios xcode-script`) that expects options written by a parent `tauri ios build|dev`
process, so a bare `xcodebuild` is expected to fail with "failed to read CLI options". *Not run in this audit — verify on a
macOS runner.* **Fix:** build through the CLI, e.g. `npm run tauri -- ios build --target aarch64-sim --ci` (debug if needed),
then zip the `.app` from `src-tauri/gen/apple/build`. Drop the unused `x86_64-apple-ios` rust target (`mobile.yml:137`).

### P1-2 Mobile artifacts never reach the GitHub Release
`mobile.yml` only uploads workflow artifacts (`:121-125`, `:211-215`, default 90-day retention) and has
`permissions: contents: read` (`:22-23`). `paths-ignore` under `push.tags` (`:12-14`) has no effect (path filters are not
evaluated for tag pushes). **Fix:** on tag runs, add `contents: write` and `gh release upload "$GITHUB_REF_NAME" dist/android/* dist/ios/* --clobber`
(after the draft exists — `needs` across workflows is impossible, so either move the mobile jobs into `release.yml` behind
`create-release`, or upload with retry). Re-run the `checksums` job afterwards so `SHA256SUMS` covers APK/AAB/IPA.

### P1-3 App identifiers are permanent after 1.0 — confirm them now
`apps/desktop/src-tauri/tauri.conf.json:6` `dev.bozkurt.code`, `apps/mobile/src-tauri/tauri.conf.json:6`
`dev.bozkurt.code.mobile`. These become the macOS bundle ID, Windows/Linux app-data folders, the Android `applicationId`
(Play package — immutable) and the iOS bundle ID. Use reverse-DNS of a domain the project controls (e.g. `tr.cumabozkurt.code`
if `bozkurt.dev` is not owned). Also update `apps/desktop/README.md` ("Identifier `dev.bozkurt.code`").

### P1-4 No auto-updater — proposal: `tauri-plugin-updater` with signed manifests for 1.0
`CONTRIBUTING.md:165` and `docs/ROADMAP.md:47` defer it. Shipping 1.0 without it strands every 1.0 user on 1.0.
Proposed wiring (keys per https://v2.tauri.app/plugin/updater/, fetched 2026-10-09):
1. `npm run tauri -w @bozkurt/desktop -- signer generate -w ~/.tauri/bozkurt.key`; store `TAURI_SIGNING_PRIVATE_KEY` and
   `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` as repo secrets; back up the private key offline (losing it = no more updates).
2. `apps/desktop/src-tauri/Cargo.toml`: `tauri-plugin-updater = "2"` under
   `[target.'cfg(any(target_os = "macos", windows, target_os = "linux"))'.dependencies]` (plugin needs Rust ≥ 1.90).
3. `tauri.conf.json`:
   ```json
   "bundle": { "createUpdaterArtifacts": true, … },
   "plugins": { "updater": {
     "pubkey": "<contents of bozkurt.key.pub>",
     "endpoints": ["https://github.com/<owner>/bozkurt-code/releases/latest/download/latest.json"],
     "windows": { "installMode": "passive" } } }
   ```
4. `release.yml` build steps: add `TAURI_SIGNING_PRIVATE_KEY` / `_PASSWORD` to `env`; keep `tauri-action` (it uploads
   `.sig` files and merges `latest.json` across the matrix when `releaseId` is set; leave `includeUpdaterJson` at its
   default `true`).
5. Rust side (`lib.rs` setup): register `tauri_plugin_updater::Builder::new().build()` under `#[cfg(desktop)]`; check from
   Rust (tray item "Check for updates…" + a daily check) so no `updater:*` capability is granted to the webview. Before
   `install()`, stop the `bozkurtd` child (Windows installers cannot replace a running sidecar; use `on_before_exit`).
6. Caveats to document: `/releases/latest/` only resolves after the draft is **published**; on Linux only the AppImage
   self-updates (deb/rpm users update via package); CLI-only installs keep using the install scripts (consider `bz self-update`).

### P1-5 `docs/CLI.md` is stale and incomplete against the real `bz`
- `:6` placeholder status (P0-5). `:132-135` says `bz dns setup --undo` / `bz trust --undo` "need an API endpoint not yet in
  API.md" — both flags exist and `POST /dns/unconfigure`, `POST /tls/untrust` are routed (`api.rs:435,493`).
- `:142-147` and `:215-223` ("proposed `GET /db`…", "Gaps against API.md v1") — the implementation uses
  `GET|POST /services/{engine}/databases` and `DELETE /services/{engine}/databases/{name}` (`api.rs:102-106,310,337`). Delete the
  Gaps section; fix the API column.
- Undocumented but shipped: `bz site rm|set <site> <key> <value>|list|info`, `bz daemon start|stop|status|run`,
  `bz mcp register --command <PATH>`, `bz php list -a`, `bz config set` "values are parsed as JSON when possible".
- **Fix:** regenerate the command tables from `bz … --help` (consider a `cargo xtask docs-cli` or a CI diff against
  `clap_mangen`/markdown output so this cannot drift again).

### P1-6 `docs/MCP.md` is stale against `tools/list`
- `:8` placeholder status. `:130-134` db tools "proposed `/db` … return `not_implemented`" — they are implemented over
  `/services/{engine}/databases`.
- Undocumented arguments present in the schemas: `wait` on `<rt>_install`, `services_install`, `tls_trust`, `share_start`;
  `system` on `tls_trust` (`:104-108,118,145-146`).
- `:78` "About 50 tools" vs actual **55** (the doc's own total at `:149` is right). Same for `README.md:84,270` and
  `README.tr.md:80,227` ("~50"); `docs/COMPARISON.md:74` already says ~55 → use "55".

### P1-7 `docs/API.md` vs `crates/bz-daemon/src/api.rs`
- Missing: `GET /services/{name}` (→ Service, `api.rs:99-100,281`), `GET /share/{site_id}` (`api.rs:126-129,638`).
- `:93-100` "v1 additions (2026-10-09, binding)" is a changelog appendix; fold the CORS list, `?token=`, database routes,
  undo routes, doctor fix ids and `PortOwner.in_use` into the main tables/types, and document `POST /services/{engine}/databases`
  returning **201** `{name}`.
- `:1` "(contract)" → "HTTP API v1 reference"; state the stability promise for 1.x (additive changes only).

### P1-8 `docs/ARCHITECTURE.md` makes claims the code does not back
- `:107` "Every download is verified (SHA-256 from upstream checksums)" — false for static-php.dev
  (`crates/bz-core/src/catalogs.rs:202` "publishes no checksums; served over HTTPS only", `download.rs:49`). This is a
  security claim → state precisely which sources are checksum-verified and which are TLS-only.
- `:33` "One elevation, ever, done by a tiny helper" — there is no helper; `bz-net` applies named plans with one prompt per
  admin block (`crates/bz-net/src/lib.rs:11-12`).
- `:66` `bin/` shims include `bz` — no `bz` shim is created (observed home: `bun bunx composer corepack go gofmt node npm npx
  php pip pip3 python python3`).
- `:16` "Feature set (beyond the reference product)" — see P1-9. `:1` "(v0.1 contract)".

### P1-9 Clean-room positioning: copy echoes Kettle Code's marketing text
The docs assert a clean-room design from public descriptions (`README.md:31-32,302-304`, `ARCHITECTURE.md:3-5`). Several
phrases and defaults mirror kettlecode.org almost verbatim ("never grabs a port silently" `ARCHITECTURE.md:40`; "one admin
prompt, ever" `:33`; "You tick the ones you want" `CLI.md:153`; DNS on `:5390` `ARCHITECTURE.md:30,82`; "Quick Setup"),
and `ARCHITECTURE.md:16` calls it "the reference product". Not a legal opinion, but for a public 1.0 rewrite these lines
in original wording, remove "reference product", and consider whether the identical DNS port default is worth keeping.

### P1-10 Agent scaffolding committed in the Tauri crates
`apps/desktop/src-tauri/STUB.md` and `apps/mobile/src-tauri/STUB.md` ("Placeholder created by the bz-net agent…") are
tracked. Delete both.

### P1-11 No third-party license notices in distributed binaries
Release archives (`release.yml:159-162`) ship only `LICENSE-MIT`, `LICENSE-APACHE`, `README.md`; installers ship nothing.
The binaries statically link many MIT/Apache/ISC/BSD (and possibly MPL/Unicode/CDLA) crates and npm packages whose
licenses require attribution. **Fix:** generate `THIRD_PARTY_LICENSES.md` (cargo-about or `cargo deny` + `license-checker`
for npm) in CI, include it in CLI archives and as a Tauri `bundle.resources` file, and add a `deny.toml` license allow-list
check to CI.

### P1-12 CHANGELOG / ROADMAP / SECURITY not 1.0-ready
- `CHANGELOG.md:9-11` "[0.1.0] - Unreleased … not tagged yet"; `:80-84` "Not yet included". No tag `v0.1.0` exists in the repo
  (`git tag` is empty), so `:86-87` compare links will 404. **Fix:** add `## [1.0.0] - YYYY-MM-DD` (Added/Changed/Security),
  either fold 0.1.0 into it or tag 0.1.0 retroactively, and update the link refs to `v1.0.0`. Note the 1.x semver promise
  (`:5` "Until 1.0, minor versions may contain breaking changes" → state what is covered: CLI flags, exit codes, API v1, MCP
  tool names, `bozkurt.toml`).
- `docs/ROADMAP.md:8-40`: rename the section to 1.0.0, mark daemon/CLI/MCP/import/share done, delete resolved "Known
  gaps" (db + undo endpoints + command spellings are implemented; patch-android is P0-2).

### P1-13 Desktop-only users have no `bz` / `bozkurt-mcp` on `PATH`
Even after P0-1, sidecars live inside the app bundle; MCP docs (`docs/MCP.md:26-27`, `README.md:227-228`) point at
`~/.bozkurt/bin/bozkurt-mcp`, which only the CLI installer creates. **Fix:** add a desktop action (Settings → "Install command-line
tools") that symlinks/copies the bundled `bz` and `bozkurt-mcp` into `$BOZKURT_HOME/bin`, and make `bz mcp register`
default `--command` to the bundled path when not on `PATH`; document it in USER_GUIDE §12.

---

## P2 — hardening and polish

- **P2-1 Pin third-party actions by commit SHA** (`tauri-apps/tauri-action@v0` `release.yml:215,230`,
  `dtolnay/rust-toolchain@stable`, `Swatinem/rust-cache@v2`, `android-actions/setup-android@v3` `mobile.yml:52`,
  `actions/*@v4|v7`). The release job has `contents: write` and signing secrets. Dependabot (already configured) will bump SHAs.
- **P2-2 Reproducibility:** add `--locked` to `cargo build`/`clippy`/`test` (`release.yml:142`, `ci.yml:56-60`); pin a
  toolchain (`rust-toolchain.toml`) and declare `rust-version` in `[workspace.package]` (≥ 1.90 if the updater lands).
- **P2-3 Secrets via `$GITHUB_ENV`:** `release.yml:177-189` writes `APPLE_ID/APPLE_PASSWORD/APPLE_TEAM_ID` into
  `GITHUB_ENV` for all later steps. Pass them in the signed `tauri-action` step's `env:` instead and delete the export step.
- **P2-4 Supply-chain for artifacts:** add `actions/attest-build-provenance` for archives/bundles and sign `SHA256SUMS`
  (minisign or cosign keyless); optionally verify in the installers. Mention `gh attestation verify` in README.
- **P2-5 CI coverage gaps:** `apps/mobile/src-tauri` is a workspace member but never compiled (add
  `cargo check -p bozkurt-mobile` on Linux); add a job running actionlint, shellcheck, PSScriptAnalyzer for `install.ps1`,
  and a Markdown link check; `npm run lint --workspaces --if-present` (`ci.yml:80-81`) is a no-op because no package defines
  `lint` — add ESLint or drop the step; add a smoke test that runs `scripts/install.sh --target …` against a mirror
  (`BOZKURT_BASE_URL`) built from the job's own archive.
- **P2-6 Linux deps:** `ci.yml:22` and `release.yml:131` install `libappindicator3-dev`; Tauri v2's prerequisites and
  `apps/desktop/README.md` use `libayatana-appindicator3-dev` (the former is gone from newer Ubuntu). Align on ayatana.
- **P2-7 Runner/platform notes:** `ubuntu-22.04-arm` (`release.yml:80`) is available to public repositories only — a private
  repo leaves the job queued. Linux binaries built on 22.04 need glibc ≥ 2.35 and won't run on musl/Alpine; `install.sh:64-65`
  maps every Linux to `*-unknown-linux-gnu`. Document the baseline (or add a `*-unknown-linux-musl` CLI build).
- **P2-8 `install.sh` shellcheck:** SC2088 at `:154-156` (`"~/.zshrc"` is display-only — use `$HOME` or `# shellcheck disable=SC2088`),
  SC2086 at `:144`.
- **P2-9 Tauri `copyright` holds the licence expression** (`apps/desktop/src-tauri/tauri.conf.json:34`,
  `apps/mobile/src-tauri/tauri.conf.json:23`). Use `"Copyright © 2026 Cuma Bozkurt and contributors"`.
- **P2-10 Capabilities/CSP tightening:** desktop `capabilities/default.json:9` grants `opener:default` *and* an explicit
  `opener:allow-open-url` for `http://*`, `https://*`, `vscode://*` (`:10-13`) — drop `opener:default` and keep only the
  explicit scope (consider limiting `http(s)` to `*.<tld>` + docs URLs). Desktop CSP `connect-src` allows any localhost port
  (`tauri.conf.json:27`) — acceptable given configurable `api_port`; mobile CSP (`apps/mobile/…/tauri.conf.json:16`) allows
  all `http: ws: https: wss:` — needed for LAN IPs, but `https:`/`wss:` can be dropped if the daemon never serves TLS.
  `withGlobalTauri: false`, `script-src 'self'` and the narrow `dialog:allow-open` are good.
- **P2-11 Single source of version:** remove `"version"` from both `tauri.conf.json` (Tauri v2 then uses the Cargo package
  version, which already inherits `[workspace.package]`), leaving Cargo + package.json as the only bump points.
- **P2-12 License headers:** 0 of 110 source files (`*.rs *.ts *.tsx *.mjs *.sh *.ps1`) carry
  `SPDX-License-Identifier: MIT OR Apache-2.0`. Optional for dual-licensed Rust projects, but cheap to add and to enforce.
- **P2-13 Mobile toolchain:** `mobile.yml:40` installs `platforms;android-34`; confirm the generated template's
  compile/target SDK meets Google Play's current target-API requirement before uploading. `mobile.yml:116-117` signs AAB with
  `jarsigner` (fine for Play upload key); document that Play App Signing is expected.
- **P2-14 CLI help text:** many positionals/options show no description (`[SERVICE]`, `[SITE]`, `--php <VER>`, `--node <VER>`,
  `--https`, `--wildcard`, `-a/--available`, `site start|stop|restart|rm|list|info`, `config get`). Add one-line `help` for
  each so `bz --help` can be the source for CLI.md (P1-5).
- **P2-15 ARCHITECTURE layout nits:** home layout omits `cache/`, `tmp/`, `bozkurtd.pid`, `caddy/bin`, `caddy/data`,
  `caddy/snippets` (`:64-77`); repo layout says `docs/` contains CONTRIBUTING (`:59`, it is at the root) and that
  `.github/workflows` release includes mobile builds (`:60`, they are a separate `mobile.yml`).
- **P2-16 Comparison consistency:** README table (`README.md:263-271`) and COMPARISON tables agree except the MCP tool count
  (P1-6). Re-fetched 2026-10-09 and confirmed: Herd Pro $99/yr licence, Teams $299 for 10, "no plans for a Linux version";
  Laragon $49/yr or $149 lifetime commercial, non-commercial use free with popup; Kettle Code macOS 13+ only, 18 MCP tools,
  "free for a limited time", `~/.kettle`, DNS on 5390; ServBay macOS 12+/Windows 10+, Free = 5 sites/5 DNS records,
  Pro $9.9/mo or $99/yr shown at $59, Team $39.9/mo, PHP 5.6–8.5, Node 12–24, PostgreSQL 10–17, mobile app "coming soon".
  One ambiguity: herd.laravel.com labels the nvm/Node section "Pro Feature" while the free tier lists "Node.js" — mark
  Herd's Node cell "✅ (nvm; tier ❓)". Update the "fetched" date in `COMPARISON.md:4,92` and `README.md:261` on release day.
- **P2-17 Turkish (overall natural, idiomatic Turkish — good quality):**
  - Terminology clash: "servis" is used both for the daemon and for MariaDB/Redis/etc. — `README.tr.md:31` ("yerel servise
    (`bozkurtd`)"), `:35`, `docs/USER_GUIDE.tr.md:40` ("**Servis (daemon).**") next to `:55` ("**Servis.**"), `:371`
    ("servis API'si"). Use "arka plan hizmeti (`bozkurtd`)" or keep "daemon" for `bozkurtd`, and "servis" only for services.
  - `README.tr.md:85` "tamamen Türkçe arayüz" — English says "English and Turkish UI"; use "Türkçe ve İngilizce arayüz".
  - `README.tr.md:56` "ön vekil sunucu" is unusual; prefer "önde çalışan ters vekil sunucu (reverse proxy)".
  - `README.tr.md:126` "--target ÜÇLÜ" → "--target HEDEF (ör. aarch64-apple-darwin)"; `SAHİP/DEPO` is fine.
  - `README.tr.md:65` "sahte SMTP sunucusu" → "e-posta yakalayıcı (SMTP)" reads more natural.

---

## 1.0.0 release checklist

**Decide first (irreversible)**
- [ ] Final GitHub slug and replace every `OWNER/…` (P0-4); real security + conduct contact addresses.
- [ ] Final bundle identifiers for desktop and mobile (P1-3).
- [ ] Generate and back up the Tauri updater key pair; add `TAURI_SIGNING_PRIVATE_KEY(_PASSWORD)` secrets (P1-4).

**Code / config**
- [ ] Local API: Content-Type, Origin and Host checks + tests (P0-3).
- [ ] Sidecar overlay lists all intended binaries and `release.yml` passes `--config src-tauri/tauri.sidecar.conf.json`; bundle-contents assertion step (P0-1).
- [ ] `mobile.yml` runs `patch-android.mjs` (with a hard check); iOS job builds via `tauri ios build` (P0-2, P1-1).
- [ ] Updater plugin wired (Rust-side), `createUpdaterArtifacts: true`, `latest.json` endpoint (P1-4).
- [ ] "Install command-line tools" path for desktop users (P1-13).
- [ ] Delete `apps/*/src-tauri/STUB.md` (P1-10); generate `THIRD_PARTY_LICENSES.md` and ship it (P1-11).
- [ ] Bump all versions to 1.0.0, regenerate both lockfiles; tag check fails on mismatch (P0-6).

**Docs**
- [ ] Remove all "placeholder / unreleased / contract" wording; release badge; SECURITY supported-versions (P0-5).
- [ ] CLI.md, MCP.md, API.md regenerated/updated against the binaries (P1-5…P1-7); "55 tools" everywhere.
- [ ] ARCHITECTURE accuracy (download verification, elevation, shims) and original wording for Kettle-like phrases (P1-8, P1-9).
- [ ] CHANGELOG `[1.0.0] - <date>` with compare links; ROADMAP re-based on 1.0 (P1-12).
- [ ] Turkish terminology pass on README.tr / USER_GUIDE.tr (P2-17); refresh COMPARISON fetch date (P2-16).

**CI/CD dry run**
- [ ] Push a `v1.0.0-rc.1` tag (prerelease) → all 5 desktop targets, CLI archives, `latest.json`, `.sig`, `SHA256SUMS`, APK/AAB/IPA attached to the draft.
- [ ] Install each bundle on a clean VM/Mac/PC: app starts `bozkurtd` from the bundle, tray shows "running", `bz --version` = 1.0.0.
- [ ] `curl … install.sh | sh` and `irm … install.ps1 | iex` against the RC (with `--version v1.0.0-rc.1`), checksum OK.
- [ ] Updater: install rc.1, publish rc.2, confirm in-app update on macOS, Windows (passive) and Linux AppImage.
- [ ] Android: release APK pairs with a desktop over LAN (cleartext works); iOS IPA (if signing secrets set) installs via TestFlight.
- [ ] macOS notarization stapled (`spctl -a -vv`), Windows Authenticode valid (`Get-AuthenticodeSignature`), if secrets exist; otherwise README warns about Gatekeeper/SmartScreen.

**Publish**
- [ ] Tag `v1.0.0` on the release commit; wait for `release.yml` + `mobile.yml`; review the draft notes; publish (makes `/releases/latest/` and the updater endpoint live).
- [ ] Verify the README one-liners against the published release from a clean machine.
- [ ] Open the `[Unreleased]` section in CHANGELOG for 1.0.1.

## Not verified here (needs a real runner or a decision)
- Whether `tauri-action@v0` forwards `args` correctly through `tauriScript: npm run tauri` (it is the documented pattern; confirm in the RC run).
- The iOS `xcodebuild` failure in P1-1 is inferred from how Tauri's Xcode build phase works, not reproduced.
- P0-3 is from code reading; no live exploit was attempted against a running daemon.
- `cargo`-based checks (clippy, tests, `cargo check` of the Tauri crates) were intentionally not run.

Scratch artifacts used for this audit (not part of the repo): `/workspace/projects/6662349e-630d-40ce-b3ae-b8ca0a4dcf8b/work/docs-audit/`
(`bz-all-help.txt`, `mcp-tools.json`, `routes.txt`, `url-status.txt`, `actionlint.txt`, `sc.txt`, `linkcheck.py`, `cmdcheck.py`, `mcp_list.py`).
