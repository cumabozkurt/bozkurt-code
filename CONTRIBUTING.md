# Contributing to BOZKURT CODE

Thanks for helping! This guide covers the repository layout, local setup, the checks CI runs, and how releases
are built and signed.

By taking part you agree to the [Code of Conduct](CODE_OF_CONDUCT.md). Report security issues privately as described
in [SECURITY.md](SECURITY.md), never in a public issue.

## Ground rules

- **Original work only.** BOZKURT CODE is an independent implementation. Do **not** copy code, assets, text, configs or
  screenshots from other products, and do not decompile, disassemble or inspect their binaries or bundles. Write docs and
  UI text in your own words.
- **API first.** [`docs/API.md`](docs/API.md) is the reference for the interface between the daemon and every client (GUI, CLI, MCP, mobile).
  If a change alters behaviour visible through the API, update `docs/API.md` (and `docs/CLI.md` / `docs/MCP.md` if
  affected) in the same PR.
- **Core rules** from [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) are not optional:
  - Verify every download against the upstream SHA-256 where the source publishes one (HTTPS-only otherwise, and say so
    in the catalog), and extract it atomically.
  - Back up anything written outside `$BOZKURT_HOME` to `backups/<stamp>/` first.
  - Elevate only through named `bz-net` plans that the caller confirms.
  - Run long operations as jobs that emit events.
