//! Caddyfile generation from sites + config, and hot reload through Caddy's admin API.
//!
//! Layout of the generated file:
//! * global options (admin endpoint, ports, no automatic redirects/trust — we manage both),
//! * one shared HTTP→HTTPS redirect block for every HTTPS host,
//! * per site a snippet `(bz-site-<id>)` holding the handlers, imported by one block per
//!   certificate group (the domain [+ `*.domain`], and each alias that the domain's
//!   certificate does not cover),
//! * framework presets replace `.htaccess` (never parsed): dotfile protection for all PHP
//!   sites, WordPress/Drupal sensitive files, Symfony "front controller only".
//!   Laravel and generic front-controller apps are fully served by `php_fastcgi`'s
//!   built-in `try_files {path} {path}/index.php index.php`.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::error::{CoreError, Result};
use crate::models::{Site, SiteKind};

/// Caddy's stock admin endpoint. BOZKURT no longer uses it (P1-3: a user's own Caddy
/// listens there); kept for tests and docs.
pub const ADMIN_ADDR: &str = "127.0.0.1:2019";

/// Where our Caddy's admin API listens (P1-3). Unix: a socket in `caddy/run/` (dir mode
/// 0700, so only this user can reconfigure the proxy). Windows, or a socket path too long
/// for `sun_path`: a free loopback port picked when Caddy starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdminEndpoint {
    Tcp(u16),
    #[cfg(unix)]
    Unix(PathBuf),
}

impl AdminEndpoint {
    /// Pick the endpoint for a Caddy about to start.
    pub fn choose(paths: &crate::paths::Paths) -> std::io::Result<AdminEndpoint> {
        #[cfg(unix)]
        {
            let sock = paths.caddy_run().join("admin.sock");
            // sun_path is 104 bytes on macOS, 108 on Linux
            if sock.as_os_str().len() < 100 {
                return Ok(AdminEndpoint::Unix(sock));
            }
            tracing::warn!(
                "caddy admin socket path too long ({}); using a loopback port",
                sock.display()
            );
        }
        let _ = paths;
        Ok(AdminEndpoint::Tcp(crate::ports::free_port()?))
    }

    /// Prepare the endpoint before Caddy starts: create `caddy/run/` (0700) and remove a
    /// stale socket.
    pub fn prepare(&self) -> std::io::Result<()> {
        match self {
            AdminEndpoint::Tcp(_) => Ok(()),
            #[cfg(unix)]
            AdminEndpoint::Unix(p) => {
                use std::os::unix::fs::PermissionsExt;
                if let Some(dir) = p.parent() {
                    std::fs::create_dir_all(dir)?;
                    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
                }
                match std::fs::remove_file(p) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                    _ => Ok(()),
                }
            }
        }
    }

    /// The value for the Caddyfile `admin` option and `caddy reload --address`.
    pub fn address(&self) -> String {
        match self {
            AdminEndpoint::Tcp(p) => format!("127.0.0.1:{p}"),
            #[cfg(unix)]
            AdminEndpoint::Unix(p) => format!("unix/{}", p.display()),
        }
    }

    /// Base URL for requests (over the socket the host is only a placeholder; Caddy
    /// accepts `127.0.0.1` as the Host of unix-socket admin requests).
    pub fn base_url(&self) -> String {
        match self {
            AdminEndpoint::Tcp(p) => format!("http://127.0.0.1:{p}"),
            #[cfg(unix)]
            AdminEndpoint::Unix(_) => "http://127.0.0.1".into(),
        }
    }

    /// A no-proxy client that reaches this endpoint.
    pub fn client(&self) -> reqwest::Client {
        let b = reqwest::Client::builder()
            .user_agent(crate::download::USER_AGENT)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(30));
        #[cfg(unix)]
        let b = match self {
            AdminEndpoint::Unix(p) => b.unix_socket(p.as_path()),
            AdminEndpoint::Tcp(_) => b,
        };
        b.build().expect("reqwest client")
    }

    /// The TCP port, if any (for "is this port ours").
    pub fn tcp_port(&self) -> Option<u16> {
        match self {
            AdminEndpoint::Tcp(p) => Some(*p),
            #[cfg(unix)]
            AdminEndpoint::Unix(_) => None,
        }
    }

    /// Wait until the admin API accepts connections.
    pub async fn wait_ready(&self, timeout: std::time::Duration) -> bool {
        match self {
            AdminEndpoint::Tcp(p) => crate::ports::wait_listening(*p, timeout).await,
            #[cfg(unix)]
            AdminEndpoint::Unix(p) => {
                let deadline = std::time::Instant::now() + timeout;
                loop {
                    if tokio::net::UnixStream::connect(p).await.is_ok() {
                        return true;
                    }
                    if std::time::Instant::now() >= deadline {
                        return false;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }
        }
    }
}

/// A certificate group: hosts served with one certificate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertGroup {
    /// Host used to request the certificate (and key of [`CaddyInput::certs`]).
    pub primary: String,
    /// Whether the certificate must also cover `*.primary`.
    pub wildcard: bool,
    /// Hosts to put on the site block.
    pub hosts: Vec<String>,
}

