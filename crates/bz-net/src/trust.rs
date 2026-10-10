//! Trusting the local CA in OS and browser trust stores.
//!
//! | OS | user store (no admin) | optional system store (admin) |
//! |---|---|---|
//! | macOS | `security add-trusted-cert -r trustRoot -k ~/Library/Keychains/login.keychain-db` (macOS shows its own auth dialog) | `security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain` |
//! | Windows | `certutil -user -addstore Root` (Windows shows a confirm dialog) | `certutil -addstore Root` |
//! | Linux | NSS dbs via `certutil` (`~/.pki/nssdb` for Chrome/Chromium, each Firefox profile) | `update-ca-certificates` / `update-ca-trust` / `trust anchor` |
//!
//! [`trust_plan`] / [`untrust_plan`] are pure; [`detect_env`] and [`status`] inspect the machine.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::plan::{Action, Os, Plan};
use crate::tls::{CaInfo, TrustStoreStatus};

/// File name used when the CA is copied into a Linux system anchor directory.
pub const SYSTEM_ANCHOR_NAME: &str = "bozkurt-code-local-ca";

/// Linux system trust store flavour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinuxSystemStore {
    /// Debian/Ubuntu: `/usr/local/share/ca-certificates/*.crt` + `update-ca-certificates`.
    Debian,
    /// Fedora/RHEL style: `/etc/pki/ca-trust/source/anchors/` + `update-ca-trust extract`.
    Fedora,
    /// Arch & others with p11-kit: `trust anchor --store`.
    P11Kit,
}

impl LinuxSystemStore {
    /// Anchor file path for this flavour (P11Kit manages its own files).
    pub fn anchor_path(self) -> Option<PathBuf> {
        match self {
            LinuxSystemStore::Debian => Some(PathBuf::from(format!(
                "/usr/local/share/ca-certificates/{SYSTEM_ANCHOR_NAME}.crt"
            ))),
            LinuxSystemStore::Fedora => Some(PathBuf::from(format!(
                "/etc/pki/ca-trust/source/anchors/{SYSTEM_ANCHOR_NAME}.pem"
            ))),
            LinuxSystemStore::P11Kit => None,
        }
    }
}

/// Facts about the machine's trust tooling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustEnv {
    /// User home directory.
    pub home: PathBuf,
    /// NSS `certutil` is on PATH (Linux; package libnss3-tools / nss-tools).
    pub nss_certutil: bool,
    /// Existing NSS databases (`~/.pki/nssdb`, Firefox profiles).
    pub nss_dbs: Vec<PathBuf>,
    /// Detected Linux system store.
    pub linux_system: Option<LinuxSystemStore>,
}

/// Default Chrome/Chromium NSS db.
pub fn chrome_nssdb(home: &Path) -> PathBuf {
    home.join(".pki").join("nssdb")
}

/// macOS login keychain path.
pub fn login_keychain(home: &Path) -> PathBuf {
    home.join("Library/Keychains/login.keychain-db")
}

/// NSS nickname for our CA.
pub fn nss_nickname(ca: &CaInfo) -> String {
    ca.common_name.clone()
}