- **License.** Contributions are dual-licensed MIT OR Apache-2.0 (see [README](README.md#license)). No CLA. Please sign
  off your commits (`git commit -s`, [DCO](https://developercertificate.org/)).

## Repository layout

```
crates/bz-core     runtimes, services, sites, supervisor, detection, Caddyfile, catalogs, config/state
crates/bz-net      DNS responder, local CA + leaf certs, trust stores, OS resolver plans, ports, hosts
crates/bz-daemon   bin `bozkurtd`: HTTP API + WebSocket events      (in progress)
crates/bz-cli      bin `bz`                                          (in progress)
crates/bz-mcp      bin `bozkurt-mcp`, stdio MCP server               (in progress)
packages/ui        shared React 18 + TS UI and typed API client (`@bozkurt/ui`)
apps/desktop       Tauri 2 desktop shell (`@bozkurt/desktop`)
apps/mobile        Tauri 2 mobile shell (`@bozkurt/mobile`)
scripts/           install.sh, install.ps1
docs/              architecture, API, user guides, CLI, MCP, comparison, roadmap
```

## Prerequisites

- **Rust** stable (`rustup`), with `rustfmt` and `clippy`.
- **Node.js ≥ 20** (CI uses 22) and npm. The repo is an npm workspace.
- **Desktop shell, Linux only:** the Tauri/WebKitGTK system packages. CI installs:
  `libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf libxdo-dev libssl-dev build-essential curl wget file`
  (add `rpm` for building `.rpm` bundles).
- **Mobile:**
  - Android: JDK 17, Android SDK platform 34, build-tools 34.0.0, NDK 27.2.12479018.
  - iOS: macOS and Xcode.

## Building and running

```sh
# Rust: by default only the five library/binary crates build (see `default-members` in Cargo.toml)
cargo build
cargo test

# UI against the in-memory mock backend (no daemon needed)
npm ci
npm run dev:mock                          # desktop UI in the browser, mock data
npm run dev -w @bozkurt/ui                # UI playground → http://localhost:5173/?mock

# Desktop app (needs bozkurtd running or on PATH). A release-built daemon accepts the dev-server
# origins only when started with BOZKURT_DEV=1 (debug builds always accept them).
BOZKURT_DEV=1 bz daemon run &
npm run tauri -w @bozkurt/desktop -- dev
VITE_BOZKURT_MOCK=1 npm run tauri -w @bozkurt/desktop -- dev

# Desktop bundle with sidecars (copy target/release/{bozkurtd,bz,bozkurt-mcp} to
# apps/desktop/src-tauri/binaries/<name>-<target-triple>[.exe] first; see binaries/README.md)
npm run tauri -w @bozkurt/desktop -- build --config src-tauri/tauri.sidecar.conf.json
```

Use a throwaway home while developing so you don't touch your real environment:

```sh
export BOZKURT_HOME="$PWD/.bozkurt-dev"
```

Mobile builds are described in [`apps/mobile/README.md`](apps/mobile/README.md). For a release APK that can reach the
LAN daemon over plain HTTP, run `node apps/mobile/scripts/patch-android.mjs` after `android init`.

## Checks (what CI runs)

`.github/workflows/ci.yml` runs on every push to `main` and on every pull request:

| Job | Command |
|---|---|
| rustfmt | `cargo fmt --all -- --check` |
| clippy + test (Linux, macOS, Windows) | `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked --all-targets`, `cargo test --locked --doc` (workspace `default-members`, no Tauri crates) |
| node | `npm ci`, then `npm run typecheck --workspaces --if-present` and `npm run build --workspaces --if-present` |
| desktop-check | builds `apps/desktop` and `apps/mobile`, stubs the sidecars, `cargo check --locked -p bozkurt-desktop` (with `tauri.sidecar.conf.json`) and `cargo check --locked -p bozkurt-mobile` |
| repo-lint | `node scripts/check-versions.mjs`, actionlint, shellcheck, PSScriptAnalyzer, JSON config parse, `python3 scripts/check-md-links.py` |

`.github/workflows/pages.yml` publishes the landing page in `site/` to GitHub Pages on pushes to `main` that touch it
(see [site/README.md](site/README.md)).

Run the same before pushing:

```sh
cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test --all-targets
npm run typecheck && npm run build
```

### Tests

- Catalog parsers in `bz-core` are pure functions tested against captured upstream JSON in `crates/bz-core/tests/fixtures/`.
  If an upstream format changes, refresh the fixture and keep the old one only if both shapes must be supported.
- Caddyfile output is snapshot-tested (`crates/bz-core/tests/snapshots/*.Caddyfile`).
- `bz-net` has end-to-end tests for the DNS responder and TLS (`crates/bz-net/tests/`). Tests must never modify the real
  OS resolver, hosts file or trust stores. Use the plan/dry-run APIs and temp directories.
- UI: every English string needs a Turkish translation. `tr` must cover every `en` key, and `npm run typecheck` fails otherwise.

## Pull requests

1. Open an issue first for anything larger than a bug fix, so we can agree on the API shape.
2. Keep PRs focused. Use [Conventional Commits](https://www.conventionalcommits.org/) style subjects
   (`feat(core): …`, `fix(net): …`, `docs: …`).
3. Add or update tests and docs. Add a line to [CHANGELOG.md](CHANGELOG.md) under the heading for the next version at the
   top (open one, e.g. `## [1.0.1]`, if the top entry is already released).
4. For UI changes, attach before/after screenshots. To update `docs/screenshots/`, use the mock backend so no real paths
   or hostnames leak.

## Releases

Pushing a tag `v*` (or running **Release** manually with an existing tag) runs `.github/workflows/release.yml`:

1. It creates or reuses a **draft** GitHub Release and **fails** if the tag doesn't match the version in `Cargo.toml`,
   the npm packages and both `tauri.conf.json` files (`scripts/check-versions.mjs`).
2. It builds `bozkurtd`, `bz` and `bozkurt-mcp` for `x86_64/aarch64-unknown-linux-gnu`, `x86_64/aarch64-apple-darwin` and
   `x86_64-pc-windows-msvc`. Each set is uploaded as a CLI archive (`bozkurt-cli-<target>.tar.gz|zip`, which is what the install
   scripts download) and staged as Tauri sidecars.
3. `tauri-action` builds the desktop bundles (dmg/app, msi/nsis, deb/rpm/AppImage) with the
   `tauri.sidecar.conf.json` and `tauri.release.conf.json` overlays, so every bundle contains `bozkurtd`, `bz` and
   `bozkurt-mcp` plus `THIRD_PARTY_LICENSES.md`. A post-build step fails the job if a sidecar is missing. With the updater
   key set, it also uploads `.sig` files and `latest.json`.
4. It computes `SHA256SUMS` over all assets, uploads it, and records build-provenance attestations.
5. A maintainer reviews the draft and publishes it by hand. Publishing makes `/releases/latest/` (and so the updater
   endpoint and the install one-liners) point at it.

The same tag also runs `.github/workflows/mobile.yml`, which attaches the Android APK/AAB and the iOS build to the same
release and recomputes `SHA256SUMS`. To (re)attach mobile builds to an existing release later, run
`gh workflow run mobile.yml -f release_tag=vX.Y.Z` (it waits for that tag's release workflow first).

### Release signing secrets

Signing is **optional**. Every signing step is skipped when its secrets are missing, so forks and CI still produce
unsigned builds. Set these under *Settings → Secrets and variables → Actions*:

| Secret | Workflow | Used for | Format / notes |
|---|---|---|---|
| `GITHUB_TOKEN` | release | Creating the draft release, uploading assets | Provided automatically by GitHub. The workflow requests `contents: write`. |
| `APPLE_CERTIFICATE` | release | macOS code signing (enables the signed `tauri-action` step) | Base64 of a *Developer ID Application* `.p12` |
| `APPLE_CERTIFICATE_PASSWORD` | release | macOS code signing | Password of the `.p12` |
| `APPLE_SIGNING_IDENTITY` | release | macOS code signing | e.g. `Developer ID Application: Name (TEAMID)` |
| `KEYCHAIN_PASSWORD` | release, mobile | Temporary build keychain | Any random string. Optional in mobile (a UUID is generated when it is empty). |
| `APPLE_ID` | release | Notarization | Apple ID email. Notarization runs only when `APPLE_CERTIFICATE` **and** all three notarization secrets are set. |
| `APPLE_PASSWORD` | release | Notarization | App-specific password for that Apple ID |
| `APPLE_TEAM_ID` | release | Notarization | 10-character team ID |
| `WINDOWS_CERTIFICATE` | release | Authenticode signing of msi/nsis | Base64 of a `.pfx`. It is imported to `CurrentUser\My`, and its thumbprint is passed to Tauri with SHA-256 digest and the DigiCert timestamp server. |
| `WINDOWS_CERTIFICATE_PASSWORD` | release | Authenticode signing | Password of the `.pfx` |
| `ANDROID_KEYSTORE` | mobile | Signing the APK (zipalign + apksigner) and AAB (jarsigner) | Base64 of a `.jks` keystore. Without it the APK/AAB are uploaded unsigned, plus a `*-sideload.apk` signed with a throwaway key generated on the runner (installable by sideloading; a later build signed with a different key needs an uninstall first, so set a real keystore before shipping updates). |
| `ANDROID_KEYSTORE_PASSWORD` | mobile | Android signing | Keystore password |
| `ANDROID_KEY_ALIAS` | mobile | Android signing | Key alias inside the keystore |
| `ANDROID_KEY_PASSWORD` | mobile | Android signing | Key password |
| `IOS_CERTIFICATE` | mobile | Signed iOS IPA | Base64 of an Apple distribution/development `.p12`. All three of `IOS_CERTIFICATE`, `IOS_MOBILE_PROVISION` and `APPLE_DEVELOPMENT_TEAM` are required, otherwise an unsigned **simulator** build is produced (without `APPLE_DEVELOPMENT_TEAM` a placeholder team id is used only to generate the Xcode project; simulator builds are not signed). |
| `IOS_CERTIFICATE_PASSWORD` | mobile | Signed iOS IPA | Password of the `.p12` |
| `IOS_MOBILE_PROVISION` | mobile | Signed iOS IPA | Base64 of the `.mobileprovision` |
| `APPLE_DEVELOPMENT_TEAM` | mobile | Signed IPA (and the real team id in the generated Xcode project) | Team ID |

| `TAURI_SIGNING_PRIVATE_KEY` | release | Signing updater artifacts (`.sig`, `latest.json`) | Private key from `npm run tauri -w @bozkurt/desktop -- signer generate -w ~/.tauri/bozkurt.key`. Back it up offline: losing it means existing installs can no longer verify updates. |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | release | Updater signing | Password of that key |

Repository **variables** (not secrets):

- `TAURI_UPDATER_PUBKEY`: contents of the matching `.pub` file. It is written into the bundled updater config; the release
  fails if the private key is set without it.
- `IOS_EXPORT_METHOD`: export method for `tauri ios build`, default `app-store-connect`.

To create a base64 value: `base64 -i cert.p12 | pbcopy` (macOS) or `base64 -w0 cert.p12` (Linux), or
`[Convert]::ToBase64String([IO.File]::ReadAllBytes('cert.pfx'))` in PowerShell.

> Updates: the release publishes signed updater artifacts and `latest.json` at
> `https://github.com/cumabozkurt/bozkurt-code/releases/latest/download/latest.json`. The desktop app registers
> `tauri-plugin-updater` only when its config is present (`tauri.release.conf.json`, i.e. release builds with a signing
> key), checks at startup and emits `bozkurt://update-available`; dev builds and forks skip it. On Linux only the AppImage can update
> itself; `.deb`/`.rpm` users update through the package, CLI-only installs by re-running the install script.

## Translations

The UI ships in English and Turkish (`packages/ui/src/i18n/en.ts`, `tr.ts`). Docs come in pairs:
`README.md`/`README.tr.md` and `docs/USER_GUIDE.md`/`docs/USER_GUIDE.tr.md`. If you change one of a pair, update the other
too, or open an issue tagged `translation`. Turkish text should read as natural Turkish, not word-for-word translation.

## Questions

Open a [discussion](https://github.com/cumabozkurt/bozkurt-code/discussions) or an issue.
