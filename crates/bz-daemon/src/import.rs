//! `GET /import/scan` + `POST /import`: find sites and DNS routing of other local dev
//! tools (Valet, Herd, MAMP, XAMPP, Laragon, Homebrew Apache, `/etc/resolver`) —
//! strictly read-only — and import the selected vhosts as BOZKURT sites.
//!
//! Importing never modifies the other tool's files; it only creates sites. DNS entries
//! are never imported or changed (P1-20): `POST /import` with only `dns` entries is
//! rejected (`400 invalid_input`), and with vhosts the job message lists each DNS entry as
//! left unchanged, flagging the ones that route BOZKURT's own TLD elsewhere
//! ([`dns_notes`]) so the user can remove the old resolver themselves.

use std::path::{Path, PathBuf};

use bz_core::models::CreateSiteRequest;
use bz_core::{Engine, Result};
use serde::{Deserialize, Serialize};

/// A virtual host found in another tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportVhost {
    pub source: String,
    pub domain: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub php: Option<String>,
}

/// DNS routing found in another tool / the OS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportDns {
    pub source: String,
    pub tld: String,
    pub file: String,
}

/// `GET /import/scan` response, also the `POST /import` body.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportScan {
    #[serde(default)]
    pub vhosts: Vec<ImportVhost>,
    #[serde(default)]
    pub dns: Vec<ImportDns>,
}

/// Where to look (overridable for tests).
#[derive(Debug, Clone)]
pub struct ScanRoots {
    /// User home directory.
    pub home: PathBuf,
    /// Filesystem root prefix ("/" normally; a temp dir in tests). Applied to absolute
    /// system paths such as `/Applications/MAMP/...` and `/etc/resolver`.
    pub root: PathBuf,
}

impl ScanRoots {
    pub fn system() -> ScanRoots {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_default();
        ScanRoots {
            home,
            root: PathBuf::from("/"),
        }
    }
    fn sys(&self, p: &str) -> PathBuf {
        if self.root == Path::new("/") {
            PathBuf::from(p)
        } else {
            self.root.join(p.trim_start_matches('/').replace(':', ""))
        }
    }
}

/// Parse an Apache config for `<VirtualHost>` blocks → (ServerName, DocumentRoot) pairs
/// (ServerAlias entries are ignored; `localhost`/`_default_` hosts are skipped).
pub fn parse_apache_vhosts(conf: &str) -> Vec<(String, String)> {
    let mut out = vec![];
    let mut in_vhost = false;
    let mut name: Option<String> = None;
    let mut root: Option<String> = None;
    for raw in conf.lines() {
        let line = raw.trim();
        if line.starts_with('#') {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("<virtualhost") {
            in_vhost = true;
            name = None;
            root = None;
        } else if lower.starts_with("</virtualhost") {
            if let (Some(n), Some(r)) = (name.take(), root.take()) {
                let n = n.to_ascii_lowercase();
                if n != "localhost" && !n.starts_with('_') && !n.is_empty() {
                    out.push((n, r));
                }
            }
            in_vhost = false;
        } else if in_vhost {
            let mut parts = line.splitn(2, char::is_whitespace);
            let key = parts.next().unwrap_or("").to_ascii_lowercase();
            let val = parts
                .next()
                .unwrap_or("")
                .trim()
                .trim_matches('"')
                .to_string();
            match key.as_str() {
                "servername" => {
                    // strip :port
                    let v = val.split(':').next().unwrap_or("").to_string();
                    name = Some(v);
                }
                "documentroot" => root = Some(val.trim_end_matches(['/', '\\']).to_string()),
                _ => {}
            }
        }
    }
    out
}

/// A site's project dir from its web root (`…/public` → parent).
fn project_dir(doc_root: &str) -> String {
    let p = Path::new(doc_root);
    match p.file_name().and_then(|n| n.to_str()) {
        Some("public") | Some("web") | Some("public_html") => p
            .parent()
            .map(|x| x.to_string_lossy().into_owned())
            .unwrap_or_else(|| doc_root.to_string()),
        _ => doc_root.to_string(),
    }
}

fn read(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok()
}

fn apache_sources(r: &ScanRoots) -> Vec<(&'static str, PathBuf)> {
    let mut v = vec![
        (
            "mamp",
            r.sys("/Applications/MAMP/conf/apache/extra/httpd-vhosts.conf"),
        ),
        (
            "mamp",
            r.sys("/Applications/MAMP PRO/conf/apache/extra/httpd-vhosts.conf"),
        ),
        (
            "xampp",
            r.sys("/Applications/XAMPP/xamppfiles/etc/extra/httpd-vhosts.conf"),
        ),
        ("xampp", r.sys("/opt/lampp/etc/extra/httpd-vhosts.conf")),
        (
            "xampp",
            r.sys("C:/xampp/apache/conf/extra/httpd-vhosts.conf"),
        ),
        (
            "homebrew-apache",
            r.sys("/opt/homebrew/etc/httpd/extra/httpd-vhosts.conf"),
        ),
        (
            "homebrew-apache",
            r.sys("/usr/local/etc/httpd/extra/httpd-vhosts.conf"),
        ),
    ];
    // Laragon: one file per site
    for dir in [
        r.sys("C:/laragon/etc/apache2/sites-enabled"),
        r.home.join("laragon/etc/apache2/sites-enabled"),
    ] {
        if let Ok(rd) = std::fs::read_dir(&dir) {
            let mut files: Vec<PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "conf"))
                .collect();
            files.sort();
            v.extend(files.into_iter().map(|f| ("laragon", f)));
        }
    }
    v
}

