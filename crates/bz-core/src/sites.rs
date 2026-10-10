//! Site records: creation (missing fields filled from `bozkurt.toml` and detection),
//! patching, validation, and the environment used to run `app` sites.
//!
//! Persistence and process control live in [`crate::engine::Engine`]; this module is
//! pure logic so it is easy to test.

use std::collections::BTreeMap;
use std::path::Path;

use crate::config::ProjectFile;
use crate::detection::{detect, slugify};
use crate::error::{CoreError, Result};
use crate::models::{CreateSiteRequest, Site, SiteKind, SitePatch};

/// Supervisor process name of an app site.
pub fn process_name(site_id: &str) -> String {
    format!("site-{site_id}")
}

/// Normalise a domain: lowercase, strip scheme/port/trailing dot, append `.<tld>` when
/// it does not already end with it (`shop` → `shop.test`, `Admin.Shop` → `admin.shop.test`).
pub fn normalize_domain(input: &str, tld: &str) -> Result<String> {
    let mut d = input.trim().to_ascii_lowercase();
    for p in ["https://", "http://"] {
        if let Some(r) = d.strip_prefix(p) {
            d = r.to_string();
        }
    }
    if let Some(i) = d.find(['/', ':']) {
        d.truncate(i);
    }
    let d = d.trim_end_matches('.').to_string();
    if d.is_empty() {
        return Err(CoreError::InvalidInput("domain must not be empty".into()));
    }
    let valid_label = |l: &str| {
        !l.is_empty()
            && l.len() <= 63
            && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            && !l.starts_with('-')
            && !l.ends_with('-')
    };
    if !d.split('.').all(valid_label) {
        return Err(CoreError::InvalidInput(format!("invalid domain `{input}`")));
    }
    let suffix = format!(".{tld}");
    Ok(if d == tld || d.ends_with(&suffix) {
        if d == tld {
            return Err(CoreError::InvalidInput(format!("`{d}` is the TLD itself")));
        }
        d
    } else {
        format!("{d}{suffix}")
    })
}

/// Site id derived from the domain (`admin.shop.test` → `admin-shop`), made unique
/// against `taken` by appending `-2`, `-3`, ...
pub fn make_id(domain: &str, tld: &str, taken: &[String]) -> String {
    let base_name = domain.strip_suffix(&format!(".{tld}")).unwrap_or(domain);
    let base = slugify(base_name);
    if !taken.contains(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|c| !taken.contains(c))
        .expect("infinite iterator")
}

/// Build a new [`Site`] from a request. Priority per field: request → `bozkurt.toml` →
/// detection → defaults (`https = true`, `wildcard = false`).
pub fn build_site(
    req: &CreateSiteRequest,
    tld: &str,
    existing: &[Site],
    now: &str,
) -> Result<Site> {
    let path = Path::new(&req.path);
    if !path.is_absolute() {
        return Err(CoreError::InvalidInput(format!(
            "site path must be absolute: {}",
            req.path
        )));
    }
    if !path.is_dir() {
        return Err(CoreError::NotFound(format!(
            "directory {} does not exist",
            req.path
        )));
    }
    let pf = ProjectFile::load_from_dir(path)?.unwrap_or_default();
    let det = detect(path, tld);

    let domain_src = req
        .domain
        .clone()
        .or(pf.domain.clone())
        .unwrap_or_else(|| det.suggested_domain.clone());
    let domain = normalize_domain(&domain_src, tld)?;
    let mut aliases = vec![];
    for a in req.aliases.clone().unwrap_or_else(|| pf.aliases.clone()) {
        let a = normalize_domain(&a, tld)?;
        if a != domain && !aliases.contains(&a) {
            aliases.push(a);
        }
    }
    check_unique(&domain, &aliases, existing, None)?;
    let kind = req.kind.or(pf.kind).unwrap_or(det.kind);
    let framework = det.framework.clone();
    let mut env: BTreeMap<String, String> = pf.env.clone();
    if let Some(e) = &req.env {
        env.extend(e.clone());
    }
    let ids: Vec<String> = existing.iter().map(|s| s.id.clone()).collect();
    let site = Site {
        id: make_id(&domain, tld, &ids),
        domain,
        aliases,
        path: req.path.clone(),
        kind,
        root: req.root.clone().or(pf.root.clone()).or(det.root.clone()),
        php: req.php.clone().or(pf.php.clone()).or(det.php.clone()),
        node: req.node.clone().or(pf.node.clone()).or(det.node.clone()),
        proxy_port: req.proxy_port,
        command: req
            .command
            .clone()
            .or(pf.command.clone())
            .or(det.command.clone()),
        app_port: None,
        app_status: None,
        https: req.https.unwrap_or(true),
        wildcard: req.wildcard.unwrap_or(false),
        framework,
        env,
        created_at: now.to_string(),
    };
    validate(&site)?;
    Ok(site)
}

