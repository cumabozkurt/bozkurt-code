# Security Policy

BOZKURT CODE runs a local daemon, can listen on your LAN, generates and trusts a local certificate authority, and
changes OS DNS routing. We take reports about any of these seriously.

## Supported versions

| Version | Supported |
|---|---|
| `1.0.x` | ✅ |
| `< 1.0` (pre-release builds from `main`) | ❌ |

Security fixes go to the latest 1.x release. Update to it before reporting.

## Reporting a vulnerability

**Please do not open a public issue, discussion or PR for a vulnerability.**

- Preferred: GitHub **private vulnerability reporting**, at
  [github.com/cumabozkurt/bozkurt-code/security/advisories/new](https://github.com/cumabozkurt/bozkurt-code/security/advisories/new).
- Alternative: email **[info@cumabozkurt.tr](mailto:info@cumabozkurt.tr)** with `[security]` in the subject.

Include:

- the affected version or commit, OS and architecture;
- the component (`bozkurtd`, `bz`, `bozkurt-mcp`, desktop app, mobile app, install scripts, release workflow);
- steps to reproduce or a proof of concept, and the impact you see;
- whether the issue is already public anywhere.

What to expect:

- We acknowledge your report within **3 working days**.
- We send an initial assessment within **10 working days**.
- We coordinate disclosure with you. The default embargo is **90 days**, shorter if a fix ships sooner.
- We credit you in the advisory and CHANGELOG unless you ask us not to.

## Scope and threat model

In scope, among others:

- **Local API** (`127.0.0.1:7878`, protected as described in [Local API protection](#local-api-protection)). Problems here include:
  - DNS-rebinding attacks or cross-origin requests from a browser page that reach the API;
  - any request that is executed without a valid token or an allow-listed `Origin`;
  - leakage of `$BOZKURT_HOME/api-token` (it must stay mode 0600 and never be logged);
  - binding the API, or Caddy-served sites while `lan_access` is off, to a non-loopback address;
  - any way to trigger elevation or file writes outside `$BOZKURT_HOME` without the documented confirmation.
- **Remote listener** (`:7879`, **off by default**). Problems here include:
  - bypassing pairing or the bearer token;
  - getting around the [pairing limits](#remote-access-and-pairing) (8-digit code, 5-minute lifetime, rate limits,
    revocation after 10 wrong guesses);
  - token leakage, for example through the `?token=` WebSocket query parameter ending up in logs;
  - mDNS (`_bozkurt._tcp`) information disclosure beyond what is documented.
- **Local CA.** Problems here include:
  - exposure of `ca/rootCA-key.pem`, which must be mode 0600 and never leave `$BOZKURT_HOME`;
  - certificates issued for names outside the configured TLD, or a CA created by 1.0.0 that is missing its
    [name constraints](#local-ca);
  - trust being added to a store the user didn't confirm.
- **Elevation.** Problems here include:
  - a privileged step that does something other than what its confirmation text says;
  - command injection via domain names, TLDs, paths or site settings;
  - missing backups.
- **Downloads.** Problems here include:
  - checksum verification being skipped or bypassable, including installing an
    [unverified download](#unverified-downloads) without the user's explicit opt-in or ignoring a pinned sha256;
  - path traversal during archive extraction;
  - TLS downgrade.
- **MCP server.** Problems here include:
  - a destructive or system-level tool running without `confirm: true`;
  - a site command or environment set or run without the [MCP confirmation](#mcp-confirmation-for-shell-commands);
  - prompt-injection paths that let a tool act outside its documented scope.
- **Install scripts and release pipeline.** Problems here include:
  - SHA256SUMS bypass;
  - unsafe temp-file handling;
  - workflow injection;
  - secret exposure in CI logs.

Out of scope:

- Problems that need an attacker who already runs code as your user. That user owns `$BOZKURT_HOME` by design.
- Vulnerabilities in your own projects served by BOZKURT, or in upstream runtimes (PHP, Node.js, MariaDB, …). Report those upstream.
  If BOZKURT installs a known-vulnerable version *by default*, that is in scope.
- Missing hardening headers on local development sites.
- Exposure through `bz share`. Sharing makes a site public on purpose. Bugs in *how* the tunnel is set up are still in scope.

## Local API protection

`bozkurtd` serves its API on `127.0.0.1:<api_port>` (default `7878`). Since 1.0.0, every request goes through these checks:

| Check | Applies to | Rejected with |
|---|---|---|
| A present `Origin` header must be one of the packaged app's origins (`tauri://localhost`, `http://tauri.localhost`, `https://tauri.localhost`). The dev-server origins (`http://localhost:1420`, `:1430`, `:5173`) are accepted only when the daemon runs with `BOZKURT_DEV=1` or is a debug build. | every method, including the `/events` WebSocket upgrade | `403` |
| `POST`/`PUT`/`PATCH`/`DELETE` with a `Content-Type` other than `application/json` (stops preflight-free "simple" cross-site requests) | both listeners | `415` |
| The `Host` header must be `127.0.0.1`, `localhost` or `[::1]` with the API port (stops DNS rebinding) | local listener | `421` |
| A request without an allow-listed `Origin` (CLI, MCP, scripts, other local users) must carry the per-install token as `Authorization: Bearer <token>` or `X-Bozkurt-Token: <token>`; `/events` also accepts `?token=` | local listener; `GET /health` and CORS preflights are exempt | `401` |

The token is 64 hex characters, created by `bozkurtd` at startup in `$BOZKURT_HOME/api-token` with mode 0600, and
reused across restarts. `bz` and `bozkurt-mcp` read it automatically. A script can do the same:

```sh
curl -H "Authorization: Bearer $(cat ~/.bozkurt/api-token)" http://127.0.0.1:7878/api/v1/overview
```

To rotate it, stop the daemon (`bz daemon stop`), delete the file and start the daemon again. The LAN listener
(`:7879`, off by default) does not use this token; it uses per-device bearer tokens from pairing.

Caddy binds the sites it serves to `127.0.0.1` and `::1` only. Set `lan_access = true` in `config.toml` (or the
**Serve sites on the local network** switch in Settings → Network) to serve them on every interface, for example to
test from another device. Anyone on that network can then open your local sites.

### Caddy admin API

`bozkurtd` configures Caddy through Caddy's admin API, and since 1.0.0 that API is not on the well-known
`127.0.0.1:2019`, where any local process (or a second Caddy) could reach or clobber it. On macOS and Linux it listens
on the unix socket `$BOZKURT_HOME/caddy/run/admin.sock`; its directory is created with mode 0700 and a stale socket is
removed before Caddy starts. On Windows (and when the socket path would be 100 bytes or longer) it uses a free loopback
port chosen at each Caddy start. Known gap: on Windows the daemon does not yet check that the process on that port
is its own Caddy before sending it a config.

## Remote access and pairing

The remote (LAN) listener is off by default. When it is on:

- Pairing codes are **8 random digits**, valid **5 minutes** and **single use**; generating a new code replaces the old one.
- Each client address gets **5 attempts per minute** and all addresses together **30 per minute** (`429 rate_limited`).
- After **10 wrong guesses** in total, from any address, the code is **revoked** (`429 pairing_locked`); even the right
  code is refused until a new one is generated on the computer. An attacker with many addresses therefore gets at most
  10 guesses out of 10⁸ per code.
- Only a SHA-256 of each device token is stored (`$BOZKURT_HOME/remote.json`). A paired device cannot generate
  pairing codes, stop the daemon, or change `api_port` / `remote.port` (`403`).
- **Known limitation:** the remote listener speaks plain HTTP. The pairing code, the device token and `?token=` on
  `/events` cross the LAN in cleartext, so a passive sniffer on the same network can capture a token with full API
  rights. Only enable remote access on networks you trust and revoke devices you no longer use. TLS with a pinned
  local-CA certificate is planned.

## Local CA

A CA created by 1.0.0 carries X.509 **name constraints**: it can only sign certificates for DNS names under `test` and
`localhost`, so even a stolen `rootCA-key.pem` cannot mint a certificate that your browser accepts for a real domain.
If you configure another TLD, the next certificate for it regenerates the CA with that TLD added to the permitted
names (same common name), deletes the old site certificates so they are re-issued, and the new CA has to be trusted
again (`bz trust`, or the prompt in the app). Avoid real public TLDs: a CA permitted for one is trusted for every
name under it. A CA created before 1.0.0 has no constraints and is kept as it is so trust does not break silently; to
get a constrained one, run `bz trust --undo`, delete `$BOZKURT_HOME/ca/` and trust the new CA.

## Unverified downloads

Every runtime and service download is checked against the checksum its upstream publishes. Some upstreams publish
none: the static-php.dev PHP builds used on macOS and Linux, and on Windows the PHP, Go or cloudflared indexes when
they drop a hash. Since 1.0.0 these downloads **fail closed** with `unverified_download`. They go through only when
you opt in, for one install ("Install anyway" in the app, `bz … install --allow-unverified`,
`"allow_unverified": true` on the API) or for all downloads (`allow_unverified = true` in `config.toml`). The first
allowed download is **pinned**: its sha256 is stored in `$BOZKURT_HOME/state.json` (`download_pins`), and any later
download of the same URL that differs is refused with `checksum_mismatch`, even with the opt-in. This is trust on
first use: it protects re-downloads, not the first one, which relies on HTTPS alone. The MCP server has no way to opt
in.

## MCP confirmation for shell commands

An `app` site's `command` runs in a shell and its `env` reaches that process, so `bozkurt-mcp` treats both as code
execution: `sites_create` with `command` or `env`, `sites_update` that changes `command` or sets `env`, and
`sites_start` / `sites_restart` of an app site whose command was not confirmed in the current MCP session return a
preview with the exact command and environment unless called with `confirm: true`. `BOZKURT_MCP_ALLOW_COMMANDS=1`
in the MCP server's environment turns this off for setups you trust. See [docs/MCP.md](docs/MCP.md#shell-commands-need-confirmation).

## Hardening tips for users

- Leave **Remote access** off unless you're using the mobile companion. Revoke paired devices you no longer use
  (Settings → Remote access, or `DELETE /remote/devices/{id}`).
- Never copy `rootCA-key.pem` to another machine. Run `bz trust --undo` before deleting `$BOZKURT_HOME`.
- Check each release against `SHA256SUMS`. The install scripts do this automatically.
- Review what an AI agent asks to `confirm`. A preview tells you exactly what will change, including the exact shell
  command a site will run.
- Leave `allow_unverified` off and use the one-time "Install anyway" / `--allow-unverified` instead, so each
  unverified file is a decision you made.
- Leave `lan_access` off on public or untrusted Wi-Fi.
