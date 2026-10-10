//! Local certificate authority and per-site leaf certificates (rcgen, no external mkcert).
//!
//! * CA: `ca/rootCA.pem` + `ca/rootCA-key.pem` (0600), ECDSA P-256, 10 years,
//!   CN `BOZKURT CODE Local CA <user>@<host>`, path length 0.
//! * Leaf: `certs/<domain>.pem` + `certs/<domain>-key.pem` (0600), SAN = domain + aliases
//!   (+ `*.domain` / `*.alias` when wildcard), serverAuth, 825 days total validity (Apple's limit),
//!   re-issued when < 30 days remain, when the SAN set changes or when the CA changed.
//! * Name constraints (P1-4): the CA may only sign names under its permitted DNS subtrees
//!   ([`DEFAULT_PERMITTED`] = `test`, `localhost`, plus every other TLD a site uses), so a
//!   stolen `rootCA-key.pem` cannot mint trusted certificates for real domains. Issuing a
//!   leaf under a TLD the CA does not permit regenerates the CA with that TLD added (the
//!   new CA must be trusted again, `bz tls trust`). A CA created before constraints existed
//!   is kept as is.

use chrono::{DateTime, Utc};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose,
    GeneralSubtree, IsCa, Issuer, KeyPair, KeyUsagePurpose, NameConstraints,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use time::{Duration as TDuration, OffsetDateTime};

use crate::error::{IoContext, NetError, Result};

/// CA certificate file name inside the CA dir.
pub const CA_CERT_FILE: &str = "rootCA.pem";
/// CA private key file name inside the CA dir.
pub const CA_KEY_FILE: &str = "rootCA-key.pem";
/// CA validity in days (10 years).
pub const CA_VALIDITY_DAYS: i64 = 3650;
/// Maximum total leaf validity accepted by Apple platforms.
pub const LEAF_VALIDITY_DAYS: i64 = 825;
/// Re-issue a leaf when fewer than this many days remain.
pub const RENEW_BEFORE_DAYS: i64 = 30;
/// Organisation name on our certificates.
pub const ORG: &str = "BOZKURT CODE";
/// DNS subtrees a new CA is constrained to (P1-4).
pub const DEFAULT_PERMITTED: &[&str] = &["test", "localhost"];

/// Permitted DNS subtrees of a CA certificate; `None` when it has no name constraints.
pub fn permitted_dns_names(pem: &str) -> Result<Option<Vec<String>>> {
    let (_, p) = x509_parser::pem::parse_x509_pem(pem.as_bytes())
        .map_err(|e| NetError::Cert(format!("bad PEM: {e}")))?;
    let x = p
        .parse_x509()
        .map_err(|e| NetError::Cert(format!("bad certificate: {e}")))?;
    let Ok(Some(nc)) = x.name_constraints() else {
        return Ok(None);
    };
    let names = nc
        .value
        .permitted_subtrees
        .as_ref()
        .map(|subs| {
            subs.iter()
                .filter_map(|s| match &s.base {
                    x509_parser::extensions::GeneralName::DNSName(n) => {
                        Some(n.trim_start_matches('.').to_ascii_lowercase())
                    }
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Some(names))
}

/// Is `name` (possibly `*.x`) inside one of the DNS subtrees `permitted`?
pub fn name_permitted(name: &str, permitted: &[String]) -> bool {
    let n = name.trim_start_matches("*.").to_ascii_lowercase();
    permitted
        .iter()
        .any(|p| n == *p || n.ends_with(&format!(".{p}")))
}

/// The top-level label of a domain (`shop.test` → `test`).
fn tld_of(name: &str) -> String {
    name.trim_end_matches('.')
        .rsplit('.')
        .next()
        .unwrap_or(name)
        .to_ascii_lowercase()
}

/// Facts about the local CA.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaInfo {
    /// `rootCA.pem` path.
    pub cert_path: PathBuf,
    /// `rootCA-key.pem` path.
    pub key_path: PathBuf,
    /// Common name.
    pub common_name: String,
    /// Expiry.
    pub not_after: DateTime<Utc>,
    /// Serial number, lowercase hex.
    pub serial_hex: String,
    /// SHA-256 fingerprint of the DER, lowercase hex.
    pub sha256: String,
}

/// Facts about a certificate parsed from PEM.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CertInfo {
    /// Subject DN (RFC 4514-ish string).
    pub subject: String,
    /// Issuer DN.
    pub issuer: String,
    /// DNS subject alternative names.
    pub dns_names: Vec<String>,
    /// Validity start.
    pub not_before: DateTime<Utc>,
    /// Validity end.
    pub not_after: DateTime<Utc>,
    /// Serial number, lowercase hex.
    pub serial_hex: String,
    /// SHA-256 fingerprint, lowercase hex.
    pub sha256: String,
    /// Whether it is a CA certificate.
    pub is_ca: bool,
    #[serde(skip)]
    issuer_raw: Vec<u8>,
    #[serde(skip)]
    subject_raw: Vec<u8>,
}

impl CertInfo {
    /// Days of validity left at `now` (negative when expired).
    pub fn days_left(&self, now: DateTime<Utc>) -> i64 {
        (self.not_after - now).num_days()
    }
}

/// CA status for the API (`GET /tls` → `{ca_path, ca_trusted, ca_expires}`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaStatus {
    /// Path of `rootCA.pem`.
    pub ca_path: PathBuf,
    /// The CA exists on disk.
    pub exists: bool,
    /// Trusted by the primary store for this OS (see [`crate::trust`]).
    pub ca_trusted: bool,
    /// CA expiry.
    pub ca_expires: Option<DateTime<Utc>>,
    /// CA common name.
    pub common_name: Option<String>,
    /// Per-store details.
    pub stores: Vec<TrustStoreStatus>,
}