/// Apply a patch (domain/aliases re-normalised and checked for uniqueness).
pub fn apply_patch(site: &Site, patch: &SitePatch, tld: &str, existing: &[Site]) -> Result<Site> {
    let mut s = site.clone();
    if let Some(d) = &patch.domain {
        s.domain = normalize_domain(d, tld)?;
    }
    if let Some(a) = &patch.aliases {
        let mut out = vec![];
        for x in a {
            let x = normalize_domain(x, tld)?;
            if x != s.domain && !out.contains(&x) {
                out.push(x);
            }
        }
        s.aliases = out;
    }
    if let Some(p) = &patch.path {
        if !Path::new(p).is_absolute() {
            return Err(CoreError::InvalidInput(format!(
                "site path must be absolute: {p}"
            )));
        }
        if !Path::new(p).is_dir() {
            return Err(CoreError::NotFound(format!("directory {p} does not exist")));
        }
        s.path = p.clone();
    }
    macro_rules! set {
        ($($f:ident),*) => { $( if let Some(v) = &patch.$f { s.$f = Some(v.clone()); } )* };
    }
    set!(root, php, node, command, framework);
    if let Some(v) = patch.proxy_port {
        s.proxy_port = Some(v);
    }
    if let Some(k) = patch.kind {
        s.kind = k;
    }
    if let Some(v) = patch.https {
        s.https = v;
    }
    if let Some(v) = patch.wildcard {
        s.wildcard = v;
    }
    if let Some(e) = &patch.env {
        s.env = e.clone();
    }
    // empty strings clear optional fields
    for f in [
        &mut s.root,
        &mut s.php,
        &mut s.node,
        &mut s.command,
        &mut s.framework,
    ] {
        if f.as_deref() == Some("") {
            *f = None;
        }
    }
    check_unique(&s.domain, &s.aliases, existing, Some(&s.id))?;
    validate(&s)?;
    Ok(s)
}

fn check_unique(
    domain: &str,
    aliases: &[String],
    existing: &[Site],
    skip: Option<&str>,
) -> Result<()> {
    for e in existing.iter().filter(|e| Some(e.id.as_str()) != skip) {
        let theirs: Vec<&String> = std::iter::once(&e.domain).chain(e.aliases.iter()).collect();
        for mine in std::iter::once(&domain.to_string()).chain(aliases.iter()) {
            if theirs.contains(&mine) {
                return Err(CoreError::Conflict(format!(
                    "{mine} is already used by site `{}`",
                    e.id
                )));
            }
        }
    }
    Ok(())
}

fn invalid(msg: String) -> CoreError {
    CoreError::InvalidInput(msg)
}

/// `^[a-z0-9-]{1,32}$`
fn is_framework_name(f: &str) -> bool {
    (1..=32).contains(&f.len())
        && f.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// `^[A-Za-z_][A-Za-z0-9_]*$`
fn is_env_key(k: &str) -> bool {
    let mut b = k.bytes();
    matches!(b.next(), Some(c) if c.is_ascii_alphabetic() || c == b'_')
        && b.all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

/// Field checks that keep site values from escaping their Caddyfile token (P0-2), plus
/// kind-specific required fields.
pub fn validate(s: &Site) -> Result<()> {
    if s.id.is_empty()
        || !s
            .id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(invalid(format!(
            "invalid site id `{}`",
            s.id.escape_debug()
        )));
    }
    for d in std::iter::once(&s.domain).chain(s.aliases.iter()) {
        if d.is_empty()
            || !d
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'*'))
        {
            return Err(invalid(format!("invalid domain `{}`", d.escape_debug())));
        }
    }
    if let Some(f) = &s.framework {
        if !is_framework_name(f) {
            return Err(invalid(format!(
                "invalid framework `{}` (expected [a-z0-9-]{{1,32}})",
                f.escape_debug()
            )));
        }
    }
    for k in s.env.keys() {
        if !is_env_key(k) {
            return Err(invalid(format!(
                "invalid environment variable name `{}`",
                k.escape_debug()
            )));
        }
    }
    for (name, v) in [
        ("path", Some(s.path.as_str())),
        ("root", s.root.as_deref()),
        ("php", s.php.as_deref()),
        ("node", s.node.as_deref()),
    ] {
        if let Some(v) = v {
            if v.chars().any(char::is_control) {
                return Err(invalid(format!(
                    "`{name}` must not contain control characters"
                )));
            }
        }
    }
    if !Path::new(&s.path).is_absolute() {
        return Err(invalid(format!("site path must be absolute: {}", s.path)));
    }
    if let Some(r) = &s.root {
        if r.split(['/', '\\']).any(|c| c == "..") {
            return Err(invalid(format!("document root must not contain `..`: {r}")));
        }
    }
    match s.kind {
        SiteKind::Proxy if s.proxy_port.is_none() => Err(CoreError::InvalidInput(
            "proxy sites need `proxy_port`".into(),
        )),
        SiteKind::App if s.command.as_deref().is_none_or(|c| c.trim().is_empty()) => {
            Err(CoreError::InvalidInput("app sites need a `command`".into()))
        }
        _ => Ok(()),
    }
}

