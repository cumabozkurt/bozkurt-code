# Daemon follow-ups from the UI audit

Date: 2026-10-09. Source: `docs/audit/UI_AUDIT.md`. These are the findings that need a change in the Rust crates. The UI fix pass (`packages/ui`, `apps/*` web parts) did **not** touch any crate. For each item the UI is already robust on its own, as described under "UI today", so each change below is an improvement, not a blocker for the UI.

Everything below targets `crates/` (bz-core, bz-daemon), plus one desktop-shell item (`apps/desktop/src-tauri/src/lib.rs`).

---

## D1 · Reject invalid ports in `Config::validate` (P0-1, P0-2)

- **Where:**
  - `crates/bz-core/src/config.rs` `Config::validate` (~L88-105). It is called by `Engine::patch_config` and on load.
  - The same file defines `RemoteConfig`.
- **Spec:**
  - Every port (`http_port`, `https_port`, `dns_port`, `api_port`, `remote.port`) must be in `1..=65535`. `0` is an error, not "unset".
  - `http_port`, `https_port`, `dns_port`, `api_port` and `remote.port` must be pairwise distinct.
  - Error: `invalid input: <key> must be between 1 and 65535`, or `invalid input: <key> (<n>) is already used by <other_key>` → HTTP 400 `{"error":{"code":"invalid_input",…}}`.
  - `PATCH /config` with `api_port` should be **refused on the remote listener** (scope `Remote`), with `403 forbidden: api_port can only be changed locally`. Today a paired phone can move the API port and strand the desktop.
  - Optional: refuse `api_port` changes over HTTP entirely (CLI / config.toml only). The UI no longer offers it.
- **Test:**
  - `PATCH /config {"https_port":0}` → 400.
  - `{"http_port":443,"https_port":443}` → 400.
  - Both leave `config.toml` unchanged.
- **UI today:**
  - Port drafts are kept as strings and validated client-side (integer `1–65535`, distinct, not equal to `api_port` or `remote.port`). Save is disabled while anything is invalid, and only changed keys are sent, so a port of `0` is never sent.
  - `api_port` is shown read-only with a hint.
  - The whole General section is read-only in remote (paired phone) mode.
  - Overview shows a red port badge when the daemon reports a port outside `1–65535`.

## D2 · Desktop shell must read `api_port` from `config.toml` (P0-2)

- **Where:**
  - `apps/desktop/src-tauri/src/lib.rs` (Rust; not edited in the UI pass, no cargo run).
  - `packages/ui/src/api/client.ts` `DEFAULT_BASE_URL`.
- **Spec:**
  - At startup the shell reads `<BOZKURT_HOME>/config.toml` `api_port` the same way `bz-cli/src/daemon.rs:24` does.
  - It exposes the port to the webview, for example via `window.__BOZKURT_API__ = "http://127.0.0.1:<port>/api/v1"` in an init script, or via a `get_api_base` command.
  - The UI can then pass it as `<BozkurtApp baseUrl=…>`. That prop now exists, so the host only has to provide the value.
- **UI today:** the UI never edits `api_port`, so the mismatch can only happen through the CLI or a hand edit.

## D3 · Emit `service.status` for DNS and after `PATCH /services/{name}` (P1-1)

- **Where:**
  - `crates/bz-core/src/services.rs:88`: the built-in DNS service is not supervised, so it emits no event.
  - `crates/bz-core/src/engine.rs` `patch_service` (~L800).
- **Spec:**
  - After `POST /services/dns/{start|stop|restart}`, `POST /services/all/{action}` (for its DNS part), and `PATCH /services/{name}`, publish `{"type":"service.status","service":<Service>}` with the same body the endpoint returns.
- **UI today:**
  - The UI upserts the `Service` / `Service[]` returned by every action and PATCH (`store.applyService`), so the cards update without events.
  - Other clients (CLI, a second window, the phone) still only learn about the change from the event.

