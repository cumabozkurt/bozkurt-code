# BOZKURT CODE: User Guide

*[Türkçe](USER_GUIDE.tr.md)*

This guide takes you from installation to a working `https://myapp.test`, then covers everyday tasks in the desktop app
and on the command line. For exact command syntax see [CLI.md](CLI.md). For AI agents see [MCP.md](MCP.md).

This guide covers release 1.0. Platform limits (for example which OSes get a bundled MariaDB) are listed in the
[README](../README.md) and the [ROADMAP](ROADMAP.md#known-limitations-in-10).

## Contents

1. [Concepts](#1-concepts)
2. [Install](#2-install)
3. [First run: Quick setup](#3-first-run-quick-setup)
4. [Sites](#4-sites)
5. [Runtimes (PHP, Node.js, Python, Bun, Go)](#5-runtimes)
6. [Services and databases](#6-services-and-databases)
7. [HTTPS and the local CA](#7-https-and-the-local-ca)
8. [Local DNS](#8-local-dns)
9. [Logs, jobs and Doctor](#9-logs-jobs-and-doctor)
10. [Sharing a site publicly](#10-sharing-a-site-publicly)
11. [Mobile companion](#11-mobile-companion)
12. [AI agents (MCP)](#12-ai-agents-mcp)
13. [Importing from MAMP, Valet, Herd, XAMPP, Laragon](#13-importing-from-other-tools)
14. [Settings and files](#14-settings-and-files)
15. [Uninstall](#15-uninstall)
16. [Troubleshooting](#16-troubleshooting)

## 1. Concepts

- **Home folder.** Everything BOZKURT installs lives in one folder:
  - macOS/Linux: `~/.bozkurt`
  - Windows: `%LOCALAPPDATA%\Bozkurt`
  - Set `BOZKURT_HOME` to use a different folder.
- **Daemon.** `bozkurtd` is a small background process. It owns the services and serves a local API on `127.0.0.1:7878`. The
  desktop app, `bz`, `bozkurt-mcp` and the phone app are all clients of it, so it doesn't matter which one you use.
  Web pages cannot use this API: `bz` and `bozkurt-mcp` authenticate with a token the daemon keeps in
  `$BOZKURT_HOME/api-token`, which they read automatically (details in [SECURITY.md](../SECURITY.md#local-api-protection)).
- **Site.** A project folder mapped to a domain such as `shop.test`. There are four kinds:

  | Kind | What happens |
  |---|---|
  | `php` | Caddy serves files and sends `.php` to a PHP-FPM pool of the site's PHP version (php-cgi on Windows). |
  | `app` | BOZKURT runs your dev command (e.g. `npm run dev`), sets `PORT`, and Caddy proxies to it. |
  | `proxy` | Caddy forwards the domain to a port you already run something on. |
  | `static` | Caddy serves files from a folder (e.g. `dist/`). |

- **Runtime.** An installed version of PHP, Node.js, Python, Bun or Go. One version of each is *global*, linked under
  `current/` with commands in `bin/`. Each site can pin its own version.
- **Service.** A long-running process BOZKURT supervises: `caddy`, `dns`, `mariadb`, `postgres`, `redis`, `mailpit`, and
  `php-fpm-X.Y` pools.
- **Job.** Anything slow, such as a download or the DNS setup, runs as a job with a progress bar. You can close the window
  and the job keeps going.

## 2. Install

**Desktop app:** download the installer for your OS from GitHub Releases (`.dmg`, `.msi` / `-setup.exe`, `.deb` / `.rpm` /
`.AppImage`) and open it. The app includes `bozkurtd`, `bz` and `bozkurt-mcp` inside its bundle. To use `bz` from a terminal
or register `bozkurt-mcp` with an editor, also run the CLI installer below.

**CLI only:**

```sh
# macOS / Linux
curl -fsSL https://raw.githubusercontent.com/cumabozkurt/bozkurt-code/main/scripts/install.sh | sh
```
```powershell
# Windows
irm https://raw.githubusercontent.com/cumabozkurt/bozkurt-code/main/scripts/install.ps1 | iex
```

The scripts check every download against the release's `SHA256SUMS` and install into `$BOZKURT_HOME/bin`. They never need
root. If that folder isn't on your `PATH`, they print the exact line to add (on Windows, re-run with `-AddToPath`).

**Shell completions:** `bz completions zsh|bash|fish|powershell`.

## 3. First run: Quick setup

When you open the app, **Overview** shows what is ready and what is missing. Click **Run quick setup** and it walks you through:

1. **Ports.** Checks 80/443. If another program (Apache, nginx, another dev tool) holds them, BOZKURT shows which process it
   is. You decide: stop that program yourself, or use 8080/8443 instead.
2. **DNS.** Routes `*.test` to BOZKURT. Before anything changes you see which file or rule will be written; the OS then shows
   its own admin prompt. The previous state is backed up.
3. **HTTPS.** Trusts the BOZKURT local CA in *your user* certificate store. On macOS and Windows this normally needs no admin rights.
4. **Runtimes.** Installs the PHP and Node.js versions you choose.
5. **Services.** Optionally installs MariaDB, PostgreSQL, Redis and Mailpit and sets them to autostart.

From the terminal, the same steps are:

```sh
bz doctor
bz dns setup
bz trust
bz php install 8.3 && bz node install lts
bz start all
```

## 4. Sites

### Create a site

**App:** **Sites → New site**, choose the folder. BOZKURT detects the framework and pre-fills the domain, kind, document
root and versions. Adjust anything, enable HTTPS, then **Create**.

**CLI:**

```sh
cd ~/code/shop
bz link --https                 # → https://shop.test
bz link --domain store          # → store.test
bz link ./frontend --domain app --command "npm run dev" --https
bz link --domain api --proxy 8000          # something already listening on :8000
```

What detection recognises:

| Framework | Kind | Root | Command |
|---|---|---|---|
| Laravel, Symfony | php | `public` | — |
| WordPress | php | project root (or `web` for Bedrock) | — |
| Drupal | php | `web` or root | — |
| Next.js, Nuxt, Astro, Remix / React Router | app | — | dev script with `--port $PORT` (e.g. `npm run dev -- --port $PORT`) |
| Vite, SvelteKit | app | — | dev script with `--port $PORT --strictPort` |
| Express / Fastify / Koa | app | — | `dev` script (or `start`) |
| Django | app | — | `python manage.py runserver 127.0.0.1:$PORT` |
| FastAPI | app | — | `python -m uvicorn main:app --host 127.0.0.1 --port $PORT --reload` |
| Flask | app | — | `python -m flask run --host 127.0.0.1 --port $PORT --debug` |
| Go (`go.mod`) | app | — | `go run .` |
| `index.php` / `composer.json` | php | `public` if `public/index.php` exists | — |
| Anything else | static | `public`, `dist`, `build`, `_site` or `out` if it holds `index.html` | — |

The package manager (npm, pnpm, yarn or bun) is picked from your lock file.

### Per-project file `bozkurt.toml`

Commit this file to share site settings with your team. Settings you pass explicitly win over the file, and the file wins over detection.

```toml
domain = "shop"             # → shop.test
type = "php"                # php | proxy | static | app
root = "public"
php = "8.3"
node = "22"
command = "npm run dev"     # for type = "app"
aliases = ["admin.shop"]
services = ["mariadb", "redis", "mailpit"]

[env]
APP_ENV = "local"
```

### Change, start and stop sites

- **Site detail** in the app lets you change the domain, aliases, root, PHP/Node version, environment variables, HTTPS and wildcard
  subdomains. For `app` sites it also has **Start / Stop / Restart** and live logs.
- `bz open` opens the current folder's site in your browser. `bz sites` lists all of them.
- `bz unlink` removes the site from BOZKURT. **Your project files are never touched.**

### Custom Caddy rules

BOZKURT uses Caddy, which does not read `.htaccess`. WordPress, Laravel, Symfony and generic front-controller sites get the
equivalent rules automatically. For anything else, create `$BOZKURT_HOME/caddy/snippets/<domain>.caddy`. It is imported
at the end of that site's block:

```caddy
# ~/.bozkurt/caddy/snippets/shop.test.caddy
header /assets/* Cache-Control "no-store"
redir /old-blog/* /blog{uri} permanent
```

## 5. Runtimes

**App:** go to **Runtimes**, pick a tab (PHP, Node.js, Python, Bun, Go), then **Install** or **Use** on a version. The list comes
live from upstream and shows LTS/current/EOL badges.

**CLI:**

```sh
bz php list --available
bz php install 8.4
bz php use 8.4            # global default
bz node install lts
bz python install 3.12
bz go install latest
bz php uninstall 8.1
```

**PHP on macOS and Linux: "Install without a checksum?"** PHP for macOS and Linux comes from
[static-php.dev](https://static-php.dev), which publishes no checksums, so BOZKURT cannot verify the download and refuses it by
default. The app then shows a dialog explaining this; choose **Install anyway** to trust the file once. Its SHA-256 is then
**pinned**: every later download of the same file must match it exactly, or it is refused. On the CLI the install fails with
`unverified download: …`; re-run it with the flag:

```sh
bz php install 8.4 --allow-unverified
```

A version you installed once this way reinstalls without asking. To allow every unverified download, set
`allow_unverified = true` in `config.toml` (not recommended). Windows PHP builds normally come with checksums and install without asking.

**Per-site versions.** A site uses the version pinned in its settings or `bozkurt.toml`. If neither has one, the project files decide:

| Runtime | Read from |
|---|---|
| PHP | `composer.json` → `require.php` |
| Node.js | `.nvmrc`, `.node-version`, `package.json` → `engines.node` |
| Python | `.python-version` |
| Go | `go.mod` |

**In your terminal:** the shims in `$BOZKURT_HOME/bin` (`php`, `composer`, `node`, `npm`, `npx`, `bun`, `python`, `pip`, `go`)
run the project's version when you're inside a project folder and the global version everywhere else. `composer` is
downloaded and verified automatically.

## 6. Services and databases

**App:** the **Services** page shows each service's status, port, version and uptime, with Start/Stop/Restart, autostart and logs.
**CLI:** `bz start mariadb`, `bz stop all`, `bz restart caddy`, `bz status`.

| Service | Default port | Notes |
|---|---|---|
| caddy | 80 / 443 (fallback 8080 / 8443) | Web server for all sites |
| dns | 5390 (UDP+TCP) | Answers `*.test` |
| mariadb | 3306 | User `root`, no password (local development only). Bundled for Linux x64 and Windows x64 today. |
| postgres | 5432 | User `postgres`, trust authentication on localhost |
| redis | 6379 | Valkey. Bundled for Linux today. |
| mailpit | SMTP 1025, web UI 8025 | Point your app's mailer at `127.0.0.1:1025` and open `http://127.0.0.1:8025` |
| php-fpm-X.Y | socket (Windows: TCP 9000+X·10+Y, e.g. 8.3 → 9083) | One pool per PHP version in use |

**Already have MySQL, PostgreSQL or Redis?** If something else is listening on the default port, the service shows as
**external** and BOZKURT uses it as it is, without changing its data or config. This is also how to use a database on a
platform where BOZKURT doesn't bundle one yet.

**Databases:** `bz db create shop`, `bz db list`, `bz db drop shop` (this asks for confirmation).

**Laravel `.env` example:**

```dotenv
DB_CONNECTION=mariadb
DB_HOST=127.0.0.1
DB_PORT=3306
DB_DATABASE=shop
DB_USERNAME=root
DB_PASSWORD=
MAIL_HOST=127.0.0.1
MAIL_PORT=1025
REDIS_HOST=127.0.0.1
```

## 7. HTTPS and the local CA

- BOZKURT creates its own certificate authority in `$BOZKURT_HOME/ca/`. The private key is readable only by you.
- The CA is added to your user stores:
  - macOS: your **login** keychain;
  - Windows: **CurrentUser\Root**;
  - Linux: your NSS databases (Chrome/Chromium and Firefox profiles).

  `bz trust` also adds it to the system store, which needs admin rights (the OS asks for your password).
- Each HTTPS site gets a certificate for its domain and aliases, plus `*.domain` if wildcard is on. Certificates are renewed
  automatically before they expire.
- Toggle per site in **Site detail**, or with `bz secure` / `bz unsecure`. HTTP requests redirect to HTTPS when it is on.
- Command-line tools that don't use the OS store (some versions of `curl`, Node.js, Python `requests`) need to be pointed at the CA:
  ```sh
  export NODE_EXTRA_CA_CERTS="$HOME/.bozkurt/ca/rootCA.pem"
  export SSL_CERT_FILE="$HOME/.bozkurt/ca/rootCA.pem"   # careful: replaces the default bundle for that process
  ```
- `bz trust --undo` removes the CA from every store it was added to.

## 8. Local DNS

- The built-in responder answers every name under your TLD (`shop.test`, `api.shop.test`, …) with `127.0.0.1` and `::1`.
- `bz dns setup` tells your OS to send `.test` lookups to it:

  | OS | Method | Fallback |
  |---|---|---|
  | macOS | `/etc/resolver/test` | — |
  | Linux | systemd-resolved drop-in, or NetworkManager's dnsmasq | managed block in `/etc/hosts` |
  | Windows | NRPT rule for `.test` (the responder must then listen on port 53) | managed block in the hosts file |

  The hosts-file fallback can't do wildcards, so each site and alias gets its own line.
- **Windows: port 53 in use.** NRPT needs the responder on port 53. When another program already holds it (often
  Internet Connection Sharing, Hyper-V/WSL or Docker Desktop), `bz dns setup` uses the hosts file instead and says
  why, naming the program, e.g. `127.0.0.1:53 is in use by svchost.exe (pid 1234) … Sites are listed in the hosts file
  instead; wildcard subdomains do not resolve.` Free port 53 and run `bz dns setup` again to switch to NRPT.
- **Change the TLD:** Settings → General → TLD, or `bz config set tld local`. `.dev` and `.app` are rejected because they
  are real public TLDs that force HTTPS. Run `bz dns setup` again afterwards.
  **Trust the CA again after a TLD change.** The local CA only signs certificates for `.test` and `localhost` (so it cannot be
  abused for real websites). When you switch to another TLD, BOZKURT renews the CA to include it and re-issues the site
  certificates, and browsers warn until you trust the new CA: the app shows a **Trust CA** prompt, or run `bz trust`. Pick a
  reserved name (`test`, `localhost`, or a made-up one like `loc`) rather than a real public TLD.
- `bz dns setup --undo` restores the previous configuration from the backup.

## 9. Logs, jobs and Doctor

- **Logs:** the Logs page streams every service and site live, with filtering. CLI: `bz logs caddy -f`, `bz logs shop.test -n 500`.
  Files are kept in `$BOZKURT_HOME/logs/`.
- **Jobs:** the Jobs drawer (bottom right) shows downloads and setup tasks with progress. CLI: `bz jobs`.
- **Doctor:** the Doctor page, or `bz doctor`, checks ports, DNS resolution, CA trust, `PATH`, permissions and services. Each
  warning or failure comes with a suggested fix.

## 10. Sharing a site publicly

`bz share shop.test`, or **Share** in Site detail, opens a temporary public URL through Cloudflare's `cloudflared`, which is
downloaded on first use. Anyone with the URL can reach the site while sharing is on. Stop it with Ctrl-C or **Stop sharing**.

## 11. Mobile companion

The phone app (iOS/Android) is a **remote control** for your computer. Phones cannot run PHP, MySQL or dev servers, so your
computer stays the server and the phone controls it over your local network.

1. **Desktop:** Settings → **Remote access** → enable. This starts a listener on port 7879, which is off by default. Click
   **Generate pairing code**. A QR code and an 8-digit code appear (shown as `1234 5678`), valid for 5 minutes and usable once.
2. **Phone:** open the app → **Scan QR code**, or type `<computer-ip>:7879` and the code.
3. Done. The phone gets its own token. Revoke it any time under Settings → Remote access → paired devices, or with `bz remote revoke <id>`.

Both devices must be on the same network. If the connection fails, allow port 7879 in your computer's firewall. If enabling
remote access says the port is in use, remote access stays off; free port 7879 or set another one with
`bz config set remote.port 7880`.

Wrong codes are limited: after 5 tries a minute from one phone (30 from all devices) you have to wait a minute, and after 10
wrong codes in total the code is revoked. Generate a new code on the computer and enter that one. The connection between
phone and computer is not encrypted yet, so only use remote access on networks you trust. The phone app
has a **Try the demo** mode that works without a computer.

## 12. AI agents (MCP)

Register the MCP server with your editor or agent:

```sh
bz mcp register claude-code     # or: cursor, vscode, windsurf, gemini, codex
```

Then ask things like "link this repo with HTTPS and PHP 8.3" or "why is shop.test returning 502?". Tools that delete
data, touch the OS or expose a site publicly first return a **preview**. The agent must show it to you and call again with
`confirm: true`. The same goes for anything that sets or runs a site's shell command or environment: you see the exact command
before it runs. Remote access, pairing and import are not available to agents. See [MCP.md](MCP.md).

## 13. Importing from other tools

**App:** Settings → **Import**. **CLI:** `bz import`.

BOZKURT scans for sites and DNS settings from Laravel Valet, Laravel Herd, MAMP, XAMPP, Laragon and Homebrew (Apache/nginx/dnsmasq),
then lists what it found. Only the items you select are imported. Every file it changes outside `$BOZKURT_HOME` is backed
up first to `$BOZKURT_HOME/backups/<timestamp>/`. DNS settings are listed but **never imported**: BOZKURT does not
change another tool's resolver. The import result names each one under "DNS not imported" and warns when an old
resolver also handles your TLD (for example Valet's `/etc/resolver/test`). Remove that file yourself, otherwise the
old tool may keep answering for your sites. Selecting only DNS entries imports nothing.

## 14. Settings and files

**Settings page:** language (English / Türkçe), theme (light/dark/system), TLD, ports, network (serve sites on the local
network; whether unverified downloads are allowed), remote access and import. Ports must be between 1 and 65535 and all
different, including the API and remote ports; the daemon's message appears under the field.

Values can also be changed with `bz config set <key> <value>` (dotted keys such as `remote.port`).

`$BOZKURT_HOME/config.toml` (defaults shown):

```toml
tld = "test"
dns_port = 5390
http_port = 80          # falls back to 8080
https_port = 443        # falls back to 8443
api_port = 7878         # daemon API, 127.0.0.1 only
autostart = ["caddy", "dns"]
lan_access = false      # true: serve sites on every network interface, not only 127.0.0.1 / ::1
allow_unverified = false  # true: accept every download without a published checksum (pinned on first use)

[remote]
enabled = false
port = 7879
```

Folder layout:

```
bin/        runtime shims (php, composer, node, npm, npx, corepack, bun, bunx, python, python3, pip, pip3, go, gofmt);
            the CLI installer also puts bz, bozkurtd and bozkurt-mcp here
runtimes/   <kind>/<version>/        current/<kind> → active version
services/   <name>/ binaries and data (e.g. services/mariadb/data)
caddy/      Caddyfile (generated), bin/, data/, snippets/<domain>.caddy
ca/         rootCA.pem, rootCA-key.pem
certs/      <domain>.pem, <domain>-key.pem
logs/       <component>.log
cache/      upstream version lists (6 h)
tmp/        downloads in progress
backups/    <stamp>/ copies of anything changed outside this folder
config.toml, state.json, api-token (local API token, 0600), bozkurtd.pid
```

**Keyboard:** ⌘K / Ctrl K opens the command palette, and `?` lists all shortcuts.

## 15. Uninstall

1. `bz dns setup --undo` and `bz trust --undo`. These remove the OS resolver entry and the CA trust, restoring the backups.
2. `bz stop all`, then quit the app.
3. Delete the app (Applications, Programs and Features, or your package manager).
4. Delete `$BOZKURT_HOME`. **This removes your BOZKURT-managed databases**, so export anything you need first.

## 16. Troubleshooting

| Symptom | Try |
|---|---|
| `shop.test` doesn't resolve | `bz doctor`. Then `bz dns setup`. On Linux with a custom resolver, check that the systemd-resolved drop-in or hosts block exists. On Windows, port 53 must be free for NRPT, otherwise the hosts fallback is used. |
| Browser warns about the certificate | `bz trust`, then restart the browser. On Firefox (Linux), make sure the profile's NSS db was updated. Check `bz trust status`. |
| Port 80/443 already in use | `bz doctor` names the process. Stop it, or accept 8080/8443 (URLs then become `http://shop.test:8080`). |
| 502 on an `app` site | The dev command crashed or listens on the wrong port. Check `bz logs shop.test`. Your command must use `$PORT`. A Vite / SvelteKit / Astro site created before 1.0 may need `--host 127.0.0.1` added to its command (new sites get it automatically). |
| Windows: only exact site names resolve, not `*.shop.test` | Port 53 is taken, so DNS uses the hosts file. `bz dns setup` names the program holding it; free the port and run it again. |
| Windows: a `^` in a quoted `npm` / `npx` argument arrives doubled | Known limit in 1.0 for `npm`, `npx` and `corepack` (they are `.cmd` files). Avoid `^` inside quoted arguments, e.g. write `"react@~18"` or an exact version, or put the range in `package.json`. `node`, `php`, `composer`, `python`, `go` and `bun` are not affected. |
| "PHP X is not installed" page | `bz php install X` (or change the site's PHP version). |
| MariaDB shows `external` | Another MySQL/MariaDB holds :3306. BOZKURT is using it, so connect with *its* credentials. |
| `bz: daemon unreachable` (exit 3) | Open the desktop app or run `bz daemon`. Check `$BOZKURT_HOME/logs/bozkurtd.log`. |
| `401` / "missing local API token" from a script | Send `Authorization: Bearer $(cat ~/.bozkurt/api-token)`. With `bz`, make sure `--home` / `BOZKURT_HOME` matches the running daemon. |
| Phone can't connect | Same Wi-Fi? Remote access enabled? Firewall allows 7879? Is the pairing code less than 5 minutes old? |
| "Too many wrong codes: the pairing code was revoked" | Generate a new pairing code on the computer and use that one. |
| PHP install fails with "unverified download" | Expected on macOS/Linux: choose **Install anyway**, or `bz php install <v> --allow-unverified`. See [Runtimes](#5-runtimes). |
| "differs from the copy pinned on its first download" | A file changed since you first installed it. It was refused on purpose; report it if it keeps happening. |
| `bozkurtd` doesn't start after editing `config.toml` | A port is out of range or used twice; the log names the key (`https_port (80) is already used by http_port`). |

Still stuck? Open an issue with the output of `bz doctor --json` (remove any private paths first).