/// Plan that trusts the CA (`ca_pem` = contents of `rootCA.pem`). User-store steps are
/// unprivileged; `include_system` adds elevated system-store steps.
pub fn trust_plan(os: Os, env: &TrustEnv, ca: &CaInfo, ca_pem: &str, include_system: bool) -> Plan {
    let cert = ca.cert_path.to_string_lossy().to_string();
    let mut p = Plan::new(format!("Trust the local CA \"{}\"", ca.common_name), os);
    match os {
        Os::Macos => {
            let kc = login_keychain(&env.home).to_string_lossy().to_string();
            p.run(
                "Add the CA to your login keychain as a trusted root (macOS will ask for your password)",
                "security",
                ["add-trusted-cert", "-r", "trustRoot", "-k", kc.as_str(), cert.as_str()],
                false,
            );
            if include_system {
                p.run(
                    "Add the CA to the System keychain as a trusted root",
                    "security",
                    [
                        "add-trusted-cert",
                        "-d",
                        "-r",
                        "trustRoot",
                        "-k",
                        "/Library/Keychains/System.keychain",
                        cert.as_str(),
                    ],
                    true,
                );
            }
            p.notes.push("Firefox uses its own certificate store unless security.enterprise_roots.enabled is on.".into());
        }
        Os::Windows => {
            p.run(
                "Add the CA to your user Trusted Root store (Windows will ask to confirm)",
                "certutil",
                ["-user", "-addstore", "Root", cert.as_str()],
                false,
            );
            if include_system {
                p.run(
                    "Add the CA to the machine Trusted Root store",
                    "certutil",
                    ["-addstore", "Root", cert.as_str()],
                    true,
                );
            }
        }
        Os::Linux => {
            let nick = nss_nickname(ca);
            if env.nss_certutil {
                let mut dbs = env.nss_dbs.clone();
                let chrome = chrome_nssdb(&env.home);
                if !dbs.contains(&chrome) {
                    let d = chrome.to_string_lossy().to_string();
                    p.shell(
                        format!("Create the Chrome/Chromium NSS database {d}"),
                        format!(
                            "mkdir -p {q} && certutil -d sql:{q} -N --empty-password",
                            q = crate::elevation::sh_quote(&d)
                        ),
                        false,
                    );
                    dbs.insert(0, chrome);
                }
                for db in &dbs {
                    let d = format!("sql:{}", db.display());
                    p.push(
                        format!("Remove any stale copy of the CA from {}", db.display()),
                        Action::Run {
                            program: "certutil".into(),
                            args: vec![
                                "-d".into(),
                                d.clone(),
                                "-D".into(),
                                "-n".into(),
                                nick.clone(),
                            ],
                            ignore_failure: true,
                        },
                        false,
                    );
                    p.run(
                        format!("Trust the CA in NSS database {}", db.display()),
                        "certutil",
                        [
                            "-d",
                            d.as_str(),
                            "-A",
                            "-t",
                            "C,,",
                            "-n",
                            nick.as_str(),
                            "-i",
                            cert.as_str(),
                        ],
                        false,
                    );
                }
            } else {
                p.notes.push(
                    "certutil (libnss3-tools / nss-tools) is not installed: Chrome and Firefox will not trust the CA until it is installed and trust is re-run.".into(),
                );
            }
            if include_system {
                match env.linux_system {
                    Some(store @ (LinuxSystemStore::Debian | LinuxSystemStore::Fedora)) => {
                        let path = store.anchor_path().unwrap_or_default();
                        p.write_file(
                            format!("Copy the CA to the system anchors ({})", path.display()),
                            path,
                            ca_pem,
                            Some(0o644),
                            true,
                        );
                        if store == LinuxSystemStore::Debian {
                            p.run("Rebuild the system CA bundle", "update-ca-certificates", Vec::<String>::new(), true);
                        } else {
                            p.run("Rebuild the system CA bundle", "update-ca-trust", ["extract"], true);
                        }
                    }
                    Some(LinuxSystemStore::P11Kit) => {
                        p.run(
                            "Add the CA to the p11-kit system trust",
                            "trust",
                            ["anchor", "--store", cert.as_str()],
                            true,
                        );
                    }
                    None => p
                        .notes
                        .push("No supported system trust tool found (update-ca-certificates, update-ca-trust, trust).".into()),
                }
            }
        }
    }
    p
}