/// Trust status in one store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustStoreStatus {
    /// Store name, e.g. `macos-keychain`, `windows-user-root`, `nss:/home/u/.pki/nssdb`, `linux-system`.
    pub store: String,
    /// Whether our CA is trusted there.
    pub trusted: bool,
    /// Explanation.
    pub detail: String,
}

fn to_chrono(t: OffsetDateTime) -> DateTime<Utc> {
    DateTime::from_timestamp(t.unix_timestamp(), 0).unwrap_or_default()
}

/// Parse the first certificate of a PEM document.
pub fn parse_cert_pem(pem: &str) -> Result<CertInfo> {
    let (_, p) = x509_parser::pem::parse_x509_pem(pem.as_bytes())
        .map_err(|e| NetError::Cert(format!("bad PEM: {e}")))?;
    let x = p
        .parse_x509()
        .map_err(|e| NetError::Cert(format!("bad certificate: {e}")))?;
    let mut dns_names = Vec::new();
    if let Ok(Some(san)) = x.subject_alternative_name() {
        for gn in &san.value.general_names {
            if let x509_parser::extensions::GeneralName::DNSName(n) = gn {
                dns_names.push(n.to_string());
            }
        }
    }
    let ts = |t: i64| DateTime::from_timestamp(t, 0).unwrap_or_default();
    Ok(CertInfo {
        subject: x.subject().to_string(),
        issuer: x.issuer().to_string(),
        dns_names,
        not_before: ts(x.validity().not_before.timestamp()),
        not_after: ts(x.validity().not_after.timestamp()),
        serial_hex: hex::encode(x.raw_serial()),
        sha256: hex::encode(Sha256::digest(&p.contents)),
        is_ca: x.is_ca(),
        issuer_raw: x.issuer().as_raw().to_vec(),
        subject_raw: x.subject().as_raw().to_vec(),
    })
}

/// Expiry of the first certificate in a PEM document.
pub fn cert_expiry(pem: &str) -> Result<DateTime<Utc>> {
    Ok(parse_cert_pem(pem)?.not_after)
}

/// `<user>@<host>` for the CA name.
pub fn user_at_host() -> String {
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "user".into());
    let host = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .ok()
        .filter(|h| !h.is_empty())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_string())
        })
        .filter(|h| !h.is_empty())
        .or_else(|| {
            crate::elevation::try_output("hostname", &[])
                .filter(|(ok, _)| *ok)
                .map(|(_, s)| s.trim().to_string())
        })
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "localhost".into());
    format!("{user}@{host}")
}

/// Common name of the CA for this user/machine.
pub fn ca_common_name() -> String {
    format!("{ORG} Local CA {}", user_at_host())
}