/// Valet-style `config.json` + `Sites/` links: (tld, vhosts).
fn scan_valet(source: &str, dir: &Path, out: &mut ImportScan) {
    let Some(cfg) = read(&dir.join("config.json")) else {
        return;
    };
    let v: serde_json::Value = serde_json::from_str(&cfg).unwrap_or_default();
    let tld = v
        .get("tld")
        .or_else(|| v.get("domain"))
        .and_then(|t| t.as_str())
        .unwrap_or("test")
        .to_string();
    let mut add = |name: &str, path: PathBuf| {
        let domain = format!("{}.{tld}", name.to_ascii_lowercase());
        if out
            .vhosts
            .iter()
            .any(|x| x.domain == domain && x.source == source)
        {
            return;
        }
        let php = read(&path.join(".valetrc"))
            .or_else(|| read(&path.join(".valetphprc")))
            .and_then(|s| {
                s.lines()
                    .find_map(|l| l.trim().strip_prefix("php=").map(str::to_string))
                    .or_else(|| Some(s.trim().to_string()))
            })
            .map(|s| s.trim_start_matches("php@").to_string())
            .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit() || c == '.'));
        out.vhosts.push(ImportVhost {
            source: source.into(),
            domain,
            path: path.to_string_lossy().into_owned(),
            php,
        });
    };
    // linked sites
    if let Ok(rd) = std::fs::read_dir(dir.join("Sites")) {
        let mut ents: Vec<_> = rd.flatten().collect();
        ents.sort_by_key(|e| e.file_name());
        for e in ents {
            let target = std::fs::read_link(e.path()).unwrap_or_else(|_| e.path());
            if target.is_dir() {
                add(&e.file_name().to_string_lossy(), target);
            }
        }
    }
    // parked directories: every sub-directory is a site
    if let Some(paths) = v.get("paths").and_then(|p| p.as_array()) {
        for p in paths.iter().filter_map(|p| p.as_str()) {
            if p.ends_with("/Sites") && Path::new(p).starts_with(dir) {
                continue; // the links dir itself
            }
            if let Ok(rd) = std::fs::read_dir(p) {
                let mut ents: Vec<_> = rd.flatten().filter(|e| e.path().is_dir()).collect();
                ents.sort_by_key(|e| e.file_name());
                for e in ents {
                    let n = e.file_name().to_string_lossy().into_owned();
                    if !n.starts_with('.') {
                        add(&n, e.path());
                    }
                }
            }
        }
    }
    out.dns.push(ImportDns {
        source: source.into(),
        tld,
        file: dir.join("config.json").to_string_lossy().into_owned(),
    });
}