/// Plan that removes the CA from the stores [`trust_plan`] writes to.
pub fn untrust_plan(os: Os, env: &TrustEnv, ca: &CaInfo, include_system: bool) -> Plan {
    let cert = ca.cert_path.to_string_lossy().to_string();
    let mut p = Plan::new(
        format!("Stop trusting the local CA \"{}\"", ca.common_name),
        os,
    );
    let ignore = |program: &str, args: Vec<String>| Action::Run {
        program: program.into(),
        args,
        ignore_failure: true,
    };
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    match os {
        Os::Macos => {
            p.push(
                "Remove trust settings for the CA",
                ignore("security", s(&["remove-trusted-cert", &cert])),
                false,
            );
            let kc = login_keychain(&env.home).to_string_lossy().to_string();
            p.push(
                "Delete the CA from your login keychain",
                ignore(
                    "security",
                    s(&["delete-certificate", "-Z", &ca.sha256.to_uppercase(), &kc]),
                ),
                false,
            );
            if include_system {
                p.push(
                    "Delete the CA from the System keychain",
                    ignore(
                        "security",
                        s(&[
                            "delete-certificate",
                            "-Z",
                            &ca.sha256.to_uppercase(),
                            "/Library/Keychains/System.keychain",
                        ]),
                    ),
                    true,
                );
            }
        }
        Os::Windows => {
            p.push(
                "Remove the CA from your user Root store",
                ignore(
                    "certutil",
                    s(&["-user", "-delstore", "Root", &ca.serial_hex]),
                ),
                false,
            );
            if include_system {
                p.push(
                    "Remove the CA from the machine Root store",
                    ignore("certutil", s(&["-delstore", "Root", &ca.serial_hex])),
                    true,
                );
            }
        }
        Os::Linux => {
            if env.nss_certutil {
                for db in &env.nss_dbs {
                    let d = format!("sql:{}", db.display());
                    p.push(
                        format!("Remove the CA from NSS database {}", db.display()),
                        ignore("certutil", s(&["-d", &d, "-D", "-n", &nss_nickname(ca)])),
                        false,
                    );
                }
            }
            if include_system {
                match env.linux_system {
                    Some(store @ (LinuxSystemStore::Debian | LinuxSystemStore::Fedora)) => {
                        p.remove_file(
                            "Remove the CA from the system anchors",
                            store.anchor_path().unwrap_or_default(),
                            true,
                        );
                        if store == LinuxSystemStore::Debian {
                            p.run(
                                "Rebuild the system CA bundle",
                                "update-ca-certificates",
                                ["--fresh"],
                                true,
                            );
                        } else {
                            p.run(
                                "Rebuild the system CA bundle",
                                "update-ca-trust",
                                ["extract"],
                                true,
                            );
                        }
                    }
                    Some(LinuxSystemStore::P11Kit) => {
                        p.push(
                            "Remove the CA from p11-kit trust",
                            ignore("trust", s(&["anchor", "--remove", &cert])),
                            true,
                        );
                    }
                    None => {}
                }
            }
        }
    }
    p
}

fn on_path(program: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|d| {
        d.join(program).is_file() || (cfg!(windows) && d.join(format!("{program}.exe")).is_file())
    })
}

/// Firefox profile directories (with a `cert9.db`) under `home`.
pub fn firefox_profiles(home: &Path) -> Vec<PathBuf> {
    let roots = [
        home.join(".mozilla/firefox"),
        home.join("snap/firefox/common/.mozilla/firefox"),
        home.join(".var/app/org.mozilla.firefox/.mozilla/firefox"),
    ];
    let mut out = Vec::new();
    for r in roots {
        if let Ok(rd) = std::fs::read_dir(&r) {
            for e in rd.flatten() {
                let p = e.path();
                if p.join("cert9.db").is_file() {
                    out.push(p);
                }
            }
        }
    }
    out.sort();
    out
}

/// Inspect the machine's trust tooling for the user whose home is `home`.
pub fn detect_env(home: &Path) -> TrustEnv {
    let mut nss_dbs = Vec::new();
    let chrome = chrome_nssdb(home);
    if chrome.join("cert9.db").is_file() {
        nss_dbs.push(chrome);
    }
    nss_dbs.extend(firefox_profiles(home));
    let linux_system = if Os::current() != Os::Linux {
        None
    } else if on_path("update-ca-certificates")
        && Path::new("/usr/local/share/ca-certificates").is_dir()
    {
        Some(LinuxSystemStore::Debian)
    } else if on_path("update-ca-trust") {
        Some(LinuxSystemStore::Fedora)
    } else if on_path("trust") {
        Some(LinuxSystemStore::P11Kit)
    } else {
        None
    };
    TrustEnv {
        home: home.to_path_buf(),
        nss_certutil: Os::current() == Os::Linux && on_path("certutil"),
        nss_dbs,
        linux_system,
    }
}