/// Substitute `$PORT` / `${PORT}` / `%PORT%` in a dev command.
pub fn expand_command(cmd: &str, port: u16) -> String {
    let p = port.to_string();
    cmd.replace("${PORT}", &p)
        .replace("$PORT", &p)
        .replace("%PORT%", &p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(path: &Path) -> CreateSiteRequest {
        CreateSiteRequest {
            path: path.to_string_lossy().into_owned(),
            ..Default::default()
        }
    }

    #[test]
    fn domains() {
        assert_eq!(normalize_domain("shop", "test").unwrap(), "shop.test");
        assert_eq!(normalize_domain("Shop.TEST", "test").unwrap(), "shop.test");
        assert_eq!(
            normalize_domain("https://admin.shop:8443/x", "test").unwrap(),
            "admin.shop.test"
        );
        assert!(normalize_domain("", "test").is_err());
        assert!(normalize_domain("bad_name", "test").is_err());
        assert!(normalize_domain("test", "test").is_err());
        assert_eq!(make_id("admin.shop.test", "test", &[]), "admin-shop");
        assert_eq!(
            make_id("shop.test", "test", &["shop".into(), "shop-2".into()]),
            "shop-3"
        );
        assert_eq!(
            expand_command("vite --port $PORT", 4100),
            "vite --port 4100"
        );
        assert_eq!(expand_command("x ${PORT} %PORT%", 1), "x 1 1");
    }

    #[test]
    fn create_from_detection_and_project_file() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("My Shop");
        std::fs::create_dir_all(dir.join("public")).unwrap();
        std::fs::write(dir.join("artisan"), "").unwrap();
        std::fs::write(dir.join("public/index.php"), "").unwrap();
        std::fs::write(
            dir.join("composer.json"),
            r#"{"require":{"php":"^8.2","laravel/framework":"^11"}}"#,
        )
        .unwrap();
        let s = build_site(&req(&dir), "test", &[], "now").unwrap();
        assert_eq!(s.domain, "my-shop.test");
        assert_eq!(s.id, "my-shop");
        assert_eq!(s.kind, SiteKind::Php);
        assert_eq!(s.root.as_deref(), Some("public"));
        assert_eq!(s.php.as_deref(), Some("8.2"));
        assert_eq!(s.framework.as_deref(), Some("laravel"));
        assert!(s.https && !s.wildcard);

        std::fs::write(
            dir.join("bozkurt.toml"),
            "domain = \"shop\"\nphp = \"8.4\"\naliases = [\"admin.shop\"]\n[env]\nAPP_ENV = \"local\"\n",
        )
        .unwrap();
        let mut r = req(&dir);
        r.wildcard = Some(true);
        let s2 = build_site(&r, "test", &[], "now").unwrap();
        assert_eq!(s2.domain, "shop.test");
        assert_eq!(s2.php.as_deref(), Some("8.4"));
        assert_eq!(s2.aliases, vec!["admin.shop.test"]);
        assert_eq!(s2.env["APP_ENV"], "local");
        assert!(s2.wildcard);
        // duplicate domain conflicts
        assert!(matches!(
            build_site(&r, "test", std::slice::from_ref(&s2), "now"),
            Err(CoreError::Conflict(_))
        ));
        // request beats project file
        let mut r3 = req(&dir);
        r3.domain = Some("other".into());
        r3.php = Some("8.3".into());
        r3.aliases = Some(vec![]);
        let s3 = build_site(&r3, "test", &[s2], "now").unwrap();
        assert_eq!(
            (s3.domain.as_str(), s3.php.as_deref()),
            ("other.test", Some("8.3"))
        );
    }

    #[test]
    fn validation_and_patch() {
        let t = tempfile::tempdir().unwrap();
        let mut r = req(t.path());
        r.kind = Some(SiteKind::Proxy);
        assert!(build_site(&r, "test", &[], "now").is_err());
        r.proxy_port = Some(3000);
        let s = build_site(&r, "test", &[], "now").unwrap();
        let p = SitePatch {
            domain: Some("grafana".into()),
            https: Some(false),
            ..Default::default()
        };
        let s2 = apply_patch(&s, &p, "test", std::slice::from_ref(&s)).unwrap();
        assert_eq!(s2.domain, "grafana.test");
        assert!(!s2.https);
        let bad = SitePatch {
            kind: Some(SiteKind::App),
            ..Default::default()
        };
        assert!(apply_patch(&s, &bad, "test", &[]).is_err());
        let clear = SitePatch {
            root: Some(String::new()),
            ..Default::default()
        };
        assert_eq!(apply_patch(&s, &clear, "test", &[]).unwrap().root, None);
        assert!(build_site(&req(Path::new("relative")), "test", &[], "now").is_err());
    }
}