/// Split a site's hostnames into certificate groups.
pub fn cert_groups(site: &Site) -> Vec<CertGroup> {
    let mut main = CertGroup {
        primary: site.domain.clone(),
        wildcard: site.wildcard,
        hosts: vec![site.domain.clone()],
    };
    if site.wildcard {
        main.hosts.push(format!("*.{}", site.domain));
    }
    let mut extra = vec![];
    for a in &site.aliases {
        if a == &site.domain
            || main.hosts.contains(a)
            || extra.iter().any(|g: &CertGroup| &g.primary == a)
        {
            continue;
        }
        let covered = site.wildcard
            && a.strip_suffix(&format!(".{}", site.domain))
                .is_some_and(|label| !label.is_empty() && !label.contains('.'));
        if covered {
            main.hosts.push(a.clone());
        } else {
            extra.push(CertGroup {
                primary: a.clone(),
                wildcard: false,
                hosts: vec![a.clone()],
            });
        }
    }
    let mut groups = vec![main];
    groups.extend(extra);
    groups
}

/// Everything the generator needs. Pure data so output is deterministic.
#[derive(Debug, Clone)]
pub struct CaddyInput<'a> {
    pub sites: &'a [Site],
    pub http_port: u16,
    pub https_port: u16,
    pub admin: &'a str,
    pub logs_dir: PathBuf,
    /// Certificate files by [`CertGroup::primary`]. A missing HTTPS group falls back to
    /// `tls internal` (Caddy's own CA) so the config still loads.
    pub certs: BTreeMap<String, (PathBuf, PathBuf)>,
    /// FastCGI upstream for PHP sites by site id: `unix//path/fpm.sock` or `127.0.0.1:9083`.
    pub php_upstreams: BTreeMap<String, String>,
    /// Existing custom snippet files by site id (imported last in the site block).
    pub snippets: BTreeMap<String, PathBuf>,
    /// Serve sites on every interface (`config.lan_access`). Off by default: every site
    /// block binds `127.0.0.1` and `::1` only (`default_bind`).
    pub lan: bool,
}

/// Quote one Caddyfile token so it can never end the token, the line or the block.
///
/// - Control characters (newlines, tabs, NUL, …) are dropped; [`crate::sites::validate`]
///   already refuses them, this is defence in depth.
/// - Without a backslash: a double-quoted token with `"` escaped as `\"`.
/// - With a backslash (regexes, Windows-style values): a backtick raw token, where Caddy
///   escapes nothing, so the value is passed through byte for byte.
/// - Both a backslash and a backtick: the backslashes become `/` (the only lossy case;
///   `validate` refuses such values for fields that end up here).
pub fn q(s: &str) -> String {
    let s: String = s.chars().filter(|c| !c.is_control()).collect();
    if !s.contains('\\') {
        return format!("\"{}\"", s.replace('"', "\\\""));
    }
    if !s.contains('`') {
        return format!("`{s}`");
    }
    let s = s.replace('\\', "/");
    format!("\"{}\"", s.replace('"', "\\\""))
}