/// Trust status of the CA in every relevant store on this machine. The first entry is the
/// primary store for the OS (what `ca_trusted` reports): macOS keychain verification,
/// Windows user Root store, Linux: any NSS db or the system store.
pub fn status(env: &TrustEnv, ca: &CaInfo, ca_pem: &str) -> Vec<TrustStoreStatus> {
    let cert = ca.cert_path.to_string_lossy().to_string();
    let mut out = Vec::new();
    match Os::current() {
        Os::Macos => {
            let r = crate::elevation::try_output("security", &["verify-cert", "-c", &cert]);
            out.push(TrustStoreStatus {
                store: "macos-keychain".into(),
                trusted: r.as_ref().is_some_and(|(ok, _)| *ok),
                detail: r
                    .map(|(_, s)| s.trim().to_string())
                    .unwrap_or_else(|| "security not available".into()),
            });
        }
        Os::Windows => {
            let r = crate::elevation::try_output(
                "certutil",
                &["-user", "-verifystore", "Root", &ca.serial_hex],
            );
            out.push(TrustStoreStatus {
                store: "windows-user-root".into(),
                trusted: r.as_ref().is_some_and(|(ok, _)| *ok),
                detail: format!("serial {}", ca.serial_hex),
            });
        }
        Os::Linux => {
            for db in &env.nss_dbs {
                let d = format!("sql:{}", db.display());
                let ok = env.nss_certutil
                    && crate::elevation::try_output(
                        "certutil",
                        &["-d", &d, "-L", "-n", &nss_nickname(ca)],
                    )
                    .is_some_and(|(ok, _)| ok);
                out.push(TrustStoreStatus {
                    store: format!("nss:{}", db.display()),
                    trusted: ok,
                    detail: if env.nss_certutil {
                        "certutil -L".into()
                    } else {
                        "certutil not installed".into()
                    },
                });
            }
            let (sys_ok, detail) = match env.linux_system {
                Some(store @ (LinuxSystemStore::Debian | LinuxSystemStore::Fedora)) => {
                    let path = store.anchor_path().unwrap_or_default();
                    let same = std::fs::read_to_string(&path)
                        .map(|s| s.trim() == ca_pem.trim())
                        .unwrap_or(false);
                    (same, format!("{}", path.display()))
                }
                Some(LinuxSystemStore::P11Kit) => {
                    let ok =
                        crate::elevation::try_output("trust", &["list", "--filter=ca-anchors"])
                            .is_some_and(|(ok, s)| ok && s.contains(&ca.common_name));
                    (ok, "trust list --filter=ca-anchors".into())
                }
                None => (false, "no system trust tool".into()),
            };
            out.push(TrustStoreStatus {
                store: "linux-system".into(),
                trusted: sys_ok,
                detail,
            });
            let any = out.iter().any(|s| s.trusted);
            out.insert(
                0,
                TrustStoreStatus {
                    store: "linux-any".into(),
                    trusted: any,
                    detail: "trusted by at least one NSS db or the system store".into(),
                },
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ca() -> CaInfo {
        CaInfo {
            cert_path: PathBuf::from("/h/.bozkurt/ca/rootCA.pem"),
            key_path: PathBuf::from("/h/.bozkurt/ca/rootCA-key.pem"),
            common_name: "BOZKURT CODE Local CA u@h".into(),
            not_after: chrono::Utc::now(),
            serial_hex: "0abc".into(),
            sha256: "ff00".into(),
        }
    }

    fn env(certutil: bool, sys: Option<LinuxSystemStore>) -> TrustEnv {
        TrustEnv {
            home: PathBuf::from("/h"),
            nss_certutil: certutil,
            nss_dbs: vec![PathBuf::from("/h/.mozilla/firefox/abc.default")],
            linux_system: sys,
        }
    }

    fn runs(p: &Plan) -> Vec<String> {
        p.steps
            .iter()
            .filter_map(|s| match &s.action {
                Action::Run { program, args, .. } => Some(format!("{program} {}", args.join(" "))),
                Action::Shell { script, .. } => Some(script.clone()),
                _ => None,
            })
            // plans for macOS/Linux joined on a Windows test host contain `\`
            .map(|l| {
                if cfg!(windows) {
                    l.replace('\\', "/")
                } else {
                    l
                }
            })
            .collect()
    }

    #[test]
    fn macos() {
        let p = trust_plan(Os::Macos, &env(false, None), &ca(), "PEM", false);
        assert_eq!(
            runs(&p),
            vec!["security add-trusted-cert -r trustRoot -k /h/Library/Keychains/login.keychain-db /h/.bozkurt/ca/rootCA.pem"]
        );
        assert!(!p.requires_elevation());
        let p = trust_plan(Os::Macos, &env(false, None), &ca(), "PEM", true);
        assert!(p.requires_elevation());
        assert!(runs(&p)[1].contains("/Library/Keychains/System.keychain"));
    }

    #[test]
    fn windows() {
        let p = trust_plan(Os::Windows, &env(false, None), &ca(), "PEM", false);
        assert_eq!(
            runs(&p),
            vec!["certutil -user -addstore Root /h/.bozkurt/ca/rootCA.pem"]
        );
        assert!(!p.requires_elevation());
        let u = untrust_plan(Os::Windows, &env(false, None), &ca(), false);
        assert_eq!(runs(&u), vec!["certutil -user -delstore Root 0abc"]);
    }

    #[test]
    #[cfg_attr(
        windows,
        ignore = "Linux plans are only built on Linux; Windows paths get shell-quoted"
    )]
    fn linux_nss_and_system() {
        let p = trust_plan(
            Os::Linux,
            &env(true, Some(LinuxSystemStore::Debian)),
            &ca(),
            "PEM",
            true,
        );
        let r = runs(&p);
        assert!(r[0].contains("certutil -d sql:/h/.pki/nssdb -N --empty-password"));
        assert!(r.iter().any(|l| l == "certutil -d sql:/h/.pki/nssdb -A -t C,, -n BOZKURT CODE Local CA u@h -i /h/.bozkurt/ca/rootCA.pem"));
        assert!(r
            .iter()
            .any(|l| l.starts_with("certutil -d sql:/h/.mozilla/firefox/abc.default -A")));
        assert!(r.iter().any(|l| l == "update-ca-certificates "));
        let w = p.steps.iter().find_map(|s| match &s.action {
            Action::WriteFile { path, contents, .. } => {
                Some((path.clone(), contents.clone(), s.elevated))
            }
            _ => None,
        });
        assert_eq!(
            w,
            Some((
                PathBuf::from("/usr/local/share/ca-certificates/bozkurt-code-local-ca.crt"),
                "PEM".into(),
                true
            ))
        );

        let f = trust_plan(
            Os::Linux,
            &env(false, Some(LinuxSystemStore::Fedora)),
            &ca(),
            "PEM",
            true,
        );
        assert!(runs(&f).iter().any(|l| l == "update-ca-trust extract"));
        assert!(f.notes.iter().any(|n| n.contains("certutil")));
        let a = trust_plan(
            Os::Linux,
            &env(false, Some(LinuxSystemStore::P11Kit)),
            &ca(),
            "PEM",
            true,
        );
        assert_eq!(
            runs(&a),
            vec!["trust anchor --store /h/.bozkurt/ca/rootCA.pem"]
        );
        // user-only: nothing elevated
        assert!(!trust_plan(
            Os::Linux,
            &env(true, Some(LinuxSystemStore::Debian)),
            &ca(),
            "PEM",
            false
        )
        .requires_elevation());
    }
}
