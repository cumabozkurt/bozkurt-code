# BOZKURT CODE — UI audit against the real daemon

Date: 2026-10-09 · Auditor: automated run (playwright-core + system Chromium 154 + axe-core 4.x) · Repo: `main` @ `397f5cd`

## Scope and setup

- **Daemon:** debug `bozkurtd` (`work/target-daemon/debug/bozkurtd --foreground`), isolated `BOZKURT_HOME=work/ui-audit/home`, with `config.toml` set to `api_port=17878`, `http_port=18080`, `https_port=18443`, `dns_port=15390`, `remote.port=17879`, `autostart=[]`. The daemon was stopped at the end (it was killed during the offline test). No caddy or cloudflared processes were left running.
- **UI under test:**
  - `apps/desktop` dev server on `:1420` (StrictMode/dev), with `VITE_BOZKURT_API=http://127.0.0.1:17878/api/v1`.
  - A **production** build of `apps/desktop`, served on `:1430`. This is the main target, because it behaves like the shipped app. `:1430` is one of the daemon's CORS origins.
  - A production build of `apps/mobile`, served on `:1430`, used for the pairing flow.
- **What was exercised for real:**
  - Overview, the Quick setup wizard (every step), and Runtimes. Node **22.23.3 was installed for real**: the job ran about 64 s and the version auto-activated.
  - Caddy: installed for real (2.11.7), then start, stop and restart, the logs drawer, and the autostart switch.
  - DNS service start, and start all.
  - Sites: created a PHP site and a static site from temporary dirs. Then site detail, an alias edit, env vars, the logs tab, Open in browser, Share publicly (a real trycloudflare tunnel, stopped afterwards), the HTTPS toggle, and delete with confirm.
  - The global Logs page (live stream, search, pause).
  - Doctor, including all three kinds of fixes.
  - Settings: config patch, an invalid TLD, an empty port, theme, language, enabling remote, a pairing code with QR, import scan, and disabling remote.
  - Mobile Connect screen: a wrong code, then **real pairing over the LAN listener**.
  - Jobs drawer, command palette, the shortcuts dialog, the offline daemon (both mid-session and on a fresh load), TR/EN, light/dark, and 1280 px and 390 px widths.
- **Field-name diff:** every GET endpoint was dumped with curl and its keys compared with `packages/ui/src/api/types.ts`. The results are in §4.
- **Evidence:** screenshots are in `docs/audit/ui/`. The raw run logs (JSON, kept out of git) are in `work/ui-audit/*.json`.

> Note on timestamps: in this sandbox, tool output re-renders ISO timestamps as `Fri 2026-10-09 9:10 PM (UTC+03:00)`. The daemon itself emits RFC 3339 (`bz-core/src/jobs.rs:13`). This is **not** a bug, and it is not reported below.

## Summary

| Priority | Count |
|---|---|
| P0 | 2 |
| P1 | 13 |
| P2 | 22 |

### Top 10
1. **P0-1**: An emptied port field in Settings is saved as `0`, and the daemon accepts it (`https_port: 0`). The config is silently corrupted.
2. **P0-2**: Changing the **API port** in Settings (local, or from a paired phone) makes the UI lose the daemon after the next restart. Nothing warns about it, and the daemon accepts `api_port: 0`.
3. **P1-1**: Service actions and autostart: the UI ignores the `Service` returned by the API, so DNS start and the autostart switch never update on screen.
4. **P1-2**: The service log "Live tail" never shows new lines. The UI filters on `source === "caddy"`, but the daemon sends `service:caddy`.
5. **P1-3**: In dev/StrictMode, the event stream is closed for good. The result is a permanent "Daemon not reachable" banner, no job progress and no live logs.
6. **P1-5**: A PHP site can be created with **no PHP installed**. There is no warning, the site returns `502 "PHP latest is not installed"`, and Doctor still says "Runtimes OK".
7. **P1-4**: "Open in browser" and every site URL drop the port. The links are wrong whenever HTTP/HTTPS is not 80/443, which includes the 8080/8443 fallback.
8. **P1-6**: A fresh load with the daemon down: Overview shows "Loading…" forever, Runtimes says "No versions match this filter", Doctor shows an empty state, and Settings sections silently vanish.
9. **P1-8**: The Doctor/Wizard "Trust certificate" and "Configure DNS" jobs report **done / "Local CA trusted"**, yet `/tls` and `/dns` still say false. The UI never verifies the result.
10. **P1-9**: "Share publicly" puts a local site on the internet in **one click, with no confirmation**. Share state is also lost on reload, because `GET /share/{id}` is never used.

