# bz-daemon (`bozkurtd`)

The BOZKURT CODE daemon: HTTP API v1 (`docs/API.md`) on `127.0.0.1:<api_port>/api/v1`
plus the `/events` WebSocket, over `bz-core` with networking from `bz-net`.

```text
bozkurtd [--home DIR] [--port N] [--foreground] [--version]
```

* Without `--foreground` it detaches and prints the child pid. One instance per home
  (`$BOZKURT_HOME/bozkurtd.pid`). SIGINT/SIGTERM stop every supervised process.
* Logs: stderr + `$BOZKURT_HOME/logs/bozkurtd.log` (`RUST_LOG` overrides the `info` filter).
* At start: local CA ensured, `autostart` services started (DNS responder, Caddy, ...),
  Caddy config applied, remote listener started when `[remote] enabled = true`.

## Remote access
`[remote] enabled/port` (or `POST /remote/enable`). Listens on `0.0.0.0:<port>`, every
request needs `Authorization: Bearer <token>` (`/events` also accepts `?token=`), except
`POST /remote/pair`. Pairing codes: 8 digits, 5 minutes, single use, generated only on the
local listener; `/remote/pair` allows 5 attempts per minute per client address and 30 per
minute in total (429 `rate_limited`), and revokes the code after 10 wrong guesses from any
address (429 `pairing_locked`; generate a new code). Tokens are stored as SHA-256 hashes in
`$BOZKURT_HOME/remote.json`. Advertised as `_bozkurt._tcp` via mDNS.

**Limitation:** the remote listener is plain HTTP. The pairing code, the bearer token and
`?token=` travel in cleartext, so anyone sniffing the same network can steal a device token.
Enable remote access only on trusted networks; TLS with a pinned local-CA leaf is planned.

## Databases
`/services/{mariadb|mysql|postgres}/databases` use the bundled client (`mariadb`/`mysql`,
`psql`) or one on PATH for an external server, over TCP 127.0.0.1. Defaults: MariaDB `root`
with no password, PostgreSQL `postgres` (trust). Override in `config.toml`:

```toml
[databases.mariadb]
user = "root"
password = "secret"
```

## Share
`POST /share/{site_id}` downloads cloudflared on first use (GitHub releases, digest-verified
when GitHub publishes one) into `$BOZKURT_HOME/services/cloudflared/`, opens a quick tunnel
and emits `share.url`. `GET /share/{site_id}` returns the current URL.