/// Validate a DNS name for a certificate (lowercased, no trailing dot).
pub fn normalize_domain(d: &str) -> Result<String> {
    let d = d.trim().trim_end_matches('.').to_ascii_lowercase();
    let ok = !d.is_empty()
        && d.len() <= 253
        && d.split('.').enumerate().all(|(i, l)| {
            (i == 0 && l == "*")
                || (!l.is_empty()
                    && l.len() <= 63
                    && !l.starts_with('-')
                    && !l.ends_with('-')
                    && l.chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        });
    if ok {
        Ok(d)
    } else {
        Err(NetError::Invalid(format!("invalid domain {d:?}")))
    }
}

/// SAN list for a leaf: domain, aliases, and `*.x` for each when `wildcard` (sorted, unique,
/// domain first).
pub fn leaf_sans(domain: &str, aliases: &[String], wildcard: bool) -> Result<Vec<String>> {
    let domain = normalize_domain(domain)?;
    let mut names = vec![domain.clone()];
    let mut rest = BTreeSet::new();
    let mut bases = vec![domain.clone()];
    for a in aliases {
        let a = normalize_domain(a)?;
        bases.push(a.clone());
        rest.insert(a);
    }
    if wildcard {
        for b in &bases {
            if !b.starts_with("*.") {
                rest.insert(format!("*.{b}"));
            }
        }
    }
    rest.remove(&domain);
    names.extend(rest);
    Ok(names)
}

/// Should an existing leaf be re-issued?
pub fn needs_renewal(
    leaf: &CertInfo,
    wanted_sans: &[String],
    ca: &CertInfo,
    now: DateTime<Utc>,
) -> bool {
    let have: BTreeSet<String> = leaf
        .dns_names
        .iter()
        .map(|s| s.to_ascii_lowercase())
        .collect();
    let want: BTreeSet<String> = wanted_sans.iter().map(|s| s.to_ascii_lowercase()).collect();
    leaf.days_left(now) < RENEW_BEFORE_DAYS
        || have != want
        || leaf.issuer_raw != ca.subject_raw
        || leaf.not_before > now
}

/// Write `contents` to `path` atomically (temp file + rename), creating it with `mode` on unix.
pub(crate) fn write_atomic(path: &Path, contents: &[u8], mode: u32) -> Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| NetError::Invalid(format!("no parent for {}", path.display())))?;
    std::fs::create_dir_all(dir).ctx(|| format!("creating {}", dir.display()))?;
    let tmp = dir.join(format!(
        ".{}.tmp{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
        std::process::id()
    ));
    {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(mode);
        }
        let mut f = opts
            .open(&tmp)
            .ctx(|| format!("creating {}", tmp.display()))?;
        f.write_all(contents)
            .ctx(|| format!("writing {}", tmp.display()))?;
        f.sync_all().ok();
    }
    crate::elevation::set_mode(&tmp, Some(mode))?;
    std::fs::rename(&tmp, path).ctx(|| format!("renaming to {}", path.display()))?;
    Ok(())
}

/// The local certificate authority stored in a directory.
#[derive(Debug, Clone)]
pub struct LocalCa {
    dir: PathBuf,
}

/// Result of [`LocalCa::ensure_leaf`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeafCert {
    /// Certificate PEM path.
    pub cert_path: PathBuf,
    /// Private key PEM path.
    pub key_path: PathBuf,
    /// True when a new certificate was written by this call.
    pub issued: bool,
    /// Expiry.
    pub not_after: DateTime<Utc>,
}

