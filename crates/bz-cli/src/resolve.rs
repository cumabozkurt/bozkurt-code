//! Shared argument resolution: sites, service names, runtime versions.

use std::path::{Path, PathBuf};

use bz_core::models::{Channel, Site, Version};

use crate::client::Client;
use crate::error::CliError;

pub fn not_found(msg: impl Into<String>) -> CliError {
    CliError::Api {
        status: 404,
        code: "not_found".into(),
        message: msg.into(),
    }
}

/// Canonical service name: `mysql`→`mariadb`, `postgresql`→`postgres`, `valkey`→`redis`.
pub fn service_name(s: &str) -> String {
    match s.to_ascii_lowercase().as_str() {
        "mysql" => "mariadb".into(),
        "postgresql" | "pg" => "postgres".into(),
        "valkey" => "redis".into(),
        other => other.to_string(),
    }
}

/// Is `s` a service name (including aliases)?
pub fn is_service_name(s: &str) -> bool {
    let n = service_name(s);
    matches!(
        n.as_str(),
        "caddy" | "dns" | "mariadb" | "postgres" | "redis" | "mailpit"
    ) || n.starts_with("php-fpm-")
}

fn canon(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Pick a site from `sites` by id, domain, alias or bare name.
pub fn find_site<'a>(sites: &'a [Site], arg: &str) -> Option<&'a Site> {
    let a = arg.trim().to_ascii_lowercase();
    let a = a
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/');
    sites
        .iter()
        .find(|s| s.id == arg)
        .or_else(|| sites.iter().find(|s| s.domain.eq_ignore_ascii_case(a)))
        .or_else(|| {
            sites.iter().find(|s| {
                s.domain
                    .rsplit_once('.')
                    .map(|(n, _)| n.eq_ignore_ascii_case(a))
                    .unwrap_or(false)
            })
        })
        .or_else(|| {
            sites
                .iter()
                .find(|s| s.aliases.iter().any(|al| al.eq_ignore_ascii_case(a)))
        })
}

/// The site whose `path` contains `dir` (deepest match).
pub fn site_for_dir<'a>(sites: &'a [Site], dir: &Path) -> Option<&'a Site> {
    let dir = canon(dir);
    sites
        .iter()
        .filter(|s| dir.starts_with(canon(Path::new(&s.path))))
        .max_by_key(|s| s.path.len())
}

/// Resolve the `<site>` argument (id, domain, bare name, or the current directory).
pub fn resolve_site(client: &Client, arg: Option<&str>) -> Result<Site, CliError> {
    let sites = client.sites()?;
    match arg {
        Some(a) => find_site(&sites, a)
            .cloned()
            .ok_or_else(|| not_found(format!("no site `{a}`"))),
        None => {
            let cwd = std::env::current_dir()?;
            site_for_dir(&sites, &cwd)
                .cloned()
                .ok_or_else(|| not_found(crate::i18n::t("no_site_here")))
        }
    }
}

/// Resolve `want` (`8.3`, `22.11.0`, `lts`, `latest`) against versions where `pred` holds.
pub fn pick_version(
    versions: &[Version],
    want: &str,
    pred: impl Fn(&Version) -> bool,
) -> Option<String> {
    let cands: Vec<&Version> = versions.iter().filter(|v| pred(v)).collect();
    let w = want.trim().to_ascii_lowercase();
    let pool: Vec<String> = match w.as_str() {
        "lts" => cands
            .iter()
            .filter(|v| v.channel == Channel::Lts)
            .map(|v| v.version.clone())
            .collect(),
        "latest" | "current" | "stable" => cands
            .iter()
            .filter(|v| v.channel != Channel::Eol)
            .map(|v| v.version.clone())
            .collect(),
        _ => cands.iter().map(|v| v.version.clone()).collect(),
    };
    let key = if matches!(w.as_str(), "lts" | "latest" | "current" | "stable") {
        let mut all = pool;
        bz_core::version::sort_desc(&mut all);
        return all.into_iter().next();
    } else {
        w
    };
    bz_core::version::best_match(pool.iter(), &key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn site(id: &str, domain: &str, path: &str) -> Site {
        Site {
            id: id.into(),
            domain: domain.into(),
            aliases: vec![format!("www.{domain}")],
            path: path.into(),
            kind: bz_core::models::SiteKind::Php,
            root: None,
            php: None,
            node: None,
            proxy_port: None,
            command: None,
            app_port: None,
            app_status: None,
            https: false,
            wildcard: false,
            framework: None,
            env: BTreeMap::new(),
            created_at: String::new(),
        }
    }

    #[test]
    fn site_lookup() {
        let s = vec![
            site("7f3c", "shop.test", "/p/shop"),
            site("a1", "blog.test", "/p/blog"),
        ];
        assert_eq!(find_site(&s, "7f3c").unwrap().id, "7f3c");
        assert_eq!(find_site(&s, "blog.test").unwrap().id, "a1");
        assert_eq!(find_site(&s, "shop").unwrap().id, "7f3c");
        assert_eq!(find_site(&s, "https://SHOP.test/").unwrap().id, "7f3c");
        assert_eq!(find_site(&s, "www.blog.test").unwrap().id, "a1");
        assert!(find_site(&s, "nope").is_none());
        assert_eq!(
            site_for_dir(&s, Path::new("/p/shop/app/Http")).unwrap().id,
            "7f3c"
        );
        assert!(site_for_dir(&s, Path::new("/p/shopping")).is_none());
    }

    #[test]
    fn service_aliases() {
        assert_eq!(service_name("MySQL"), "mariadb");
        assert_eq!(service_name("valkey"), "redis");
        assert!(is_service_name("php-fpm-8.3") && is_service_name("postgresql"));
        assert!(!is_service_name("shop"));
    }

    #[test]
    fn version_picking() {
        let v = |ver: &str, ch, inst| Version {
            version: ver.into(),
            channel: ch,
            installed: inst,
            active: false,
            released: None,
            size_bytes: None,
        };
        let vs = vec![
            v("22.11.0", Channel::Lts, true),
            v("23.1.0", Channel::Current, false),
            v("20.18.0", Channel::Lts, true),
            v("8.30.0", Channel::Stable, true),
        ];
        assert_eq!(
            pick_version(&vs, "lts", |_| true).as_deref(),
            Some("22.11.0")
        );
        assert_eq!(
            pick_version(&vs, "latest", |_| true).as_deref(),
            Some("23.1.0")
        );
        assert_eq!(
            pick_version(&vs, "latest", |v| v.installed).as_deref(),
            Some("22.11.0")
        );
        assert_eq!(
            pick_version(&vs, "20", |v| v.installed).as_deref(),
            Some("20.18.0")
        );
        assert_eq!(pick_version(&vs, "8.3", |_| true), None);
    }
}
