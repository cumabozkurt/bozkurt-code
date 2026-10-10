# bozkurtd HTTP API v1 reference

This is the HTTP API of `bozkurtd` 1.0.0, the BOZKURT CODE daemon. The desktop app, the mobile companion, `bz` and
`bozkurt-mcp` all talk to it. The source of truth is `crates/bz-daemon/src/api.rs` (router, guard, handlers) and
`crates/bz-core/src/models.rs` (wire types).

**Stability.** Within 1.x the API only changes in backwards-compatible ways: new routes, new optional request
fields, new response fields. Clients must ignore response fields they do not know. Breaking changes wait for
`/api/v2` and are announced in the CHANGELOG first.

## Listeners

| Listener | Address | Enabled | Authentication |
|---|---|---|---|
| Local | `http://127.0.0.1:<api_port>/api/v1` (default port `7878`; `bozkurtd --port N` overrides `api_port`) | always; binds IPv4 loopback only | [local API token](#local-api-protection) or an allow-listed browser `Origin` |
| Remote (LAN) | `http://<lan-ip>:<remote.port>/api/v1` (default `7879`), bound on `0.0.0.0` | only while `remote.enabled` is true (default false) | [device token](#remote-listener) from pairing |

Both listeners serve the same router. The differences are listed under [Remote listener](#remote-listener).
The remote listener is advertised over mDNS as `_bozkurt._tcp.local.` (best effort).

## Conventions

- Request and response bodies are JSON. A request with a body must send `Content-Type: application/json` (or
  `application/<x>+json`, parameters allowed). An empty or whitespace-only body is read as `{}`.
- Field names are snake_case. Optional fields are **omitted** when absent (never `null`), except where a type
  below says `| null`.
- Timestamps (`created_at`, `started_at`, `finished_at`, `expires_at`, `last_seen`) are RFC 3339 UTC with second
  precision, e.g. `2026-10-09T12:00:00Z`.
- Long operations return **`202 {"job_id": "<id>"}`**. Follow them with `job.*` events or `GET /jobs/{id}`; a
  job that ends in `failed` carries the error in `message`.
- Errors use one envelope on every route:

  ```json
  {"error": {"code": "not_found", "message": "not found: site `shop`"}}
  ```

  | Status | `code` | When |
  |---|---|---|
  | 400 | `invalid_input` | bad body, bad path parameter (including unknown service/site actions and runtime kinds), validation failure (`message` often starts with `invalid input: `) |
  | 401 | `unauthorized` | missing or wrong token ([local](#local-api-protection), [remote](#remote-listener)) |
  | 401 | `bad_code` | wrong or expired pairing code (`POST /remote/pair`) |
  | 403 | `forbidden` | `Origin` not allowed, a route not available on this listener, or a config key that can only be changed locally |
  | 404 | `not_found` | unknown route, site, service, job or device; unknown runtime/DNS/TLS/remote action or sub-resource |
  | 405 | `method_not_allowed` | route exists, method does not |
  | 409 | `conflict` | domain already in use, service not running, remote port cannot be bound (`cannot listen on 0.0.0.0:<port>: …`), runtime version being installed, ... |
  | 415 | `unsupported_media_type` | request body or `Content-Type` is not JSON |
  | 421 | `forbidden` | `Host` header does not name the local listener |
  | 422 | `unsupported` | operation not supported on this platform |
  | 422 | `unverified_download` | a download has no published checksum and no pinned sha256, and was not allowed ([Unverified downloads](#unverified-downloads)); usually seen as a failed job's `message` |
  | 429 | `rate_limited` | too many pairing attempts (per address or overall) |
  | 429 | `pairing_locked` | the pairing code was revoked after too many wrong guesses |
  | 500 | `io`, `internal` | unexpected failure |
  | 502 | `upstream`, `checksum_mismatch` | download or upstream index failed; `checksum_mismatch` also when a file differs from its [pinned sha256](#unverified-downloads) |

## Authentication

### Local API protection

Every request to the local listener passes these checks, in this order (`guard` in `api.rs`). Requests that
fail get the envelope shown.

1. **Origin.** If an `Origin` header is present (any method, including the `/events` WebSocket upgrade), it must be
   one of the allowed origins (compared case-insensitively):
   - always: `tauri://localhost`, `http://tauri.localhost`, `https://tauri.localhost` (the packaged Tauri webviews);
   - dev origins, **only** in debug builds or when the daemon runs with `BOZKURT_DEV=1` (or `true`):
     `http://localhost:1420`, `http://127.0.0.1:1420`, `http://localhost:1430`, `http://127.0.0.1:1430`,
     `http://localhost:5173`, `http://127.0.0.1:5173`.

   ```json
   403 {"error": {"code": "forbidden", "message": "origin https://evil.example is not allowed"}}
   ```
2. **Content type.** A `POST`, `PUT`, `PATCH` or `DELETE` that carries a `Content-Type` other than JSON is refused,
   and on routes that read a body, so is a non-empty body without any `Content-Type`. This stops preflight-free "simple" cross-site requests
   (`text/plain`, forms).

   ```json
   415 {"error": {"code": "unsupported_media_type", "message": "request bodies must be sent as Content-Type: application/json"}}
   ```
3. **Host** (local listener only, DNS-rebinding defence). The `Host` header must be `127.0.0.1:<port>`,
   `localhost:<port>` or `[::1]:<port>`, where `<port>` is the port the listener is bound to (a `Host` without a
   port counts as `:80`).

   ```json
   421 {"error": {"code": "forbidden", "message": "host \"shop.test:7878\" is not this daemon (use 127.0.0.1:7878)"}}
   ```
4. **Token** (local listener only). A request **without** an `Origin` header (CLI, MCP, scripts, other local users)
   must carry the per-install token, as `Authorization: Bearer <token>` (`Bearer`/`bearer`) or
   `X-Bozkurt-Token: <token>`. On `GET /api/v1/events` the token may also be passed as `?token=<token>`.
   Exempt: `GET /api/v1/health`, CORS preflights (`OPTIONS`), and requests with an allow-listed `Origin` (step 1
   already vouched for them).

   ```json
   401 {"error": {"code": "unauthorized", "message": "missing local API token (send Authorization: Bearer <token> from $BOZKURT_HOME/api-token)"}}
   401 {"error": {"code": "unauthorized", "message": "invalid local API token"}}
   ```

The token lives in **`$BOZKURT_HOME/api-token`**: 64 hex characters (256 bits), created by `bozkurtd` at startup
with file mode `0600`, and reused across restarts (the mode is re-applied on every start). It is compared in
constant time. `bz`, `bozkurt-mcp` and the desktop shell read it from the file; a script can do the same:

```sh
curl -H "Authorization: Bearer $(cat "${BOZKURT_HOME:-$HOME/.bozkurt}/api-token")" http://127.0.0.1:7878/api/v1/overview
```

**CORS.** The allowed origins above get CORS headers for `GET`, `POST`, `PATCH`, `PUT`, `DELETE`, `OPTIONS` and the
request headers `Authorization`, `Content-Type`, `Accept`, `X-Bozkurt-Token` (preflight cached 600 s). The same
list applies on the remote listener.

### Remote listener

The remote listener runs the same router with these differences:

- **Device token instead of the local token.** Every request needs `Authorization: Bearer <device token>` from
  `POST /remote/pair`. `X-Bozkurt-Token` and the local `api-token` are **not** accepted here. `/events` also
  accepts `?token=<device token>`. Only `POST /api/v1/remote/pair` and `OPTIONS` are open; `GET /health` needs the
  token too.

  ```json
  401 {"error": {"code": "unauthorized", "message": "missing bearer token (pair this device first)"}}
  401 {"error": {"code": "unauthorized", "message": "invalid or revoked token"}}
  ```
- **No Host check.** The Origin check (403) and the Content-Type check (415) apply as on the local listener and run
  before the token check.
- **Routes limited by listener:**

  | Route | Local | Remote |
  |---|---|---|
  | `POST /remote/pairing-code` | yes | 403 `pairing codes can only be generated on this computer` |
  | `POST /remote/pair` | 403 `pairing happens on the remote listener (enable remote access first)` | yes, no token |
  | `POST /daemon/shutdown` | yes | 403 `the daemon can only be stopped from this computer` |

  Everything else, including `PATCH /config`, `POST /remote/disable` and `DELETE /remote/devices/{id}`, works from a
  paired device, with one limit: a `PATCH /config` from the remote listener that would change `api_port` or
  `remote.port` → `403 forbidden` (`api_port can only be changed locally` / `remote.port can only be changed
  locally`), so a phone cannot move the API or its own listener and strand the desktop.
- Device tokens are 64 hex characters; only their SHA-256 is stored, in `$BOZKURT_HOME/remote.json`.

## Types

TypeScript notation; the Rust structs in `crates/bz-core/src/models.rs` serialize to exactly these shapes.

```ts
type RuntimeKind = "php" | "node" | "python" | "bun" | "go";
// path segments also accept "nodejs", "py", "golang" (case-insensitive)
type ServiceName = "caddy" | "dns" | "mariadb" | "postgres" | "redis" | "mailpit" | `php-fpm-${string}`;
// path segments also accept "mysql" (= mariadb), "postgresql" (= postgres), "valkey" (= redis)
type ServiceStatus = "stopped" | "starting" | "running" | "stopping" | "crashed" | "external" | "not_installed";
type SiteKind = "php" | "proxy" | "static" | "app";

interface Version { version: string; channel: "lts" | "current" | "stable" | "eol"; installed: boolean;
                    active: boolean; released?: string; size_bytes?: number }
interface Runtime { kind: RuntimeKind; active?: string; installed: string[] }
interface Service { name: string; status: ServiceStatus; pid?: number; port?: number; version?: string;
                    autostart: boolean; uptime_secs?: number; message?: string }
interface Site { id: string; domain: string;              // "shop.test"
                 aliases: string[]; path: string;          // absolute project directory
                 kind: SiteKind;
                 root?: string; php?: string; node?: string; proxy_port?: number; command?: string;
                 app_port?: number; app_status?: ServiceStatus;   // app sites, while running
                 https: boolean; wildcard: boolean;
                 framework?: string;                       // detected: laravel, wordpress, nextjs, ...
                 env: Record<string, string>; created_at: string }
interface Job { id: string; title: string; status: "running" | "done" | "failed";
                progress: number;                          // 0..1
                message?: string; started_at: string; finished_at?: string }
interface JobAccepted { job_id: string }
interface Overview { version: string; os: string; arch: string; home: string; tld: string;
                     runtimes: Runtime[]; services: Service[]; sites_count: number;
                     ca_trusted: boolean; dns_configured: boolean; http_port: number; https_port: number }
interface DoctorCheck { id: string; title: string; status: "ok" | "warn" | "fail"; detail: string;
                        fix?: string }                     // machine fix id (see /doctor) or a CLI hint
interface PortOwner { port: number; pid?: number; process?: string; ours: boolean; in_use: boolean }
interface Detection { path: string; framework?: string; kind: SiteKind; root?: string; php?: string;
                      node?: string; command?: string; suggested_domain: string }
interface DnsStatus { tld: string; port: number; configured: boolean; running: boolean }
interface TlsStatus { ca_path: string; ca_trusted: boolean; ca_expires?: string }
interface Health { ok: boolean; version: string }
interface LogLines { lines: string[] }
interface CreateSiteRequest { path: string; domain?: string; kind?: SiteKind; root?: string; php?: string;
                              node?: string; proxy_port?: number; command?: string; https?: boolean;
                              wildcard?: boolean; aliases?: string[]; env?: Record<string, string> }
interface SitePatch { domain?: string; aliases?: string[]; path?: string; kind?: SiteKind; root?: string;
                      php?: string; node?: string; proxy_port?: number; command?: string; https?: boolean;
                      wildcard?: boolean; framework?: string; env?: Record<string, string> }
interface Config { tld: string; dns_port: number; http_port: number; https_port: number; api_port: number;
                   autostart: string[]; remote: { enabled: boolean; port: number }; lan_access: boolean;
                   allow_unverified: boolean;
                   [extra: string]: unknown }              // unknown keys are preserved
interface ImportVhost { source: string; domain: string; path: string; php?: string }
interface ImportDns { source: string; tld: string; file: string }
interface ImportScan { vhosts: ImportVhost[]; dns: ImportDns[] }
interface PairedDevice { id: string; name: string; last_seen: string }
interface RemoteInfo { enabled: boolean; running: boolean; port: number; addresses: string[];
                       paired_devices: PairedDevice[] }
interface PairingCode { code: string; expires_at: string }   // 8 digits, valid 5 minutes, single use
interface PairResponse { token: string; device_id: string }
interface ShareState { site_id: string; url: string | null }
interface ApiError { error: { code: string; message: string } }
```

## Endpoints

All paths are relative to `/api/v1`. "job" means `202 JobAccepted`.

### Health, overview, daemon

| Method | Path | Body / Query | Response |
|---|---|---|---|
| GET | `/health` | | `200 Health` (`{"ok": true, "version": "1.0.0"}`); no token on the local listener |
| GET | `/overview` | | `200 Overview` |
| POST | `/daemon/shutdown` | | `202 {"stopping": true}`; local listener only. The daemon then stops gracefully, the same path as SIGTERM: remote listener, API, supervised processes, DNS responder |

### Runtimes

| Method | Path | Body / Query | Response |
|---|---|---|---|
| GET | `/runtimes` | | `200 Runtime[]` |
| GET | `/runtimes/{kind}/versions` | `?refresh=true` (or `1`) bypasses the cache | `200 Version[]` (live upstream index, cached 6 h) |
| POST | `/runtimes/{kind}/install` | `{"version": string, "allow_unverified"?: bool}` (`version` required) | job |
| POST | `/runtimes/{kind}/use` | `{"version": string}` (required) | `200 Runtime` |
| DELETE | `/runtimes/{kind}/{version}` | | `204` |

A missing or blank `version` → 400 ``"`version` is required"``. Any other action → 404.

Installs are serialized per `(kind, version)`: a second install of the same version waits for the first (job message
`Waiting for another install of <kind> <version>`) and then reports it as already installed. `DELETE
/runtimes/{kind}/{version}` while that version is being installed → `409 conflict` (`<kind> <version> is being
installed`).

#### Unverified downloads

Every download is checked against the checksum its upstream publishes. When there is none (the static-php.dev PHP
builds used on macOS and Linux; Windows PHP, Go or cloudflared when the index drops its hash), bozkurtd **fails
closed**: the install job fails with code `unverified_download` before anything is fetched, e.g.

```text
unverified download: php-8.4.13-cli-linux-x86_64.tar.gz has no published checksum and no sha256 pinned from an
earlier install, so it cannot be verified. Retry with `allow_unverified` (…) to trust it once; its sha256 is then
pinned and later downloads must match
```

Such an asset is downloaded only when allowed, either for one install with `"allow_unverified": true` on
`POST /runtimes/{kind}/install` (`bz runtime install … --allow-unverified`), or for every download with the
`allow_unverified` config key (default `false`; it also covers services, php-fpm and cloudflared). The first allowed
download is **pinned** (trust on first use): its sha256 is stored in `state.json` under `download_pins` (URL → hex)
and the job message says `Unverified download: <file> has no published checksum; sha256 pinned for later installs`.
Later downloads of that URL must match the pin, even with the flag; a different file fails with
`checksum_mismatch` (502, `checksum mismatch for <file> (differs from the sha256 pinned on first download): …`). A
pinned URL needs no flag again. An upstream checksum, when there is one, always wins over a pin. The MCP server has
no bypass.

### Services and databases

| Method | Path | Body / Query | Response |
|---|---|---|---|
| GET | `/services` | | `200 Service[]` |
| GET | `/services/{name}` | | `200 Service` |
| PATCH | `/services/{name}` | `{"autostart"?: bool, "port"?: number}` | `200 Service` |
| POST | `/services/{name}/install` | `{"version"?: string}` | job |
| POST | `/services/{name}/{action}` | action ∈ `start`, `stop`, `restart` | `200 Service` |
| POST | `/services/all/{action}` | action ∈ `start`, `stop`, `restart` | `200 Service[]` |
| GET | `/services/{name}/logs` | `?lines=200` (1–10000) | `200 LogLines` |
| GET | `/services/{engine}/databases` | | `200 {"databases": string[]}` |
| POST | `/services/{engine}/databases` | `{"name": string}` | **`201 {"name": string}`** |
| DELETE | `/services/{engine}/databases/{name}` | | `204` |

- `{engine}` for the database routes is `mariadb` (alias `mysql`) or `postgres` (aliases `postgresql`, `pgsql`).
  The service must be `running` or `external` (404 when not installed, 409 when not running), and a
  `mysql`/`mariadb` or `psql` client must be found in the service's distribution or on `PATH`.
- Database names: `[A-Za-z0-9_-]{1,64}`. MariaDB system databases (`information_schema`, `performance_schema`,
  `mysql`, `sys`) cannot be dropped.
- An unknown service name → 404 ``unknown service `x` ``; an unknown action → 400.

### Sites

| Method | Path | Body / Query | Response |
|---|---|---|---|
| GET | `/sites` | | `200 Site[]` |
| POST | `/sites` | `CreateSiteRequest` | **`201 Site`** (missing fields filled from `bozkurt.toml`, then detection) |
| GET | `/sites/{id}` | | `200 Site` |
| PATCH | `/sites/{id}` | `SitePatch` | `200 Site` |
| DELETE | `/sites/{id}` | | `204` (also stops a running share) |
| POST | `/sites/{id}/{action}` | action ∈ `start`, `stop`, `restart` (only `kind = "app"`), `secure`, `unsecure` | `200 Site` |
| GET | `/sites/{id}/logs` | `?lines=200` (1–10000) | `200 LogLines` |
| POST | `/detect` | `{"path": string}` | `200 Detection` |

Create: field priority is request → `bozkurt.toml` → detection → defaults (`https = true`, `wildcard = false`).
`framework` is always taken from detection on create; it can be changed with `PATCH`. Patch: `env` replaces the
whole map; an empty string clears `root`, `php`, `node`, `command` or `framework`; `id`, `created_at`,
`app_port` and `app_status` cannot be patched. `secure`/`unsecure` set `https`. `start`/`stop`/`restart` on a
non-app site → 400.

**Site validation** (on create and on every patch; failures are 400 `invalid_input` unless noted):

- `path` must be absolute (``site path must be absolute: …``) and an existing directory (**404** `not_found`
  ``directory … does not exist``). `path`, `root`, `php` and `node` must not contain control characters.
- `root` (document root, relative to `path` or absolute) must not contain a `..` component (`/` or `\` separated).
- `domain` and every alias are normalised: lowercased; a leading `http://`/`https://`, a port, a path and a trailing
  dot are stripped; `.<tld>` is appended when missing (`shop` → `shop.test`). Each label must be 1–63 characters of
  `[A-Za-z0-9-]` without a leading or trailing `-`; the bare TLD is refused. Aliases equal to the domain and
  duplicates are dropped. A domain or alias already used by another site → **409** `conflict`.
- `id` is derived from the domain (`admin.shop.test` → `admin-shop`, then `-2`, `-3`, … when taken) and must match
  `[A-Za-z0-9_-]+`.
- `framework` must match `^[a-z0-9-]{1,32}$`.
- `env` keys must match `^[A-Za-z_][A-Za-z0-9_]*$`.
- `kind = "proxy"` needs `proxy_port`; `kind = "app"` needs a non-empty `command` (`$PORT`, `${PORT}` and `%PORT%`
  are replaced by the assigned port).

### DNS, TLS, ports

| Method | Path | Body / Query | Response |
|---|---|---|---|
| GET | `/dns` | | `200 DnsStatus` |
| POST | `/dns/configure` | `{"tld"?: string}` | job (may prompt for elevation) |
| POST | `/dns/unconfigure` | | job |
| GET | `/tls` | | `200 TlsStatus` |
| POST | `/tls/trust` | `{"system"?: bool}` (default false) | job |
| POST | `/tls/untrust` | `{"system"?: bool}` | job |
| GET | `/ports/{port}` | | `200 PortOwner` (`in_use` is true when anything, ours or not, listens on the port) |

**Local CA.** A CA created by 1.0.0 carries X.509 name constraints: it can only sign certificates for DNS names
under `test` and `localhost`. When a site needs a certificate for another TLD (after changing `tld`), the CA is
regenerated with that TLD added (same common name), the old site certificates in `certs/` are deleted and re-issued,
and the new CA must be trusted again (`POST /tls/trust`, `bz trust`): `GET /tls` then reports
`ca_trusted: false`. A CA created before 1.0.0 (without constraints) is kept as it is.

**Caddy admin API.** bozkurtd configures Caddy through its admin API, which is not on `127.0.0.1:2019`: on macOS and
Linux it is the unix socket `$BOZKURT_HOME/caddy/run/admin.sock` (directory mode 0700); on Windows (or when that
path is 100 bytes or longer) a free loopback port picked at each Caddy start. Port 2019 is not used, so a Caddy of
your own can keep it.

### Doctor

| Method | Path | Body / Query | Response |
|---|---|---|---|
| GET | `/doctor` | | `200 DoctorCheck[]` |
| POST | `/doctor/fix/{id}` | | job; unknown id → 404 |

Machine fix ids: `dns.configure`, `tls.trust`, `services.<name>.<start|stop|restart>`. `GET /doctor` rewrites
the CLI hints `bz dns configure`, `bz trust` and `bz service <start|stop|restart> <name>` to these ids; any
other `fix` is a free-text hint for a human and is not accepted by `/doctor/fix`.

### Jobs and config

| Method | Path | Body / Query | Response |
|---|---|---|---|
| GET | `/jobs` | | `200 Job[]` |
| GET | `/jobs/{id}` | | `200 Job` |
| GET | `/config` | | `200 Config` (`config.toml` as JSON) |
| PATCH | `/config` | JSON merge patch (RFC 7386; `null` removes a key, i.e. resets it to its default) | `200 Config`; 400 on validation failure (file unchanged), 403 for `api_port` / `remote.port` from the remote listener, 409 when the remote listener cannot bind |

Config keys and defaults: `tld` (`"test"`), `dns_port` (`5390`), `http_port` (`80`), `https_port` (`443`),
`api_port` (`7878`), `autostart` (`["caddy", "dns"]`), `remote.enabled` (`false`), `remote.port` (`7879`),
`lan_access` (`false`), `allow_unverified` (`false`, see [Unverified downloads](#unverified-downloads)).

- `tld` must be non-empty `[A-Za-z0-9-]` and not `dev` or `app` (HSTS-preloaded) → otherwise 400.
- **Ports.** `http_port`, `https_port`, `dns_port`, `api_port` and `remote.port` must each be in `1..=65535` and all
  different. Otherwise `400 invalid_input` and nothing is saved:

  ```json
  {"error": {"code": "invalid_input", "message": "invalid input: http_port must be between 1 and 65535"}}
  {"error": {"code": "invalid_input", "message": "invalid input: https_port (80) is already used by http_port"}}
  ```

  The second key named is the earlier one in the order `http_port`, `https_port`, `dns_port`, `api_port`,
  `remote.port`. The same check runs when the daemon loads `config.toml`: an invalid file stops `bozkurtd` from
  starting, with the same message.
- Changing `tld` to a name outside the local CA's permitted names (see [TLS](#dns-tls-ports)) makes the next
  certificate issue regenerate the CA; it must then be trusted again.
- Changing `tld` renames every site's domain and aliases onto the new TLD and restarts the DNS responder if it was
  running; changing `http_port`/`https_port` restarts Caddy; changing `remote` starts, stops or rebinds the remote
  listener. The listener is bound **before** the file is saved: when the port cannot be bound the answer is
  `409 conflict` and `remote` is left unchanged on disk (the old listener is restored if saving fails).
- **`lan_access`**: by default Caddy binds every site to loopback only (`default_bind 127.0.0.1 [::1]`), so `.test`
  sites are not reachable from the LAN. `lan_access: true` drops that bind and serves sites on every interface.
  Changing it reloads Caddy right away (the listeners are rebound).

### Import and share

| Method | Path | Body / Query | Response |
|---|---|---|---|
| GET | `/import/scan` | | `200 ImportScan` (vhosts and DNS settings found from other local stacks) |
| POST | `/import` | `ImportScan` (both lists optional) | job; creates one site per vhost. The job fails only when every vhost failed; otherwise its message lists the skipped ones. `dns` entries are never applied (BOZKURT does not change another tool's resolver): a body with `dns` entries and no `vhosts` → `400 invalid_input` (`importing DNS settings is not supported: …`); with vhosts, the job message ends with `; DNS not imported: …`, one note per entry, and an entry that also routes our TLD is flagged (`valet (/etc/resolver/test) also routes .test and may send it away from BOZKURT: remove /etc/resolver/test yourself`) |
| POST | `/share/{site_id}` | | job; on success the job message is the public URL and a `share.url` event is sent. Downloads `cloudflared` on first use; Caddy must be running |
| GET | `/share/{site_id}` | | `200 ShareState` (`url` is `null` when the site is not shared) |
| DELETE | `/share/{site_id}` | | `204` |

### Remote access

| Method | Path | Body / Query | Response |
|---|---|---|---|
| GET | `/remote` | | `200 RemoteInfo`. `running` is false when enabled but the bind failed; `addresses` is empty while disabled |
| POST | `/remote/enable` | | `200 RemoteInfo`: binds the listener first, then saves `remote.enabled = true`. `409 conflict` (`cannot listen on 0.0.0.0:<port>: …`) when the port cannot be bound; nothing is saved |
| POST | `/remote/disable` | | `200 RemoteInfo` (stops the listener, then saves `remote.enabled = false`) |
| POST | `/remote/pairing-code` | | `200 PairingCode`; local listener only. A new code replaces the previous one |
| POST | `/remote/pair` | `{"code": string, "device_name": string}` | `200 PairResponse`; remote listener only, no token |
| DELETE | `/remote/devices/{id}` | | `204`, or 404 when unknown |

Pairing codes are 8 random digits, valid 5 minutes and single use. Pairing errors:

| Status | `code` | `message` | When |
|---|---|---|---|
| 401 | `bad_code` | `Pairing code is invalid or expired` | wrong, used or expired code |
| 429 | `rate_limited` | `Too many pairing attempts; wait a minute` | more than 5 attempts per client address per minute, or more than 30 per minute from all addresses together |
| 429 | `pairing_locked` | `Too many wrong pairing codes; the code was revoked. Generate a new one on the computer` | the active code got 10 wrong guesses (from any address) and was revoked; every attempt fails this way, even with the right digits, until a new code is generated with `POST /remote/pairing-code` |
| 400 | `invalid_input` | | `device_name` not 1–100 characters |

The rate limits are checked before the code, so a limited attempt does not count as a wrong guess. The per-address
table forgets idle addresses and tracks at most 1024. Pairing QR text: `bozkurt://pair?host=<ip:port>&code=<8 digits>`.

### Events

| Method | Path | Body / Query | Response |
|---|---|---|---|
| GET | `/events` | WebSocket upgrade; `?token=` accepted (see [Authentication](#authentication)) | stream of `Event`, one JSON text message each |

```ts
type Event =
 | {type: "job.progress", job: Job} | {type: "job.done", job: Job} | {type: "job.failed", job: Job}
 | {type: "service.status", service: Service}
 | {type: "site.changed", site: Site} | {type: "site.removed", id: string}
 | {type: "runtime.changed", runtime: Runtime}
 | {type: "log", source: string, line: string}   // source: "site:<domain>", "service:<name>", "share:<site_id>"
 | {type: "share.url", site_id: string, url: string}
```

The server subscribes before completing the upgrade, answers pings, and ignores other client messages. A client
that falls behind by more than 1024 events silently misses the oldest ones; re-fetch state after a reconnect.