---

## P0

### P0-1 · Clearing a port in Settings saves `0`, and the daemon accepts it
- **Repro:** Settings → Ports → clear **HTTPS** → Save. The toast says "Settings saved". `GET /config` now returns `"https_port":0`, and Overview shows `HTTP :18081 · HTTPS :0`.
- **Evidence:** `work/ui-audit/flow-misc.json` (`emptyPortValue: "0"`, `cfgApi2`), `docs/audit/ui/flow-settings-pairing.png` (the HTTPS field shows `0`). Also `PATCH /config {"api_port":0}` → **200** (curl).
- **Cause:**
  - `packages/ui/src/screens/Settings.tsx:87`: `Number(e.target.value)` turns `""` into `0`. There is no client-side validation, even though the input has `min={1}`.
  - `crates/bz-core/src/config.rs:88-105` (`validate`) checks only the TLD, never ports.
- **Fix:**
  - UI: keep the draft as a string. Disable Save, with an inline error, unless `1 ≤ port ≤ 65535`, ports are distinct, and none is the API port.
  - Daemon: reject port `0` and duplicate ports in `Config::validate`.

### P0-2 · Editing the API port strands the UI (and a paired phone)
- **Repro:** Settings → API = 17999 → Save. The daemon persists it. On its next start it listens on 17999, but the desktop UI keeps calling `VITE_BOZKURT_API` / `DEFAULT_BASE_URL` (`:7878`) and shows "Daemon not reachable" for good. The same form is fully editable from a **paired phone** (`docs/audit/ui/mobile-remote-settings.png`).
- **Cause:**
  - `Settings.tsx:137` exposes `api_port`, with no warning or confirm.
  - `api/client.ts:27` and `api/index.ts:51` hard-code the base URL.
  - `remote.port` is not editable, but `api_port` is.
- **Fix:**
  - Hide `api_port` (CLI/config only), or make it read-only with an explanation.
  - If it stays editable, confirm with "the app will reconnect on :N", and have the desktop shell read the port from `config.toml` the way the CLI does (`bz-cli/src/daemon.rs:24`).
  - Never allow port edits from remote mode.

## P1

### P1-1 · Service state goes stale: action/PATCH responses are ignored, and DNS emits no events
- **Repro A:** Services → DNS → **Start**. `POST /services/dns/start` → 200 `{"status":"running"}`, but the card stays **Stopped**. It also stays Stopped after "Start all".
- **Repro B:** Services → Caddy → toggle "Start at login". `GET /services/caddy` → `"autostart":true`, but the switch stays **off**.
- **Evidence:** `flow-services.json` (`dnsAfterStart`, `afterStartAll`), `flow-sites.json` (`autostartChecked:false` vs `autostartApi`). The WebSocket log has `service.status` frames for caddy only.
- **Cause:**
  - `Services.tsx:75,77,79,83`, `Overview.tsx:115`, `Overview.tsx:37`, `Services.tsx:99` and `Overlays.tsx:53-55,79` call `run(() => api.serviceAction(...))` and throw the returned `Service`/`Service[]` away. They rely only on `service.status` events.
  - The daemon emits no event for the built-in DNS service (`bz-core/src/services.rs:88`, which is not supervised), and none for `patch_service` (`bz-core/src/engine.rs:800`).
- **Fix:**
  - UI: in a store helper `applyService(s)`, upsert the returned `Service` (and each item of `Service[]`), then use it everywhere.
  - Daemon: emit `service.status` for DNS start/stop and after `patch_service`.