/// Quote a filesystem path (`\` → `/`, which Caddy accepts on Windows too).
fn qp(p: &Path) -> String {
    q(&p.to_string_lossy().replace('\\', "/"))
}

/// Characters allowed in free text that lands in a `#` comment line.
fn comment_safe(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .take(64)
        .collect()
}

/// Absolute document root of a site.
pub fn site_root(site: &Site) -> PathBuf {
    let base = PathBuf::from(&site.path);
    match site.root.as_deref() {
        None | Some("") | Some(".") => base,
        Some(r) if Path::new(r).is_absolute() => PathBuf::from(r),
        Some(r) => base.join(r),
    }
}

/// Render the Caddyfile.
pub fn generate(input: &CaddyInput<'_>) -> String {
    let mut o = String::new();
    o.push_str("# Generated by BOZKURT CODE - do not edit; changes are overwritten.\n");
    o.push_str("# Per-site customisation: caddy/snippets/<domain>.caddy\n");
    let _ = writeln!(o, "{{");
    let _ = writeln!(o, "\tadmin {}", input.admin);
    let _ = writeln!(o, "\thttp_port {}", input.http_port);
    let _ = writeln!(o, "\thttps_port {}", input.https_port);
    let _ = writeln!(o, "\tauto_https disable_redirects");
    let _ = writeln!(o, "\tskip_install_trust");
    let _ = writeln!(o, "\tpersist_config off");
    if !input.lan {
        let _ = writeln!(o, "\tdefault_bind 127.0.0.1 [::1]");
    }
    let _ = writeln!(o, "}}");

    let mut sites: Vec<&Site> = input.sites.iter().collect();
    sites.sort_by(|a, b| a.domain.cmp(&b.domain));

    // HTTP → HTTPS redirect for all https hosts
    let https_hosts: Vec<String> = sites
        .iter()
        .filter(|s| s.https)
        .flat_map(|s| cert_groups(s).into_iter().flat_map(|g| g.hosts))
        .collect();
    if !https_hosts.is_empty() {
        let list: Vec<String> = https_hosts.iter().map(|h| format!("http://{h}")).collect();
        let port = if input.https_port == 443 {
            String::new()
        } else {
            format!(":{}", input.https_port)
        };
        let _ = writeln!(o, "\n# HTTP -> HTTPS");
        let _ = writeln!(o, "{} {{", list.join(", "));
        let _ = writeln!(o, "\tredir https://{{host}}{port}{{uri}} 308");
        let _ = writeln!(o, "}}");
    }

    for s in sites {
        let snippet = format!("bz-site-{}", s.id);
        let _ = writeln!(
            o,
            "\n# site {} ({}{})",
            comment_safe(&s.id),
            s.kind.as_str(),
            s.framework
                .as_deref()
                .map(|f| format!(", {}", comment_safe(f)))
                .unwrap_or_default()
        );
        let _ = writeln!(o, "({snippet}) {{");
        site_body(&mut o, s, input);
        let _ = writeln!(o, "}}");
        for g in cert_groups(s) {
            let hosts: Vec<String> = if s.https {
                g.hosts.clone()
            } else {
                g.hosts.iter().map(|h| format!("http://{h}")).collect()
            };
            let _ = writeln!(o, "{} {{", hosts.join(", "));
            if s.https {
                match input.certs.get(&g.primary) {
                    Some((c, k)) => {
                        let _ = writeln!(o, "\ttls {} {}", qp(c), qp(k));
                    }
                    None => {
                        let _ = writeln!(o, "\ttls internal");
                    }
                }
            }
            let _ = writeln!(o, "\timport {snippet}");
            let _ = writeln!(o, "}}");
        }
    }
    o
}

