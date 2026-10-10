# How BOZKURT CODE compares

This page compares BOZKURT CODE with popular local development tools, using only what each vendor states on its **own public
website**. Every page listed under [Sources](#sources) was fetched on **2026-10-09**. Products change often, so check the
vendor's site before deciding, and please open a PR if a cell is out of date.

**Legend**

- ✅ The source says it does this.
- ❌ The source says it doesn't, or contradicts it (for example, the OS is not in the supported list).
- ❓ Not stated on the pages we checked, or ambiguous. This is **not** the same as "no".
- 💲 Only in a paid tier.

BOZKURT CODE cells describe release **1.0.0** ([ARCHITECTURE](ARCHITECTURE.md)). Some bundled services are not yet on
every OS (see [ROADMAP](ROADMAP.md#known-limitations-in-10)).

## Platforms, licence and price

| | BOZKURT CODE | MAMP / MAMP PRO | XAMPP | Laravel Valet | Laravel Herd | Laragon | ServBay | Docker Desktop | Kettle Code |
|---|---|---|---|---|---|---|---|---|---|
| macOS | ✅ arm64 + x64 | ✅ | ✅ (8.2.4 is the latest OS X build listed) | ✅ | ✅ | ❓ | ✅ macOS 12+ | ✅ ¹ | ✅ macOS 13+, universal |
| Windows | ✅ x64 | ✅ | ✅ | ❌ (macOS only) | ✅ | ✅ | ✅ Windows 10+ / Server 2012+ | ✅ ¹ | ❌ ("Available on macOS only") |
| Linux | ✅ x64 + arm64 | ❓ | ✅ | ❌ | ❌ ("no plans for a Linux version") | ❓ | ❓ (only macOS/Windows listed) | ✅ ¹ | ❌ |
| Open source | ✅ MIT OR Apache-2.0 | ❌ commercial (MAMP GmbH) | ✅ ("open source package") | ✅ (GitHub `laravel/valet`) | ❌ (EULA) | ❌ (EULA, paid licences) | ❓ | ❌ Desktop is subscription-licensed ¹ | ❓ |
| Price | Free | MAMP free. MAMP PRO: lifetime licence plus 1 year of updates, then an annual update renewal. Price ❓ (not rendered on the store page). 14-day trial. | Free | Free | Basic free. Pro **$99**/year licence. Teams **$299** (10 licences) | Free for non-commercial use (with a reminder popup). Commercial **$49**/yr or **$149** lifetime per device. | Free (5 sites, 5 DNS records). Pro **$9.9**/mo or **$99**/yr (shown discounted to $59). Team **$39.9**/mo. | Personal $0. Pro $9–11, Team $15–16, Business $24 per user/month | Free "for a limited time" (early access) |
| Needs Homebrew / package manager | ❌ | ❌ | ❌ | ✅ Homebrew + Composer | ❌ ("zero dependencies") | ❌ (portable folder) | ❌ | ❌ | ❌ (since 3.0) |
| Everything in one folder / clean uninstall | ✅ `~/.bozkurt` | ❓ | ❓ | ❌ (`~/.config/valet` + Homebrew paths) | ❓ | ✅ ("Everything lives in the Laragon folder") | ❓ ("non-intrusive") | ❌ (VM) | ✅ `~/.kettle` |
| Runs natively (no VM/containers) | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ containers | ✅ |

¹ Not stated on the Docker pricing page we fetched. Docker Desktop's OS support and licensing terms are in Docker's own docs.

## Web stack

| | BOZKURT CODE | MAMP PRO | XAMPP | Valet | Herd | Laragon | ServBay | Docker | Kettle Code |
|---|---|---|---|---|---|---|---|---|---|
| Web server | Caddy | Apache and/or Nginx | Apache | Nginx | nginx | Apache, Nginx | ✅ ("web servers") ❓ which | any image | Apache (embedded) |
| `.htaccess` | ❌ (presets generate equivalent rules) | ✅ (Apache) | ✅ (Apache) | ❌ (Nginx drivers) | ❓ | ✅ (Apache) | ❓ | n/a | ✅ |
| PHP versions | 8.0+ (live upstream list) | 7.x – 8.x | One per installer (8.2.12 on Win/Linux) | Homebrew PHP | 7.4 – 8.5 | Multiple (e.g. 8.2–8.4 in its config example) | 5.6 – 8.5 | any image | 8.0 – 8.5 |
| Per-site PHP version | ✅ | ✅ | ❓ | ✅ (`valet isolate`) | ✅ | ❓ | ✅ ("project-level") | ✅ | ✅ |
| Node.js versions | ✅ (all LTS + current) | ❓ | ❓ | ❓ | ✅ (via nvm; free or Pro tier ❓) | ✅ | ✅ 12 – 24 | any image | ✅ LTS |
| Python / Go / Bun | ✅ / ✅ / ✅ | ❓ | ❓ (Perl included) | ❓ | ❓ | ✅ / ✅ / ❓ | ✅ / ✅ / ❓ (+ Java, .NET, Ruby, Rust) | any image | ❓ |
| Runs Node/Python/Go dev servers and proxies to them | ✅ (`app` sites, `$PORT`) | ❓ | ❓ | ✅ proxy only (`valet proxy`) | ❓ | ✅ (Procfile services) | ✅ reverse proxy | ✅ | ✅ reverse proxy to a port |
| Zero-config framework detection | ✅ Laravel, Symfony, WordPress, Drupal, Next, Nuxt, Astro, SvelteKit, Remix, Vite, Express, Django, Flask, FastAPI, Go, static | Laravel/WordPress installers | ❓ | ✅ ~20 PHP frameworks/CMS (drivers) | ❓ | Quick app (WordPress, Laravel, Symfony) | ❓ | ❌ | ✅ PHP version from composer.json |

## Services

| | BOZKURT CODE | MAMP PRO | XAMPP | Valet | Herd | Laragon | ServBay | Kettle Code |
|---|---|---|---|---|---|---|---|---|
| MySQL / MariaDB | ✅ MariaDB (Linux x64, Windows x64 today) or uses an existing one on :3306 | ✅ MySQL 8 / 5.7 | ✅ MariaDB | ❌ (recommends DBngin) | 💲 MySQL, MariaDB | ✅ MySQL | ✅ MySQL 5.1–9.0, MariaDB 10.4–11.7 | ✅ detects existing or bundles MySQL |
| PostgreSQL | ✅ | ❓ | ❓ | ❌ | 💲 | ✅ | ✅ 10–17 | ❓ |
| Redis / Valkey | ✅ Valkey (Linux today) | ❓ | ❓ | ❌ | 💲 | ❓ | ✅ | ❓ |
| Mail catcher | ✅ Mailpit | ✅ Mailhog | ❓ | ❓ | 💲 Mail | ❓ | ✅ (mail server 💲 Pro) | ❓ |
| Other services | — | Snapshots, Blueprints, Transfer/deploy, editor | Perl | — | 💲 Typesense, Meilisearch, MinIO, Reverb; dumps, log viewer, Xdebug | Procfile custom services | MongoDB, Memcached, Ollama, AI gateway | — |

## Networking, TLS and sharing

| | BOZKURT CODE | MAMP PRO | XAMPP | Valet | Herd | Laragon | ServBay | Kettle Code |
|---|---|---|---|---|---|---|---|---|
| Local DNS for a dev TLD | ✅ built-in responder, `*.test` (configurable) | ✅ custom hostnames ❓ mechanism | ❓ | ✅ DnsMasq, `*.test` | ✅ "DNS" | ✅ auto `app.test` | ✅ built-in DNS, custom TLDs | ✅ built-in, `*.test` on :5390 |
| Wildcard subdomains | ✅ | ❓ | ❓ | ✅ | ❓ | ❓ | 💲 wildcard certificates | ✅ |
| One-click HTTPS / local CA | ✅ own CA (no mkcert), user trust store + optional system store | ✅ "certificates are created automatically" | ❓ | ✅ `valet secure` | ❓ | ✅ self-signed, one click | ✅ private CA (more in 💲), ACME | ✅ bundled mkcert, login keychain |
| Public share / tunnel | ✅ cloudflared | ❓ (has deploy/transfer) | ❓ | ✅ ngrok, Expose, cloudflared | ✅ Expose | ✅ ngrok | ✅ ngrok, Pinggy, FRP, … | ❓ |
| Import from other tools | ✅ Valet/Herd/MAMP/XAMPP/Laragon/Homebrew vhosts (their DNS settings are detected and reported, not imported) | ❓ | ❓ | ❌ | ❓ | ❓ | ❓ | ✅ from Homebrew-based Kettle and Apache vhosts |
| Port-conflict detection | ✅ names the owning PID/process | ❓ | ❓ | ❌ (docs: make sure nothing binds :80) | ❓ | ❓ | ✅ (MCP "check ports") | ✅ names the occupant |
| Diagnostics command | ✅ `bz doctor` | ❓ | ❓ | ✅ `valet diagnose` | ❓ | ❓ | ✅ troubleshooting tools | ❓ |

## Interfaces

| | BOZKURT CODE | MAMP PRO | XAMPP | Valet | Herd | Laragon | ServBay | Kettle Code |
|---|---|---|---|---|---|---|---|---|
| GUI | ✅ desktop (Tauri) | ✅ | ❓ | ❌ CLI only | ✅ | ✅ | ✅ | ✅ (SwiftUI) |
| Tray / menu bar | ✅ | ❓ | ❓ | ❌ | ✅ | ✅ tray context menu | ❓ | ✅ |
| CLI | ✅ `bz` (full parity with the GUI) | ❓ | ❓ | ✅ | ✅ (PowerShell / CLI site management) | ❓ (bundles Cmder terminal) | ✅ ("command-line tools") | ❓ |
| MCP server for AI agents | ✅ 55 tools, confirm/preview safety tiers | ✅ (Claude Desktop: files, SQL, WP-CLI) | ❓ | ❓ | ❓ | ❓ | ✅ free (Claude Code, Cursor, Codex) | ✅ 18 tools |
| Phone app | ✅ LAN remote control (iOS/Android) | MAMP Viewer (iOS, for testing sites on devices) | ❓ | ❓ | ❓ | ❓ | Announced ("coming soon": iOS, Android, HarmonyOS) | ❓ |
| UI languages | English, Turkish | ❓ | 15 site languages ❓ app | n/a | ❓ | ❓ | 20+ site languages ❓ app | ❓ |
| Team config in repo | ✅ `bozkurt.toml` | Blueprints | ❓ | `.valetrc` (PHP only) | ✅ `herd.yml` | Profiles | 💲 unified configuration (Team) | ❓ |

## Where others are ahead (honestly)

- **Maturity.** Most products above have years of releases behind them. BOZKURT CODE 1.0.0 is its first public release.
- **Breadth of languages.** ServBay offers Java, .NET, Ruby, Rust, MongoDB and local LLMs (Ollama). BOZKURT CODE covers PHP,
  Node.js, Bun, Python and Go.
- **Apache and `.htaccess`.** MAMP, XAMPP, Laragon and Kettle Code run Apache. If your project depends on `.htaccess` rules
  that the presets do not cover, you will need a per-site Caddy snippet.
- **Older PHP.** Herd goes down to 7.4 and ServBay to 5.6. BOZKURT CODE starts at 8.0.
- **WordPress-specific tooling.** MAMP PRO has snapshots, Blueprints, deploy/transfer and WP-CLI over MCP.
- **Debugging extras.** Herd Pro has dumps, Xdebug detection and a log viewer with jump-to-IDE.

## Sources

All fetched 2026-10-09.

| Product | URL |
|---|---|
| MAMP / MAMP PRO (macOS) | https://www.mamp.info/en/mamp-pro/mac/ |
| MAMP PRO store | https://www.mamp.info/en/store/macos/ |
| XAMPP | https://www.apachefriends.org/ |
| Laravel Valet | https://laravel.com/docs/valet |
| Laravel Herd | https://herd.laravel.com/ |
| Laragon | https://laragon.org/ |
| Laragon pricing | https://laragon.org/pricing |
| ServBay | https://www.servbay.com/ |
| Docker pricing | https://www.docker.com/pricing/ |
| Kettle Code | https://kettlecode.org/ |

Product names and trademarks belong to their owners. BOZKURT CODE is not affiliated with any of them.