### P1-2 · The "Live tail" in the service logs drawer never shows new lines
- **Repro:** Services → Caddy → Logs, then restart Caddy. The drawer still shows the same 9 lines, while the global Logs page receives the new ones.
- **Evidence:** `flow-sites.json` `liveTail: {before: 9, after: 9}`; `docs/audit/ui/issue-service-live-tail-empty.png`. The WS frames carry `"source":"service:caddy"`.
- **Cause:**
  - `packages/ui/src/screens/Services.tsx:20` filters `l.source === name`.
  - The daemon rewrites log sources to `service:<name>` (`crates/bz-daemon/src/api.rs:760-779`), as documented in `docs/API.md` (v1 additions).
- **Fix:** filter on `l.source === \`service:${name}\``. Also rename the "Live tail" switch to "Auto-scroll", since it only toggles `follow`.

### P1-3 · Dev/StrictMode: the event stream is closed permanently
- **Repro:** `npm run dev` in `apps/desktop` (StrictMode), then open `:1420`. You get a permanent **"Daemon not reachable"** banner, even though every REST call succeeds. No job progress or live logs arrive, and the console shows `WebSocket is closed before the connection is established`. The production build is fine.
- **Evidence:** `docs/audit/ui/issue-dev-strictmode-offline-banner.png`; `walk-desktop-en.json` (the WS opens once, then closes and never reconnects).
- **Cause:**
  - `App.tsx:230` creates the API in `useMemo`.
  - The StrictMode effect cleanup runs `api.close()` (`store.ts:120-124`), which sets `EventStream.closed = true` (`client.ts:131-136`).
  - The re-run effect re-attaches the **same** instance, and `subscribe()` refuses to connect while `closed` (`client.ts:119-122`).
- **Fix:** have each attach create the API inside the effect. Alternatively, have `subscribe()` reset `closed` and reconnect, or keep `close()` out of the effect cleanup. Also review `App.tsx:229-237`.