/// Read-only scan.
pub fn scan(r: &ScanRoots) -> ImportScan {
    let mut out = ImportScan::default();
    for (source, dir) in [
        ("valet", r.home.join(".config/valet")),
        ("valet", r.home.join(".valet")),
        (
            "herd",
            r.home.join("Library/Application Support/Herd/config/valet"),
        ),
        ("herd", r.home.join(".config/herd/config/valet")),
    ] {
        scan_valet(source, &dir, &mut out);
    }
    for (source, file) in apache_sources(r) {
        let Some(conf) = read(&file) else { continue };
        for (domain, root) in parse_apache_vhosts(&conf) {
            if out.vhosts.iter().any(|v| v.domain == domain) {
                continue;
            }
            out.vhosts.push(ImportVhost {
                source: source.into(),
                domain,
                path: project_dir(&root),
                php: None,
            });
        }
    }
    // macOS per-TLD resolvers
    if let Ok(rd) = std::fs::read_dir(r.sys("/etc/resolver")) {
        let mut ents: Vec<_> = rd.flatten().collect();
        ents.sort_by_key(|e| e.file_name());
        for e in ents {
            let tld = e.file_name().to_string_lossy().into_owned();
            let file = e.path();
            if out.dns.iter().any(|d| d.file == file.to_string_lossy()) {
                continue;
            }
            let content = read(&file).unwrap_or_default().to_ascii_lowercase();
            let source = if content.contains("valet") {
                "valet"
            } else if content.contains("herd") {
                "herd"
            } else if content.contains("bozkurt") {
                continue;
            } else {
                "resolver"
            };
            out.dns.push(ImportDns {
                source: source.into(),
                tld,
                file: file.to_string_lossy().into_owned(),
            });
        }
    }
    out
}

/// Domain for BOZKURT: keep the name, drop a foreign TLD (`shop.local` → `shop`).
pub fn import_domain(domain: &str, tld: &str) -> String {
    let d = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    if d.ends_with(&format!(".{tld}")) {
        return d;
    }
    match d.rsplit_once('.') {
        Some((base, _)) if !base.is_empty() => base.to_string(),
        _ => d,
    }
}

/// What happens to the `dns` entries of a `POST /import` body: nothing — they are never
/// imported (P1-20). One human-readable line per entry; an entry for our own `tld` is a
/// conflict (it may route `*.tld` to the other tool instead of BOZKURT).
pub fn dns_notes(dns: &[ImportDns], tld: &str) -> Vec<String> {
    dns.iter()
        .map(|d| {
            let t = d.tld.trim_start_matches('.').to_ascii_lowercase();
            if t == tld {
                format!(
                    "{} ({}) also routes .{t} and may send it away from BOZKURT: remove {} yourself",
                    d.source, d.file, d.file
                )
            } else {
                format!("{} DNS for .{t} ({}) left unchanged", d.source, d.file)
            }
        })
        .collect()
}

/// Reject a body that asks only for DNS entries: there is nothing to import (P1-20).
pub fn validate(req: &ImportScan) -> Result<()> {
    if req.vhosts.is_empty() && !req.dns.is_empty() {
        return Err(bz_core::CoreError::InvalidInput(
            "importing DNS settings is not supported: BOZKURT never changes another tool's resolver; select at least one vhost, or remove the old resolver file yourself".into(),
        ));
    }
    Ok(())
}