## D4 · Doctor: a site whose runtime is missing (P1-5)

- **Where:** `crates/bz-core/src/engine.rs` `doctor()` (~L1360).
- **Spec:** add a check.
  - `id: "site_runtimes"`, `title: "Sites have their runtime"`.
  - For each site, apply these rules:
    - `kind == php`: needs any installed PHP, or exactly `site.php` if set. The error today is `PHP latest is not installed`.
    - `kind == app` with a Node command (or `site.node` set): needs Node.
  - When something is missing:
    - `status: "fail"`.
    - `detail`: `"<domain>[, <domain>…] need PHP, none installed"`. When exactly one site is affected, use `"<domain> needs PHP <ver>, none installed"`.
    - `fix: "runtimes.php.install"` (or `runtimes.node.install`).
  - When nothing is missing: `status: "ok"`, `detail: "all sites have their runtime"`.
  - Also register `runtimes.<kind>.install` in `machine_fix`/`doctor_fix`. It installs the recommended version (PHP: newest `stable`; Node: newest `lts`) as a job titled `Install <kind> <version>`.
- **UI today:**
  - Creating or editing a PHP site with no PHP installed shows an inline warning with **Install PHP** (installs the recommended stable version as a job) and **Choose version…** (Runtimes page). Node apps get the same warning for Node.
  - The Doctor UI already maps `runtimes.<kind>.install` and the CLI text `bz runtime install …` to the Runtimes page.
  - The mock backend implements this check, using the exact strings above. `lib/daemonText.ts` translates them.

## D5 · Trust / DNS jobs must not report success when nothing changed (P1-8)

- **Where:**
  - `crates/bz-daemon/src/api.rs` `trust_job` (~L458-480).
  - `crates/bz-core/src/engine.rs` `configure_dns` (~L1223-1250, the `"Local CA trusted"` and `".{tld} now resolves locally"` messages).
- **Spec:**
  - After `trust()` / `configure()` returns, re-run `ca_status()` / `dns_status()`.
  - If `ca_trusted` / `configured` is still false, **fail** the job, for example with `"the CA was added but is still not trusted (no system trust store / missing privileges)"` or `".test is still not routed to 127.0.0.1 (needs administrator rights?)"`.
  - Apply the same rule to `untrust` / `unconfigure`.
- **UI today:**
  - After every trust or DNS job (Doctor, Quick setup, palette, TLD change), the UI suppresses the generic "finished" toast. It then re-fetches `GET /tls` / `GET /dns` and shows either a verified success or a warning: "The trust job finished, but the local CA is still not trusted…".
  - The wizard stays on the step with a retry button.

## D6 · Doctor fix ids for installs (P1-7)

- **Where:** `crates/bz-daemon/src/api.rs` `machine_fix`, `doctor_fix`.
- **Spec:**
  - Map `bz service install <name>` to `services.<name>.install`. `POST /doctor/fix/services.caddy.install` should then return a 202 job (`Install caddy`).
  - Replace the non-runnable hint `bz runtime install php 8.4  (or node lts)` with a machine id (`runtimes.php.install`, see D4), or with a runnable command.
  - Return `fix: null` for purely informational hints such as `bz service logs caddy`. Alternatively, add a separate `hint` field.
  - Longer term: make `DoctorCheck` carry `fix_kind: "machine" | "command" | "manual"` so clients don't have to guess.
- **UI today:**
  - Client-side mapping: `bz service install X` → install job; `bz runtime install …` → Runtimes page; `bz service logs X` → Services page.
  - Any other dotted machine id is sent to `POST /doctor/fix/{id}`.
  - Free-form text gets a secondary **Copy command** button, with the text shown inline, instead of a primary "Fix".

## D7 · Stable, localisable texts (P2-13, P2-21)