fn site_body(o: &mut String, s: &Site, input: &CaddyInput<'_>) {
    let log = input.logs_dir.join(format!("site-{}.log", s.id));
    let _ = writeln!(o, "\tencode zstd gzip");
    let _ = writeln!(o, "\tlog {{");
    let _ = writeln!(o, "\t\toutput file {} {{", qp(&log));
    let _ = writeln!(o, "\t\t\troll_size 5MiB");
    let _ = writeln!(o, "\t\t\troll_keep 3");
    let _ = writeln!(o, "\t\t}}");
    let _ = writeln!(o, "\t}}");
    match s.kind {
        SiteKind::Php => {
            let _ = writeln!(o, "\troot * {}", qp(&site_root(s)));
            php_presets(o, s.framework.as_deref());
            match input.php_upstreams.get(&s.id) {
                Some(up) => {
                    // several upstreams = the Windows php-cgi pool (P1-13): spread
                    // requests and retry another instance while one restarts
                    let pool = up.contains(' ');
                    if s.env.is_empty() && !pool {
                        let _ = writeln!(o, "\tphp_fastcgi {up}");
                    } else {
                        let _ = writeln!(o, "\tphp_fastcgi {up} {{");
                        if pool {
                            let _ = writeln!(o, "\t\tlb_policy least_conn");
                            let _ = writeln!(o, "\t\tlb_try_duration 5s");
                        }
                        for (k, v) in &s.env {
                            let _ = writeln!(o, "\t\tenv {} {}", q(k), q(v));
                        }
                        let _ = writeln!(o, "\t}}");
                    }
                }
                None => {
                    let want = s.php.as_deref().unwrap_or("latest");
                    let _ = writeln!(
                        o,
                        "\trespond {} 502",
                        q(&format!(
                            "PHP {want} is not installed - run: bz runtime install php {want}"
                        ))
                    );
                }
            }
            let _ = writeln!(o, "\tfile_server");
        }
        SiteKind::Static => {
            let _ = writeln!(o, "\troot * {}", qp(&site_root(s)));
            let _ = writeln!(o, "\t@dotfiles path */.*");
            let _ = writeln!(o, "\trespond @dotfiles 404");
            let _ = writeln!(o, "\tfile_server");
        }
        SiteKind::Proxy => match s.proxy_port {
            Some(p) => {
                // `localhost`: Caddy dials every resolved address, so a dev server bound
                // to `::1` only (Vite/Node defaults) works as well as one on 127.0.0.1 (P1-16)
                let _ = writeln!(o, "\treverse_proxy localhost:{p}");
            }
            None => {
                let _ = writeln!(
                    o,
                    "\trespond \"No proxy port configured for this site\" 502"
                );
            }
        },
        SiteKind::App => match s.app_port {
            Some(p) => {
                // `localhost`: Caddy dials every resolved address, so a dev server bound
                // to `::1` only (Vite/Node defaults) works as well as one on 127.0.0.1 (P1-16)
                let _ = writeln!(o, "\treverse_proxy localhost:{p}");
            }
            None => {
                let _ = writeln!(
                    o,
                    "\trespond {} 502",
                    q(&format!("App is not running - run: bz site start {}", s.id))
                );
            }
        },
    }
    if let Some(snip) = input.snippets.get(&s.id) {
        let _ = writeln!(o, "\timport {}", qp(snip));
    }
}

/// `.htaccess`-equivalent rules for PHP frameworks.
fn php_presets(o: &mut String, framework: Option<&str>) {
    // every PHP app: never serve dotfiles (.env, .git, ...)
    let _ = writeln!(o, "\t@dotfiles {{");
    let _ = writeln!(o, "\t\tpath */.*");
    let _ = writeln!(o, "\t\tnot path /.well-known/*");
    let _ = writeln!(o, "\t}}");
    let _ = writeln!(o, "\trespond @dotfiles 404");
    match framework {
        Some("wordpress") => {
            // wp-config, debug log, PHP inside uploads and direct wp-includes/*.php
            let _ = writeln!(
                o,
                "\t@wp_forbidden path_regexp ^/(wp-config\\.php|wp-content/debug\\.log|wp-content/uploads/.*\\.php|wp-includes/[^/]+\\.php)$"
            );
            let _ = writeln!(o, "\trespond @wp_forbidden 403");
        }
        Some("symfony") => {
            // only the front controller may be executed directly
            let _ = writeln!(o, "\t@php_direct {{");
            let _ = writeln!(o, "\t\tpath *.php");
            let _ = writeln!(o, "\t\tnot path /index.php");
            let _ = writeln!(o, "\t}}");
            let _ = writeln!(o, "\trespond @php_direct 404");
        }
        Some("drupal") => {
            let _ = writeln!(
                o,
                "\t@drupal_private path_regexp (^/sites/[^/]+/files/private/|^/vendor/|\\.(engine|inc|install|module|profile|theme|twig|yml)$)"
            );
            let _ = writeln!(o, "\trespond @drupal_private 403");
        }
        _ => {}
    }
}