/// Create one site per vhost. Returns (created domains, errors).
pub async fn import(
    e: &Engine,
    req: &ImportScan,
    mut progress: impl FnMut(f64, &str),
) -> Result<(Vec<String>, Vec<String>)> {
    let tld = e.config().tld;
    let mut ok = vec![];
    let mut errs = vec![];
    let total = req.vhosts.len().max(1) as f64;
    for (i, v) in req.vhosts.iter().enumerate() {
        progress(i as f64 / total, &format!("Importing {}", v.domain));
        let r = CreateSiteRequest {
            path: v.path.clone(),
            domain: Some(import_domain(&v.domain, &tld)),
            php: v.php.clone(),
            ..Default::default()
        };
        match e.create_site(&r).await {
            Ok(s) => ok.push(s.domain),
            Err(err) => errs.push(format!("{}: {err}", v.domain)),
        }
    }
    Ok((ok, errs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dns_entries_are_reported_not_imported() {
        let dns = vec![
            ImportDns {
                source: "valet".into(),
                tld: "test".into(),
                file: "/etc/resolver/test".into(),
            },
            ImportDns {
                source: "resolver".into(),
                tld: "local".into(),
                file: "/etc/resolver/local".into(),
            },
        ];
        let n = dns_notes(&dns, "test");
        assert!(n[0].contains("routes .test") && n[0].contains("remove /etc/resolver/test"));
        assert!(n[1].contains(".local") && n[1].contains("left unchanged"));
        let only_dns = ImportScan {
            vhosts: vec![],
            dns: dns.clone(),
        };
        assert!(matches!(
            validate(&only_dns),
            Err(bz_core::CoreError::InvalidInput(_))
        ));
        assert!(validate(&ImportScan::default()).is_ok());
    }

    #[test]
    fn apache_parse() {
        let conf = r#"
# <VirtualHost *:80> commented
<VirtualHost *:80>
    ServerName localhost
    DocumentRoot "/Applications/MAMP/htdocs"
</VirtualHost>
<VirtualHost *:80>
    ServerName client-a.local:80
    ServerAlias www.client-a.local
    DocumentRoot "/Users/c/Sites/client-a/public/"
</VirtualHost>
<virtualhost *:443>
  servername Blog.Test
  documentroot /srv/blog
</virtualhost>
"#;
        let v = parse_apache_vhosts(conf);
        assert_eq!(
            v,
            vec![
                (
                    "client-a.local".into(),
                    "/Users/c/Sites/client-a/public".into()
                ),
                ("blog.test".into(), "/srv/blog".into())
            ]
        );
        assert_eq!(
            project_dir("/Users/c/Sites/client-a/public"),
            "/Users/c/Sites/client-a"
        );
        assert_eq!(import_domain("client-a.local", "test"), "client-a");
        assert_eq!(import_domain("blog.test", "test"), "blog.test");
    }

    #[test]
    fn scan_fake_tree() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().join("home");
        let root = t.path().join("root");
        let proj = t.path().join("code/shop");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join(".valetrc"), "php=php@8.2\n").unwrap();
        let valet = home.join(".config/valet");
        std::fs::create_dir_all(valet.join("Sites")).unwrap();
        std::fs::write(
            valet.join("config.json"),
            serde_json::json!({
                "tld": "test",
                "paths": [valet.join("Sites"), t.path().join("code")],
            })
            .to_string(),
        )
        .unwrap();
        let mamp = root.join("Applications/MAMP/conf/apache/extra");
        std::fs::create_dir_all(&mamp).unwrap();
        std::fs::write(
            mamp.join("httpd-vhosts.conf"),
            "<VirtualHost *:80>\nServerName legacy.local\nDocumentRoot /x/legacy\n</VirtualHost>\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("etc/resolver")).unwrap();
        std::fs::write(
            root.join("etc/resolver/test"),
            "# Generated by Valet\nnameserver 127.0.0.1\n",
        )
        .unwrap();
        let s = scan(&ScanRoots { home, root });
        assert_eq!(s.vhosts.len(), 2, "{s:?}");
        assert_eq!(s.vhosts[0].domain, "shop.test");
        assert_eq!(s.vhosts[0].php.as_deref(), Some("8.2"));
        assert_eq!(s.vhosts[1].source, "mamp");
        assert!(s.dns.iter().any(|d| d.source == "valet"
            && d.tld == "test"
            && d.file.replace('\\', "/").ends_with("resolver/test")));
    }
}