impl LocalCa {
    /// CA stored in `dir` (`$BOZKURT_HOME/ca`).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        LocalCa { dir: dir.into() }
    }

    /// `rootCA.pem` path.
    pub fn cert_path(&self) -> PathBuf {
        self.dir.join(CA_CERT_FILE)
    }

    /// `rootCA-key.pem` path.
    pub fn key_path(&self) -> PathBuf {
        self.dir.join(CA_KEY_FILE)
    }

    /// Both CA files exist.
    pub fn exists(&self) -> bool {
        self.cert_path().is_file() && self.key_path().is_file()
    }

    /// Read CA facts.
    pub fn info(&self) -> Result<CaInfo> {
        let pem = std::fs::read_to_string(self.cert_path())
            .map_err(|_| NetError::Missing(self.cert_path()))?;
        let ci = parse_cert_pem(&pem)?;
        let cn = ci
            .subject
            .split(',')
            .find_map(|p| p.trim().strip_prefix("CN=").map(str::to_string))
            .unwrap_or_else(|| ci.subject.clone());
        Ok(CaInfo {
            cert_path: self.cert_path(),
            key_path: self.key_path(),
            common_name: cn,
            not_after: ci.not_after,
            serial_hex: ci.serial_hex,
            sha256: ci.sha256,
        })
    }

    /// Generate a new CA (overwrites existing files). Previously trusted copies become stale.
    pub fn generate(&self) -> Result<CaInfo> {
        self.generate_named(&ca_common_name())
    }

    /// Generate a CA with an explicit common name, constrained to [`DEFAULT_PERMITTED`].
    pub fn generate_named(&self, common_name: &str) -> Result<CaInfo> {
        let permitted: Vec<String> = DEFAULT_PERMITTED.iter().map(|s| s.to_string()).collect();
        self.generate_constrained(common_name, &permitted)
    }

    /// Generate a CA whose name constraints permit only `permitted` DNS subtrees (an empty
    /// list means no constraints).
    pub fn generate_constrained(&self, common_name: &str, permitted: &[String]) -> Result<CaInfo> {
        let key = KeyPair::generate().map_err(|e| NetError::Cert(e.to_string()))?;
        let mut params = CertificateParams::default();
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, common_name);
        dn.push(DnType::OrganizationName, ORG);
        dn.push(DnType::OrganizationalUnitName, user_at_host());
        params.distinguished_name = dn;
        params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        if !permitted.is_empty() {
            let mut subtrees: Vec<String> =
                permitted.iter().map(|p| p.to_ascii_lowercase()).collect();
            subtrees.sort();
            subtrees.dedup();
            params.name_constraints = Some(NameConstraints {
                permitted_subtrees: subtrees.into_iter().map(GeneralSubtree::DnsName).collect(),
                excluded_subtrees: vec![],
            });
        }
        params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        let now = OffsetDateTime::now_utc();
        params.not_before = now - TDuration::days(1);
        params.not_after = now + TDuration::days(CA_VALIDITY_DAYS);
        let cert = params
            .self_signed(&key)
            .map_err(|e| NetError::Cert(e.to_string()))?;
        write_atomic(&self.key_path(), key.serialize_pem().as_bytes(), 0o600)?;
        write_atomic(&self.cert_path(), cert.pem().as_bytes(), 0o644)?;
        self.info()
    }

    /// Load the CA, generating it when missing, unreadable or expiring within 30 days.
    pub fn ensure(&self) -> Result<CaInfo> {
        if self.exists() {
            if let Ok(info) = self.info() {
                let key_ok = std::fs::read_to_string(self.key_path())
                    .ok()
                    .and_then(|k| KeyPair::from_pem(&k).ok())
                    .is_some();
                if key_ok && (info.not_after - Utc::now()).num_days() >= RENEW_BEFORE_DAYS {
                    return Ok(info);
                }
            }
        }
        self.generate()
    }

    fn issuer(&self) -> Result<(Issuer<'static, KeyPair>, CertInfo)> {
        let pem = std::fs::read_to_string(self.cert_path())
            .map_err(|_| NetError::Missing(self.cert_path()))?;
        let key_pem = std::fs::read_to_string(self.key_path())
            .map_err(|_| NetError::Missing(self.key_path()))?;
        let key = KeyPair::from_pem(&key_pem).map_err(|e| NetError::Cert(e.to_string()))?;
        let issuer =
            Issuer::from_ca_cert_pem(&pem, key).map_err(|e| NetError::Cert(e.to_string()))?;
        Ok((issuer, parse_cert_pem(&pem)?))
    }

    /// P1-4: make sure the (constrained) CA permits every name in `sans`; otherwise
    /// regenerate it with the missing TLDs added and drop the leaves signed by the old CA.
    fn ensure_permits(&self, certs_dir: &Path, sans: &[String]) -> Result<()> {
        let pem = std::fs::read_to_string(self.cert_path())
            .map_err(|_| NetError::Missing(self.cert_path()))?;
        let Some(mut permitted) = permitted_dns_names(&pem)? else {
            return Ok(()); // legacy unconstrained CA: keep it, no silent trust break
        };
        let missing: Vec<String> = sans
            .iter()
            .filter(|n| !name_permitted(n, &permitted))
            .map(|n| tld_of(n.trim_start_matches("*.")))
            .collect();
        if missing.is_empty() {
            return Ok(());
        }
        permitted.extend(missing);
        tracing::warn!(
            "local CA does not permit {:?}; regenerating it for {:?} (trust it again: bz tls trust)",
            sans,
            permitted
        );
        let cn = self.info()?.common_name;
        self.generate_constrained(&cn, &permitted)?;
        if let Ok(rd) = std::fs::read_dir(certs_dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().is_some_and(|x| x == "pem") {
                    let _ = std::fs::remove_file(p);
                }
            }
        }
        Ok(())
    }

    /// Issue (unconditionally) a leaf for `domain` into `certs_dir`.
    pub fn issue_leaf(
        &self,
        certs_dir: &Path,
        domain: &str,
        aliases: &[String],
        wildcard: bool,
    ) -> Result<LeafCert> {
        self.ensure()?;
        let sans = leaf_sans(domain, aliases, wildcard)?;
        let (issuer, _) = self.issuer()?;
        let key = KeyPair::generate().map_err(|e| NetError::Cert(e.to_string()))?;
        let mut params =
            CertificateParams::new(sans.clone()).map_err(|e| NetError::Cert(e.to_string()))?;
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, sans[0].clone());
        dn.push(DnType::OrganizationName, ORG);
        params.distinguished_name = dn;
        params.is_ca = IsCa::ExplicitNoCa;
        params.key_usages = vec![
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::KeyEncipherment,
        ];
        params.extended_key_usages = vec![
            ExtendedKeyUsagePurpose::ServerAuth,
            ExtendedKeyUsagePurpose::ClientAuth,
        ];
        params.use_authority_key_identifier_extension = true;
        let now = OffsetDateTime::now_utc();
        params.not_before = now - TDuration::days(1);
        params.not_after = params.not_before + TDuration::days(LEAF_VALIDITY_DAYS);
        let cert = params
            .signed_by(&key, &issuer)
            .map_err(|e| NetError::Cert(e.to_string()))?;
        let base = sans[0].trim_start_matches("*.").to_string();
        let cert_path = certs_dir.join(format!("{base}.pem"));
        let key_path = certs_dir.join(format!("{base}-key.pem"));
        write_atomic(&key_path, key.serialize_pem().as_bytes(), 0o600)?;
        write_atomic(&cert_path, cert.pem().as_bytes(), 0o644)?;
        Ok(LeafCert {
            cert_path,
            key_path,
            issued: true,
            not_after: to_chrono(params.not_after),
        })
    }

    /// Return a valid leaf for `domain`, issuing a new one only when needed
    /// (missing, < 30 days left, SANs changed, CA changed).
    pub fn ensure_leaf(
        &self,
        certs_dir: &Path,
        domain: &str,
        aliases: &[String],
        wildcard: bool,
    ) -> Result<LeafCert> {
        self.ensure()?;
        let sans = leaf_sans(domain, aliases, wildcard)?;
        self.ensure_permits(certs_dir, &sans)?;
        let base = sans[0].trim_start_matches("*.").to_string();
        let cert_path = certs_dir.join(format!("{base}.pem"));
        let key_path = certs_dir.join(format!("{base}-key.pem"));
        if key_path.is_file() {
            if let Ok(pem) = std::fs::read_to_string(&cert_path) {
                if let (Ok(leaf), Ok((_, ca))) = (parse_cert_pem(&pem), self.issuer()) {
                    if !needs_renewal(&leaf, &sans, &ca, Utc::now()) {
                        return Ok(LeafCert {
                            cert_path,
                            key_path,
                            issued: false,
                            not_after: leaf.not_after,
                        });
                    }
                }
            }
        }
        self.issue_leaf(certs_dir, domain, aliases, wildcard)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ca_is_name_constrained() {
        let t = tempfile::tempdir().unwrap();
        let ca = LocalCa::new(t.path().join("ca"));
        ca.generate_named("BOZKURT CODE Local CA t@h").unwrap();
        let pem = std::fs::read_to_string(ca.cert_path()).unwrap();
        let mut p = permitted_dns_names(&pem).unwrap().unwrap();
        p.sort();
        assert_eq!(p, vec!["localhost", "test"]);
        assert!(name_permitted("shop.test", &p));
        assert!(name_permitted("*.shop.test", &p));
        assert!(name_permitted("localhost", &p));
        assert!(!name_permitted("google.com", &p));
        assert!(!name_permitted("evil-test", &p));

        // a leaf under the permitted TLD keeps the CA
        let certs = t.path().join("certs");
        let fp = ca.info().unwrap().sha256;
        ca.ensure_leaf(&certs, "shop.test", &[], false).unwrap();
        assert_eq!(ca.info().unwrap().sha256, fp);

        // another TLD → CA regenerated with it added, old leaves dropped
        ca.ensure_leaf(&certs, "blog.local", &[], true).unwrap();
        assert_ne!(ca.info().unwrap().sha256, fp);
        assert!(!certs.join("shop.test.pem").exists());
        assert!(certs.join("blog.local.pem").exists());
        let pem = std::fs::read_to_string(ca.cert_path()).unwrap();
        let mut p = permitted_dns_names(&pem).unwrap().unwrap();
        p.sort();
        assert_eq!(p, vec!["local", "localhost", "test"]);

        // legacy unconstrained CA is left alone
        let legacy = LocalCa::new(t.path().join("legacy"));
        legacy.generate_constrained("old", &[]).unwrap();
        let fp = legacy.info().unwrap().sha256;
        legacy
            .ensure_leaf(&t.path().join("c2"), "x.example", &[], false)
            .unwrap();
        assert_eq!(legacy.info().unwrap().sha256, fp);
        let pem = std::fs::read_to_string(legacy.cert_path()).unwrap();
        assert_eq!(permitted_dns_names(&pem).unwrap(), None);
    }

    #[test]
    fn sans() {
        assert_eq!(
            leaf_sans("Shop.test.", &[], false).unwrap(),
            vec!["shop.test"]
        );
        assert_eq!(
            leaf_sans("shop.test", &["admin.shop.test".into()], true).unwrap(),
            vec![
                "shop.test",
                "*.admin.shop.test",
                "*.shop.test",
                "admin.shop.test"
            ]
        );
        assert!(leaf_sans("bad domain", &[], false).is_err());
    }

    #[test]
    fn ca_and_leaf_lifecycle() {
        let tmp = tempfile::tempdir().unwrap();
        let ca = LocalCa::new(tmp.path().join("ca"));
        let info = ca.ensure().unwrap();
        assert!(info.common_name.starts_with("BOZKURT CODE Local CA "));
        assert!(info.common_name.contains('@'));
        let days = (info.not_after - Utc::now()).num_days();
        assert!((3640..=3650).contains(&days), "{days}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let m = std::fs::metadata(ca.key_path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(m, 0o600);
        }
        // ensure() is idempotent
        assert_eq!(ca.ensure().unwrap().sha256, info.sha256);
        let ca_pem = std::fs::read_to_string(ca.cert_path()).unwrap();
        assert!(parse_cert_pem(&ca_pem).unwrap().is_ca);

        let certs = tmp.path().join("certs");
        let l1 = ca.ensure_leaf(&certs, "shop.test", &[], true).unwrap();
        assert!(l1.issued);
        assert_eq!(l1.cert_path, certs.join("shop.test.pem"));
        assert_eq!(l1.key_path, certs.join("shop.test-key.pem"));
        let leaf = parse_cert_pem(&std::fs::read_to_string(&l1.cert_path).unwrap()).unwrap();
        assert_eq!(leaf.dns_names, vec!["shop.test", "*.shop.test"]);
        assert!(!leaf.is_ca);
        assert_eq!(
            (leaf.not_after - leaf.not_before).num_days(),
            LEAF_VALIDITY_DAYS
        );
        assert!(leaf.issuer.contains("BOZKURT CODE Local CA"));
        assert_eq!(
            cert_expiry(&std::fs::read_to_string(&l1.cert_path).unwrap()).unwrap(),
            leaf.not_after
        );

        // Same request → reused; changed SANs → re-issued.
        assert!(
            !ca.ensure_leaf(&certs, "shop.test", &[], true)
                .unwrap()
                .issued
        );
        assert!(
            ca.ensure_leaf(&certs, "shop.test", &["admin.shop.test".into()], true)
                .unwrap()
                .issued
        );

        // Renewal window and CA change.
        let ca_info = parse_cert_pem(&ca_pem).unwrap();
        let sans = leaf_sans("shop.test", &["admin.shop.test".into()], true).unwrap();
        let leaf = parse_cert_pem(&std::fs::read_to_string(&l1.cert_path).unwrap()).unwrap();
        assert!(!needs_renewal(&leaf, &sans, &ca_info, Utc::now()));
        let later = leaf.not_after - chrono::Duration::days(10);
        assert!(needs_renewal(&leaf, &sans, &ca_info, later));
        ca.generate_named("BOZKURT CODE Local CA other@host")
            .unwrap();
        assert!(
            ca.ensure_leaf(&certs, "shop.test", &["admin.shop.test".into()], true)
                .unwrap()
                .issued
        );
    }
}