/// Push a Caddyfile to a running Caddy via its admin API (`POST /load`, caddyfile adapter).
pub async fn reload_via_admin(admin: &AdminEndpoint, caddyfile: &str) -> Result<()> {
    let base = admin.base_url();
    let url = format!("{base}/load");
    let mut req = admin
        .client()
        .post(&url)
        .header("Content-Type", "text/caddyfile");
    // Like Caddy's own CLI (`caddy reload`): an Origin only over TCP. Over the unix socket
    // Caddy 2.11 answers `Origin: http://127.0.0.1` with 403 "client is not allowed to access
    // from origin" (seen in the 1.0 live e2e), which forced the slower CLI fallback.
    if matches!(admin, AdminEndpoint::Tcp(_)) {
        req = req.header("Origin", base.clone());
    }
    let resp = req
        .body(caddyfile.to_string())
        .send()
        .await
        .map_err(|e| CoreError::Upstream(format!("caddy admin {url}: {e}")))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(CoreError::InvalidInput(format!(
            "caddy rejected the config ({status}): {body}"
        )));
    }
    Ok(())
}

/// Fallback reload with the CLI: `caddy reload --config <file> --adapter caddyfile --address <admin>`.
pub async fn reload_via_cli(caddy_bin: &Path, caddyfile: &Path, admin: &str) -> Result<()> {
    let out = tokio::process::Command::new(caddy_bin)
        .args([
            "reload",
            "--adapter",
            "caddyfile",
            "--address",
            admin,
            "--config",
        ])
        .arg(caddyfile)
        .output()
        .await?;
    if !out.status.success() {
        return Err(CoreError::InvalidInput(format!(
            "caddy reload failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn admin_endpoint_unix_socket() {
        use std::os::unix::fs::PermissionsExt;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let t = tempfile::tempdir().unwrap();
        let paths = crate::paths::Paths::new(t.path().join("h"));
        let a = AdminEndpoint::choose(&paths).unwrap();
        let AdminEndpoint::Unix(sock) = &a else {
            panic!("expected a unix socket, got {a:?}")
        };
        assert_eq!(a.address(), format!("unix/{}", sock.display()));
        assert!(a.address().starts_with("unix//"));
        assert_eq!(a.tcp_port(), None);
        std::fs::create_dir_all(sock.parent().unwrap()).unwrap();
        std::fs::write(sock, b"stale").unwrap();
        a.prepare().unwrap();
        assert!(!sock.exists());
        let mode = std::fs::metadata(sock.parent().unwrap())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);

        // fake Caddy admin API on the socket
        let l = tokio::net::UnixListener::bind(sock).unwrap();
        let srv = tokio::spawn(async move {
            loop {
                // the first connection is wait_ready's probe, which sends nothing
                let (mut s, _) = l.accept().await.unwrap();
                let mut buf = vec![0u8; 8192];
                let n = s.read(&mut buf).await.unwrap_or(0);
                if n == 0 {
                    continue;
                }
                s.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                    .await
                    .unwrap();
                return String::from_utf8_lossy(&buf[..n]).into_owned();
            }
        });
        assert!(a.wait_ready(std::time::Duration::from_secs(2)).await);
        reload_via_admin(&a, ":80 {\n}\n").await.unwrap();
        let req = srv.await.unwrap();
        assert!(req.starts_with("POST /load HTTP/1.1"), "{req}");
        assert!(
            req.to_ascii_lowercase().contains("host: 127.0.0.1"),
            "{req}"
        );
        // Caddy refuses unix-socket admin requests that carry an Origin (403)
        assert!(!req.to_ascii_lowercase().contains("\norigin:"), "{req}");
    }

    #[test]
    fn admin_endpoint_tcp() {
        let a = AdminEndpoint::Tcp(41234);
        assert_eq!(a.address(), "127.0.0.1:41234");
        assert_eq!(a.base_url(), "http://127.0.0.1:41234");
        assert_eq!(a.tcp_port(), Some(41234));
    }

    pub(crate) fn site(id: &str, kind: SiteKind) -> Site {
        Site {
            id: id.into(),
            domain: format!("{id}.test"),
            aliases: vec![],
            path: format!("/home/u/code/{id}"),
            kind,
            root: None,
            php: None,
            node: None,
            proxy_port: None,
            command: None,
            app_port: None,
            app_status: None,
            https: true,
            wildcard: false,
            framework: None,
            env: Default::default(),
            created_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    fn fixture_sites() -> Vec<Site> {
        let mut laravel = site("shop", SiteKind::Php);
        laravel.root = Some("public".into());
        laravel.php = Some("8.3".into());
        laravel.framework = Some("laravel".into());
        laravel.wildcard = true;
        laravel.aliases = vec!["admin.shop.test".into(), "shop-alt.test".into()];
        laravel.env.insert("APP_ENV".into(), "local".into());

        let mut wp = site("blog", SiteKind::Php);
        wp.php = Some("8.2".into());
        wp.framework = Some("wordpress".into());

        let mut sf = site("api", SiteKind::Php);
        sf.root = Some("public".into());
        sf.php = Some("8.4".into());
        sf.framework = Some("symfony".into());

        let mut dr = site("cms", SiteKind::Php);
        dr.root = Some("web".into());
        dr.framework = Some("drupal".into());

        let mut st = site("docs", SiteKind::Static);
        st.root = Some("dist".into());
        st.https = false;

        let mut px = site("grafana", SiteKind::Proxy);
        px.proxy_port = Some(3000);

        let mut app = site("next", SiteKind::App);
        app.framework = Some("nextjs".into());
        app.app_port = Some(41234);

        let mut stopped = site("vite", SiteKind::App);
        stopped.framework = Some("vite".into());
        vec![laravel, wp, sf, dr, st, px, app, stopped]
    }

    fn input(sites: &[Site], http: u16, https: u16) -> CaddyInput<'_> {
        let home = PathBuf::from("/home/u/.bozkurt");
        let mut certs = BTreeMap::new();
        for s in sites {
            for g in cert_groups(s) {
                if g.primary != "blog.test" {
                    certs.insert(
                        g.primary.clone(),
                        (
                            home.join(format!("certs/{}.pem", g.primary)),
                            home.join(format!("certs/{}-key.pem", g.primary)),
                        ),
                    );
                }
            }
        }
        let mut php = BTreeMap::new();
        php.insert(
            "shop".to_string(),
            // a literal Unix socket path: `Path::join` would insert `\` on Windows hosts
            "unix//home/u/.bozkurt/services/php-fpm-8.3/fpm.sock".to_string(),
        );
        php.insert("blog".to_string(), "127.0.0.1:9082".to_string());
        php.insert("api".to_string(), "127.0.0.1:9084".to_string());
        let mut snippets = BTreeMap::new();
        snippets.insert(
            "grafana".to_string(),
            home.join("caddy/snippets/grafana.test.caddy"),
        );
        CaddyInput {
            sites,
            http_port: http,
            https_port: https,
            admin: ADMIN_ADDR,
            logs_dir: home.join("logs"),
            certs,
            php_upstreams: php,
            snippets,
            lan: false,
        }
    }

    fn check_snapshot(name: &str, actual: &str) {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/snapshots")
            .join(name);
        if std::env::var_os("BZ_UPDATE_SNAPSHOTS").is_some() || !p.exists() {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, actual).unwrap();
            return;
        }
        let expected = std::fs::read_to_string(&p).unwrap();
        assert_eq!(
            expected, actual,
            "snapshot {name} differs; re-run with BZ_UPDATE_SNAPSHOTS=1 to accept"
        );
    }

    #[test]
    fn snapshot_full() {
        let sites = fixture_sites();
        check_snapshot("full.Caddyfile", &generate(&input(&sites, 80, 443)));
    }

    #[test]
    fn snapshot_unprivileged_ports() {
        let sites = fixture_sites();
        check_snapshot(
            "ports_8080_8443.Caddyfile",
            &generate(&input(&sites[..1], 8080, 8443)),
        );
    }

    #[test]
    fn content_checks() {
        let sites = fixture_sites();
        let out = generate(&input(&sites, 80, 443));
        assert!(out.contains("redir https://{host}{uri} 308"));
        assert!(out.contains("http://docs.test {"));
        assert!(out.contains("shop.test, *.shop.test, admin.shop.test {"));
        assert!(out.contains("php_fastcgi unix//home/u/.bozkurt/services/php-fpm-8.3/fpm.sock {"));
        assert!(out.contains("env \"APP_ENV\" \"local\""));
        assert!(out.contains("reverse_proxy localhost:41234"));
        assert!(out.contains("App is not running"));
        assert!(out.contains("PHP latest is not installed")); // cms has no upstream
                                                              // blog has no cert in the map → tls internal
        let blog_block = out.split("\nblog.test {\n").nth(1).unwrap();
        assert!(blog_block.starts_with("\ttls internal"));
    }

    #[test]
    fn windows_php_pool_is_load_balanced() {
        let sites = fixture_sites();
        let mut inp = input(&sites, 80, 443);
        inp.php_upstreams.insert(
            "blog".to_string(),
            "127.0.0.1:9082 127.0.0.1:21417".to_string(),
        );
        let out = generate(&inp);
        let blog = out.split("(bz-site-blog) {").nth(1).unwrap();
        assert!(blog.contains(
            "\tphp_fastcgi 127.0.0.1:9082 127.0.0.1:21417 {\n\t\tlb_policy least_conn\n\t\tlb_try_duration 5s\n"
        ));
    }

    #[test]
    fn empty() {
        let out = generate(&input(&[], 80, 443));
        assert!(out.contains("admin 127.0.0.1:2019"));
        assert!(!out.contains("redir https"));
    }

    #[test]
    fn groups() {
        let s = &fixture_sites()[0];
        let g = cert_groups(s);
        assert_eq!(g.len(), 2);
        assert_eq!(
            g[0].hosts,
            vec!["shop.test", "*.shop.test", "admin.shop.test"]
        );
        assert!(g[0].wildcard);
        assert_eq!(g[1].primary, "shop-alt.test");
    }

    #[test]
    fn quoting() {
        assert_eq!(q("plain \"x\""), r#""plain \"x\"""#);
        // backslashes survive in a raw backtick token (regexes, Windows values)
        assert_eq!(q(r#"C:\Users\a "b"\x"#), r#"`C:\Users\a "b"\x`"#);
        // backslash and backtick together: lossy fallback to `/`
        assert_eq!(q(r"a\`b"), r#""a/`b""#);
        // control characters never reach the output
        assert_eq!(q("a\n}\n:1 {"), r#""a}:1 {""#);
        assert_eq!(qp(Path::new(r"C:\Users\x")), r#""C:/Users/x""#);
        let mut s = site("x", SiteKind::Static);
        s.root = Some("/abs/root".into());
        assert_eq!(site_root(&s), PathBuf::from("/abs/root"));
        s.root = Some(".".into());
        assert_eq!(site_root(&s), PathBuf::from("/home/u/code/x"));
    }

    /// Validates the generated config with a real caddy binary when `BZ_CADDY` points to one.
    /// Top-level site blocks: unindented lines that open a block, minus snippets/global.
    fn top_level_blocks(text: &str) -> Vec<String> {
        text.lines()
            .filter(|l| !l.starts_with(['\t', ' ', '#', '(']) && l.trim_end().ends_with('{'))
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn field_injection_is_rejected_or_harmless() {
        const EVIL: &str = "\n}\n:1 {\n";
        let mut base = site("evil", SiteKind::Php);
        if cfg!(windows) {
            // `/home/...` is not absolute on Windows and validate() would reject the baseline
            base.path = "C:/home/u/code/evil".into();
        }
        base.root = Some("public".into());
        base.php = Some("8.3".into());
        base.framework = Some("laravel".into());
        base.env.insert("APP_ENV".into(), "local".into());
        crate::sites::validate(&base).unwrap();
        let baseline = top_level_blocks(&generate(&input(std::slice::from_ref(&base), 80, 443)));

        type Mutate = fn(&mut Site, &str);
        let cases: [(&str, Mutate, bool); 11] = [
            ("path", |s, e| s.path = format!("{}{e}", s.path), true),
            ("root", |s, e| s.root = Some(format!("public{e}")), true),
            ("php", |s, e| s.php = Some(format!("8.3{e}")), true),
            ("node", |s, e| s.node = Some(format!("20{e}")), true),
            (
                "framework",
                |s, e| s.framework = Some(format!("laravel{e}")),
                true,
            ),
            (
                "env key",
                |s, e| {
                    s.env.insert(format!("K{e}"), "v".into());
                },
                true,
            ),
            (
                "env value",
                |s, e| {
                    s.env.insert("K".into(), format!("v{e}"));
                },
                false,
            ),
            (
                "command",
                |s, e| s.command = Some(format!("serve{e}")),
                false,
            ),
            ("alias", |s, e| s.aliases.push(format!("a.test{e}")), true),
            ("domain", |s, e| s.domain = format!("evil.test{e}"), true),
            ("id", |s, e| s.id = format!("evil{e}"), true),
        ];
        for (field, mutate, must_reject) in cases {
            let mut s = base.clone();
            mutate(&mut s, EVIL);
            match crate::sites::validate(&s) {
                Err(_) => {}
                Ok(()) => {
                    assert!(!must_reject, "{field}: injection was not rejected");
                    let text = generate(&input(std::slice::from_ref(&s), 80, 443));
                    assert_eq!(
                        top_level_blocks(&text),
                        baseline,
                        "{field}: injection created a new top-level block:\n{text}"
                    );
                    assert!(
                        !text.lines().any(|l| l.trim() == ":1 {"),
                        "{field}: injected site address in output"
                    );
                }
            }
        }
        // `..` in the document root and relative project paths are refused too.
        let mut s = base.clone();
        s.root = Some("../../etc".into());
        assert!(crate::sites::validate(&s).is_err());
        let mut s = base.clone();
        s.path = "code/evil".into();
        assert!(crate::sites::validate(&s).is_err());
        let mut s = base.clone();
        s.env.insert("1BAD".into(), "x".into());
        assert!(crate::sites::validate(&s).is_err());
    }

    #[test]
    fn loopback_bind_unless_lan() {
        let sites = fixture_sites();
        let mut inp = input(&sites, 80, 443);
        assert!(generate(&inp).contains("\tdefault_bind 127.0.0.1 [::1]\n"));
        inp.lan = true;
        assert!(!generate(&inp).contains("default_bind"));
    }

    #[tokio::test]
    async fn validate_with_real_caddy_if_available() {
        let Some(caddy) = std::env::var_os("BZ_CADDY") else {
            return;
        };
        let t = tempfile::tempdir().unwrap();
        let sites = fixture_sites();
        let mut inp = input(&sites, 8080, 8443);
        inp.certs.clear(); // tls internal: no cert files needed for validation
        inp.snippets.clear();
        inp.logs_dir = t.path().to_path_buf();
        let f = t.path().join("Caddyfile");
        std::fs::write(&f, generate(&inp)).unwrap();
        let out = tokio::process::Command::new(caddy)
            .args(["validate", "--adapter", "caddyfile", "--config"])
            .arg(&f)
            .env("XDG_DATA_HOME", t.path())
            .env("XDG_CONFIG_HOME", t.path())
            .env("HOME", t.path())
            .output()
            .await
            .unwrap();
        assert!(
            out.status.success(),
            "caddy validate failed:\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
