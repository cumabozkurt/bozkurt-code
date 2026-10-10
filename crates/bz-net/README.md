# bz-net

Networking layer of BOZKURT CODE (no dependency on `bz-core`).

| Module | What |
|---|---|
| `dns` | UDP+TCP responder on `127.0.0.1:<port>`: `*.<tld>` (any depth, apex included) → `A 127.0.0.1` / `AAAA ::1`, other types → NOERROR+SOA, other names → `REFUSED` (or `NXDOMAIN`). Live TLD changes. |
| `resolver` | OS routing plans: macOS `/etc/resolver/<tld>`; Linux systemd-resolved drop-in → NetworkManager dnsmasq → hosts-file fallback; Windows NRPT (responder must use port 53) → hosts fallback. Status detection. |
| `tls` | Local CA (rcgen, ECDSA P-256, 10 y, `BOZKURT CODE Local CA <user>@<host>`, key 0600) and leaf certs (domain + aliases + optional wildcards, 825 days, renewed < 30 days / SAN change / CA change). |
| `trust` | Trust plans + status: macOS login keychain, Windows `CurrentUser\Root`, Linux NSS dbs (Chrome + Firefox profiles) + optional system store (update-ca-certificates / update-ca-trust / trust anchor). |
| `ports` | `port_owner` / `udp_port_owner` (pid + process), `is_free`, `find_free_port`. |
| `hosts` | Managed `# BEGIN/END BOZKURT CODE` block in the hosts file. |
| `plan`, `elevation` | Every change outside `$BOZKURT_HOME` is a `Plan` (steps with descriptions, `[admin]` flag). `apply` backs up touched files to `backups/<stamp>/`, runs user steps directly and each block of admin steps through one prompt (osascript / pkexec / `Start-Process -Verb RunAs`). |

Entry point: `NetService::new(NetPaths::from_bozkurt_home(&home))`.
`apply` blocks on a password dialog — call it from `spawn_blocking`.