### P1-4 · Site URLs ignore non-default HTTP/HTTPS ports
- **Repro:** With `http_port=18080` and `https_port=18443` (or the daemon's documented fallback to 8080/8443), use "Open in browser", the toast "Open", the palette "Open site", or the sites row link. Each one opens `https://static.test` with no port and fails (`chrome-error://chromewebdata/`). The site actually answers on `:18080` (`curl -H 'Host: static.test' :18080` → 200).
- **Cause:** `packages/ui/src/screens/Sites.tsx:13`: `siteUrl` never uses `overview.http_port` / `https_port`.
- **Fix:** add `siteUrl(site, overview)` that appends `:${port}` when the port is not 80/443, and use it in Sites, Overlays and the toasts.

### P1-5 · A PHP site can be created with no PHP runtime, and Doctor says everything is fine
- **Repro:** With no PHP installed, Sites → Add site → PHP folder. The PHP select says "Global default" and Create works with no warning. `https://phpsite.test` → **502 `PHP latest is not installed - run: bz runtime install php latest`**. Doctor still reports `runtimes ok (node 22.23.3)` and `sites ok`.
- **Evidence:** `flow-site-create-php.png`, plus the curl output quoted above.
- **Cause:**
  - `Sites.tsx:98-105` offers "Global default" even when `opts.php` is empty.
  - The daemon's doctor `sites`/`runtimes` checks do not cross-check the runtime each site needs.
- **Fix:**
  - When `kind==="php"` and no PHP is installed, show an inline warning with an "Install PHP" CTA, which deep-links to `/runtimes?kind=php` or calls `installRuntime`.
  - Do the same for `app` with Node.
  - Daemon: add a doctor check, "site X needs PHP, none installed", with a machine fix.

### P1-6 · Offline and error states are missing or misleading
- **Repro:** Stop the daemon, then load the app fresh.
  - **Overview:** "Loading…" forever.
  - **Runtimes:** "No versions match this filter."
  - **Doctor:** the empty state repeats the page title ("Doctor / Diagnoses ports…").
  - **Settings:** the General and Remote sections silently disappear, and About shows "—".
  - Mid-session, the banner appears (good), but every list still shows stale data with no visual stale marker.
- **Evidence:** `issue-offline-fresh-overview.png`, `issue-offline-fresh-runtimes.png`, `issue-offline-fresh-doctor.png`, `issue-offline-fresh-settings.png`, `issue-offline-sites-stale.png`.
- **Cause:**
  - `Overview.tsx:30` (`if (!overview) return Loading`).
  - `Runtimes.tsx:34`, `Doctor.tsx:68`, `Overview.tsx:27` and `QuickSetup.tsx:29` map errors to `[]`.
  - `Settings.tsx:67` swallows errors.
  - `store.ts:136-142` sets `loaded:true` but records no error.
- **Fix:**
  - Add a `loadError` to the store.
  - Render an `ErrorState` (message + Retry) on each screen when a fetch fails or `conn !== "open"` and there is no data.
  - Distinguish "failed to load" from "empty".

### P1-7 · Doctor "Fix" copies CLI text instead of fixing
- **Repro:** Doctor → "Web server (Caddy): Caddy is not installed" → **Fix** only copies `bz service install caddy` to the clipboard. The same happens for "Runtimes: no runtime installed" (`bz runtime install php 8.4  (or node lts)`, which is not even a runnable command) and for PATH. The primary blue "Fix" button suggests an action happens.
- **Evidence:** `issue-doctor-fix-copies-text.png`, `flow-misc.json` `fixPathToast`.
- **Cause:**
  - `Doctor.tsx:24-34` handles only `dns.configure`, `tls.trust` and `services.<n>.<a>`.
  - The daemon's `machine_fix` (`bz-daemon/src/api.rs`, tests around line 826) maps neither `service install` nor `runtime install`.
  - The UI never calls `POST /doctor/fix/{id}`, which is in the contract.
- **Fix:**
  - Daemon: add machine ids such as `services.caddy.install` and `runtimes.php.install`.
  - UI: call `POST /doctor/fix/{id}` generically.
  - For text-only fixes, label the button "Copy command" (secondary variant) instead of "Fix".

### P1-8 · Trust/DNS jobs report success while the checks still fail
- **Repro:** Doctor → Local CA → Fix, and Local DNS → Fix (the same applies in the Quick setup wizard).
  - The jobs finish **done** with `"Local CA trusted"` and `".test now resolves locally"`.
  - Re-running Doctor still shows `tls warn … is not trusted` and `dns warn … not routed`. `GET /tls` → `ca_trusted:false`, `GET /dns` → `configured:false`, and Overview badges stay "Not trusted / Not configured".
  - The user gets a success toast and is left confused.
- **Evidence:** `flow-misc.json` (`jobsApi`, `doctor`), `flow-doctor-fix-tls.png`, `flow-overview-after.png`.
- **Caveat:** in this sandbox there is no root and no system trust store, so the actual trust is expected to fail. **The success report is the bug.**
- **Fix:**
  - Daemon: re-check after trust/configure, and fail the job with a useful message when the result is not effective (`bz-daemon/src/api.rs` `trust_job` ~L458-480, and the DNS configure job).
  - UI: after the job, refetch `/tls` and `/dns`. Show "Still not trusted — see details" when they disagree.

### P1-9 · "Share publicly" has no confirmation, and its state is lost on reload
- **Repro:** Site drawer → "Share publicly". A Cloudflare quick tunnel starts immediately and exposes the local site on the internet (`https://scripts-saints-car-quite.trycloudflare.com`). Reload the app and the site shows as **not shared**; pressing Share again starts a second tunnel.
- **Evidence:** `flow-sites.json` `httpsToast` (share URL), jobs list "Share static.test".
- **Cause:**
  - `Sites.tsx:252-265` has no `confirmAction`.
  - `store.ts` `shares` is populated only from `share.url` events.
  - `GET /share/{site_id}` (`bz-daemon/src/api.rs` router) is never called; it is not even in `BozkurtApi`.
- **Fix:**
  - Confirm: "This makes https://… reachable by anyone on the internet until you stop it."
  - On drawer open (and on attach), load the share state via `GET /share/{id}`, and add `shareStatus()` to the API.

### P1-10 · Mailpit "Open" points at the SMTP port
- **Repro (code-verified; Mailpit was not installed during the run):** the Services card for Mailpit has `port: 1025` (`GET /services`), so "Open" goes to `http://127.0.0.1:1025`, which is SMTP. The web UI is on 8025 (`bz-core/src/services.rs:45` `MAILPIT_UI_PORT`).
- **Cause:** `packages/ui/src/lib/labels.ts:28-29` uses `port ?? 8025`.
- **Fix:** always use 8025 (or a new `ui_port` field on Service) for Mailpit's web UI.

### P1-11 · The pairing QR uses `addresses[0]`, whose order is unstable and can be a VPN or bridge IP
- **Repro:** Enable remote. `GET /remote` returned `["172.16.2.225"]` at first, and later `["10.77.1.1","172.16.2.225"]`. The QR (`Settings.tsx:24`) encodes the first entry, so a phone scanning it may try an unreachable interface.
- **Evidence:** `flow-misc.json` `remoteApi` vs `flow-pair.json` `devices`.
- **Fix:**
  - Daemon: sort addresses by preference (the default-route interface first; drop docker/VPN/link-local).
  - UI: let the user pick the address used in the QR when there are several, and/or encode all addresses (`host=a,b`) and have the phone try each.

### P1-12 · Quick setup declares "You're all set" regardless of what was skipped or failed
- **Repro:** Quick setup → skip PHP, Node, HTTPS and DNS → "Start services" → **"You're all set"**, even though CA is untrusted, DNS is not configured and PHP is missing. The Services step lists only installed services (here Caddy and DNS), so a user without Caddy gets an empty list.
- **Evidence:** `flow-misc.json` `qs1..qs6`, `flow-quicksetup-services.png`.
- **Cause:** `QuickSetup.tsx:85` (`startable` excludes not_installed) and `QuickSetup.tsx:171-184` (the done step is unconditional).
- **Fix:**
  - Make the done step summarise what is still pending (with links).
  - In the Services step, offer "Install & start Caddy" when it is missing.

### P1-13 · The packages/ui playground cannot talk to a real daemon (CORS)
- **Repro:** `npm run dev` in `packages/ui` serves `:5173`. `OPTIONS` with `Origin: http://localhost:5173` returns no `Access-Control-Allow-Origin`. `http://127.0.0.1:1420` is also rejected, while `http://localhost:1420` is allowed.
- **Evidence:** the curl preflight output in this run.
- **Cause:** `crates/bz-daemon/src/api.rs:29-35` `CORS_ORIGINS`; `packages/ui/vite.config.ts` has no fixed port.
- **Fix:** pin the playground to `:1420`/`:1430` (or add a vite proxy to `/api/v1`), or add `localhost:5173` and the `127.0.0.1:*` variants in debug builds. Document it in `packages/ui/README.md`.

## P2

1. **Contrast (axe `color-contrast`, serious)**
   - **Evidence:** 3–28 nodes per screen in light mode and 14 in dark mode.
   - **Main culprits:** `--text-faint` (#8a93a3 on white is about 3.1:1) used for `.faint`, `small`, the sidebar footer text and `.kbd`; and `.btn.primary` text on `--accent`.
   - **File:** `packages/ui/src/styles/index.css:26,56,96,113,136`.
   - **Fix:** darken `--text-faint` to at least #6b7280 (light) and lighten it in dark mode; check the primary button foreground.
2. **Accessible names don't match visible text (axe `label-content-name-mismatch`)**
   - The topbar search has `aria-label="Command palette"` but visible text "Type a command or search…" (`App.tsx:178`).
   - Runtime cards have `aria-label="PHP Not installed"`, which hides the "1 installed" text (`Overview.tsx:73`).
   - **Fix:** drop the aria-label, or start it with the visible text.
3. **Sidebar footer truncation**
   - At 1280 px the sidebar footer reads "Command p…" (`App.tsx:168-172`, see `flow-settings-pairing.png`).
   - **Fix:** shorten the label to "Search" or let it wrap.
4. **HTTPS row toggle is confusing**
   - The icon shows the *action* (open lock when HTTPS is on), and its label is just "HTTPS" (`Sites.tsx:414-415`, `mobile-en-sites.png`).
   - **Fix:** use the label "Disable HTTPS for X" / "Enable HTTPS for X" and a state-reflecting icon or switch.
5. **"Document root" placeholder**
   - The placeholder `public` (`Sites.tsx:95`) suggests `public` is used when empty, but empty means the project root.
   - **Fix:** set the placeholder to "(project folder)" and add a hint.
6. **"Environment" is a switch that only reveals the editor** (`Sites.tsx:205`).
   - It reads like "enable environment".
   - **Fix:** replace it with a disclosure button such as "Environment variables (0) ▸".
7. **Detect errors appear only as a raw toast**
   - Example: `not found: directory /nonexistent/dir does not exist`. Nothing is shown inline in the dialog, and Create stays disabled with no explanation (`Sites.tsx:148-157`, `flow-site-create-invalid.png`).
   - **Fix:** show the error inline under the Folder field, and strip the `not found:` prefix.
8. **Static detection is shown twice**
   - The badge reads "Detected: static", and the table shows Framework "static" for static sites.
   - **Fix:** the daemon should not set `framework:"static"`, or the UI should hide a framework that equals the kind.
9. **Uptime never ticks**
   - "Up 0s" / "· 0s" stays frozen until the next event (`Services.tsx:56`, `Overview.tsx:103`).
   - **Fix:** compute it from a `started_at` timestamp, or add a 1 s local tick.
10. **Install progress stalls**
    - It sits at "Extracting · 87%" for about 60 s with no movement (`flow-runtime.json` samples).
    - **Fix:** the daemon should report extraction progress, or the UI should show an indeterminate bar for that phase.
11. **The Overview "Quick setup" banner shows whenever PHP is not active**, even for Node-only users who finished setup (`Overview.tsx:32`).
    - **Fix:** base it on CA, DNS and "any runtime active", and let the user dismiss it.
12. **The palette finds nothing for "node" or "php"**
    - "Use runtime" commands exist only for installed, non-active versions, and there is no "Install Node…" entry. Searching by keyword (for example "node" → Runtimes page) does not match (`Overlays.tsx` ~L60-83).
    - **Fix:** add keyword aliases and an "Install <runtime>" action.
13. **TR: daemon-originated text is English**
    - This covers doctor titles and details, job titles ("Install node 22.23.3 tamamlandı"), and errors ("invalid input: `.dev` is a real HSTS-preloaded TLD…") (`issue-tr-doctor-english-checks.png`).
    - **Fix:** have the daemon return stable `id`/`code`, with the UI translating known ids; alternatively, send `Accept-Language` and localise in the daemon.
14. **TR grammar and wording** (`packages/ui/src/i18n/tr.ts`)

    | Key | Current | Suggested |
    |---|---|---|
    | `palette.openSite` | "{domain} aç" | "{domain} adresini aç" |
    | `wizard.installAndUse` | "{v} kur ve kullan" | "{v} sürümünü kur ve kullan" |
    | `wizard.useInstalled` | "{v} kullan" | "{v} sürümünü kullan" |
    | `sites.kind.proxy` | "Ters vekil" | "Ters proxy" |
    | `sites.framework` | "Çatı" | "Framework" |
    | `conn.offline` | "Servis sürecine ulaşılamıyor" | "bozkurtd'ye ulaşılamıyor" |

    `common.remove` and `common.uninstall` are both "Kaldır", which is ambiguous in the runtime row; use "Sil" or "Kaldır (sürüm)".

    Otherwise the TR dictionary is complete: all 276 keys are present and every placeholder matches EN.
15. **"Start at login" / "Oturum açılınca başlat" is wrong**
    - The flag is `config.autostart`, which means "start when bozkurtd starts" (`bz-core/src/config.rs:41`). It has nothing to do with OS login.
    - **Fix:** rename it to "Start with BOZKURT" / "BOZKURT ile başlat".
16. **Invalid TLD is only checked server-side**
    - Typing `dev` saves, then shows a raw error toast (`issue-settings-tld-dev.png`).
    - **Fix:** validate `dev`/`app` and empty values inline before Save (`Settings.tsx:129-131`).
17. **types.ts drift (minor)**
    - `PortOwner.in_use` (contract v1 additions) and `RemoteInfo.running` (daemon `api.rs` `remote_info`) are missing from `types.ts`.
    - `pair()` ignores `device_id`.
    - Contract endpoints not wrapped in `BozkurtApi`: `POST /doctor/fix/{id}`, `GET/POST/DELETE /services/{engine}/databases`, `POST /dns/unconfigure`, `POST /tls/untrust`, `POST /tls/trust {system}`, `GET /share/{id}`.
    - So no UI exists for databases, undoing DNS/TLS, or system-wide trust.
18. **The mobile Sites list shows only the domain**
    - Kind, version and path are hidden by `hide-sm`, and the two unlabeled icon buttons remain (`mobile-en-sites.png`).
    - **Fix:** add a secondary line with kind and version.
19. **The remote token is in the WebSocket URL query (`?token=`) and in localStorage in plain text**
    - This is by design (contract), but proxies and logs may capture it.
    - **Fix:** consider a short-lived WS ticket (`POST /events/ticket`), and store the token in the Tauri secure store on mobile.
20. **React Router v7 future-flag warnings on every load** (dev).
    - **Fix:** set `future={{ v7_startTransition: true, v7_relativeSplatPath: true }}` on `HashRouter` (`App.tsx:250`).
21. **The jobs drawer and toasts use daemon job titles verbatim** ("Install node 22.23.3", lowercase runtime id).
    - **Fix:** map runtime and service ids to labels (`RUNTIME_LABEL` / `SERVICE_LABEL`) when rendering.
22. **Overview port badge shows `HTTPS :0`** when the config is invalid (a consequence of P0-1).
    - **Fix:** after the P0-1 fix, show a warning badge when a port is 0 or out of range.

## 4. Field-name diff (types.ts vs real JSON)

Method: the JSON from `/overview`, `/runtimes`, `/runtimes/{node,php}/versions`, `/services`, `/services/caddy`, `/sites`, `/dns`, `/tls`, `/ports/17878`, `/doctor`, `/jobs`, `/config`, `/remote` (with a paired device) and `/import/scan` was compared key-by-key with the interfaces in `types.ts`. The script output is in `work/ui-audit/keydiff.txt`.

| Interface | Extra in JSON | Required in TS but missing | Notes |
|---|---|---|---|
| Overview, Runtime, Version, Service, DoctorCheck, DnsInfo, TlsInfo, Config, Job, PairedDevice, ImportScan | — | — | match |
| Site | — | — | optional `root/php/node/command/proxy_port/app_port/app_status` were not observed (no app/proxy site created) |
| PortOwner | `in_use` | — | add `in_use: boolean` |
| RemoteInfo | `running` | — | add `running: boolean`; show "enabled but not listening" when `enabled && !running` |
| pair response | `device_id` | — | optional |
| Events | `{"type":"connected"}` is not sent by the daemon (it came from the vite HMR socket) | — | all daemon event shapes matched `BzEvent` |

There is **no** field-name mismatch on the existing interfaces. The real contract bugs are behavioural, not shape: the log `source` prefix (P1-2), the unused endpoints (P2-17), and responses that are ignored (P1-1).

## 5. Things that worked well
- Runtime install with live progress, auto-activation, success toast, and the jobs drawer showing running and done states.
- Caddy install, start, stop and restart; the global Logs page (live stream, source filter, search, pause).
- Site creation with detection; alias and env edits saved (verified via API); delete with a proper confirm dialog.
- Remote enable; a pairing code with a 5-minute countdown and a valid `bozkurt://pair` QR. Wrong-code errors are clear ("Pairing code is invalid or expired"). Real pairing from the mobile web build worked, and that build then operated fully over the LAN listener with `?token=` on the WS.
- No horizontal overflow at 390 px on any screen. The tab bar and More screen work, and dark theme renders correctly.
- No uncaught JS exceptions (`pageerror`) anywhere in the production build.

## Appendix: reproduction harness (not committed)
`work/ui-audit/`:
- `lib.js` (playwright helpers plus axe injection)
- `walk.js` (screen walk per viewport and language)
- `flow-runtime.js`, `flow-services.js`, `flow-sites.js`, `flow-misc.js`, `flow-pair.js`, `flow-final.js`
- `serve.sh` (serves a production build on `:1430`)
- Raw results: `*.json`, `keydiff.txt`, `daemon.log`