- **Where:** doctor checks (`engine.rs` `doctor()`), job titles and messages (`jobs.spawn(...)`, `h.progress(..., Some(msg))`), and errors (`ApiErr`).
- **Spec:**
  - `DoctorCheck` already has a stable `id`. Add `code` plus `params` to `detail`, for example `{"code":"dns.not_routed","params":{"tld":"test"}}`, and add `kind` plus `params` to `Job`, for example `{"kind":"runtime.install","params":{"kind":"node","version":"22.23.3"}}`.
  - Error bodies already have `code`. Keep the `message` free of the `not found: ` / `invalid input: ` prefix; that prefix duplicates `code`.
  - Alternative: honour `Accept-Language: tr` server-side.
- **UI today:**
  - `packages/ui/src/lib/daemonText.ts` translates the current English strings by pattern (check titles by id; details, job titles and messages, and known errors by regex). Unknown text passes through unchanged.
  - **Any wording change in the daemon silently drops the translation.** Keep the strings stable, or move to codes.

## D8 · `GET /remote` address order (P1-11)

- **Where:** `crates/bz-daemon/src/remote.rs` `lan_addresses()` (~L282): it currently sorts by IP.
- **Spec:**
  - List the interface carrying the default route first.
  - Drop docker/bridge (`docker*`, `br-*`, `veth*`, `virbr*`), VPN (`tun*`, `utun*`, `wg*`, `tailscale*`), and link-local addresses, or put them last.
  - Optional: return `[{ip, iface, kind}]` in a new `interfaces` field, and keep `addresses` for compatibility.
- **UI today:**
  - Addresses are ranked client-side: 192.168/16, then 172.16/12 (except docker's 172.17/16), then 10/8, then others.
  - When there are several, the user picks the QR address from a select; the first is marked "recommended".
  - `RemoteInfo.running` is shown as "Listening on :port" / "Not listening". Pairing is disabled when the listener is down.

## D9 · CORS for development origins (P1-13)

- **Where:** `crates/bz-daemon/src/api.rs:29-35` `CORS_ORIGINS`.
- **Spec:**
  - In debug builds (`cfg!(debug_assertions)`), also allow `http://localhost:5173`, `http://127.0.0.1:5173`, `http://127.0.0.1:1420` and `http://127.0.0.1:1430`.
  - Release builds keep the strict list.
- **UI today:** the `packages/ui` playground now proxies `/api/v1` (REST and WebSocket) to the daemon via vite (`BOZKURT_API_TARGET`), so it is same-origin and needs no CORS. `apps/desktop` dev (`localhost:1420`) is already allowed.

## D10 · Small contract items

- **Share state:**
  - `GET /share/{id}` exists (`{site_id, url|null}`) and is now used by the UI. Please add it to `docs/API.md` (it is missing from the endpoint table).
  - Also consider listing every share in `GET /sites`, or adding `GET /share`, so the UI doesn't need one request per site on attach.
- **Uptime:** add `started_at` (RFC 3339) to `Service`, so clients don't have to extrapolate `uptime_secs` (P2-9). The UI ticks it locally for now.
- **Extract progress:** report progress during archive extraction, or at least a heartbeat message (P2-10). The UI shows an indeterminate bar for the "Extracting" / "Opening tunnel" phases.
- **Static framework:** don't set `framework: "static"` for static sites (P2-8). The UI hides a framework that equals the kind.
- **Mailpit:** add `ui_port: 8025` to the Mailpit `Service`, or a generic `web_url` (P1-10). The UI hard-codes 8025 (`MAILPIT_UI_PORT`) and labels the button "Open web UI".
- **WebSocket token (P2-19):** the remote token travels in `?token=`. Consider a short-lived ticket (`POST /events/ticket` → `{ticket}` valid for 30 s, single use).
- **Workspace version:** the root `Cargo.toml` `[workspace.package] version` (used by both Tauri crates via `version.workspace = true`) should become `1.0.0`, to match `tauri.conf.json` and the package.json files. These were bumped in the UI pass; `Cargo.toml` was left to the crates owner.
