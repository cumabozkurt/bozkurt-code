//! Remote (LAN) access: pairing codes, device tokens (stored as SHA-256 hashes in
//! `$BOZKURT_HOME/remote.json`), rate limiting and mDNS advertisement.
//!
//! Brute-force protection (P1-2): codes are 8 random digits (10^8), valid 5 minutes and
//! single use. Each client address gets [`MAX_ATTEMPTS_PER_MIN`] tries per minute, every
//! address together gets [`MAX_GLOBAL_ATTEMPTS_PER_MIN`], and the active code is revoked
//! after [`MAX_FAILURES_PER_CODE`] wrong guesses in total, so an attacker with many source
//! addresses gets at most 10 guesses out of 10^8 per code. The per-address table is
//! pruned and capped at [`MAX_TRACKED_ADDRS`].
//!
//! Known limitation: the remote listener speaks plain HTTP. The pairing code, the device
//! bearer token and `?token=` on `/events` travel in cleartext on the LAN, so a passive
//! sniffer on the same (e.g. public) Wi-Fi can capture a token with full API rights. Only
//! enable remote access on networks you trust and revoke devices you no longer use. TLS
//! with a local-CA leaf pinned through the QR code is the planned fix (RUST_AUDIT P1-2).

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Pairing codes live this long.
pub const CODE_TTL: Duration = Duration::from_secs(5 * 60);
/// Pairing attempts allowed per client address per minute.
pub const MAX_ATTEMPTS_PER_MIN: usize = 5;
/// Pairing attempts allowed per minute from all addresses together.
pub const MAX_GLOBAL_ATTEMPTS_PER_MIN: usize = 30;
/// Wrong guesses (from any address) after which the active code is revoked.
pub const MAX_FAILURES_PER_CODE: u32 = 10;
/// Client addresses tracked for rate limiting (oldest evicted first).
pub const MAX_TRACKED_ADDRS: usize = 1024;
/// Digits in a pairing code.
pub const CODE_DIGITS: usize = 8;
/// `last_seen` is persisted at most this often per device.
const LAST_SEEN_PERSIST: Duration = Duration::from_secs(60);

/// A paired device as stored on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredDevice {
    pub id: String,
    pub name: String,
    /// hex SHA-256 of the bearer token (the token itself is never stored).
    pub token_sha256: String,
    pub created_at: String,
    pub last_seen: String,
}

/// A paired device as returned by `GET /remote`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairedDevice {
    pub id: String,
    pub name: String,
    pub last_seen: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RemoteFile {
    #[serde(default)]
    devices: Vec<StoredDevice>,
}

/// A generated pairing code.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairingCode {
    pub code: String,
    pub expires_at: String,
}

/// Why pairing failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairError {
    BadCode,
    RateLimited,
    /// Too many wrong guesses: the code was revoked, a new one must be generated locally.
    Locked,
    Invalid(String),
}

struct ActiveCode {
    code: String,
    expires: Instant,
    failures: u32,
}

struct Inner {
    devices: Vec<StoredDevice>,
    code: Option<ActiveCode>,
    attempts: HashMap<IpAddr, VecDeque<Instant>>,
    global_attempts: VecDeque<Instant>,
    /// Set when a code was revoked for too many failures (until a new code is made).
    locked: bool,
    persisted_seen: HashMap<String, Instant>,
}

/// Device registry + pairing state.
pub struct RemoteAuth {
    path: PathBuf,
    inner: Mutex<Inner>,
}

impl std::fmt::Debug for RemoteAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteAuth")
            .field("path", &self.path)
            .finish()
    }
}

fn now_str() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Hex SHA-256 of a token.
pub fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// 256-bit random token (hex).
pub fn new_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// Uniform random [`CODE_DIGITS`]-digit code.
pub fn new_code() -> String {
    const MOD: u32 = 100_000_000; // 10^CODE_DIGITS
    loop {
        let b = uuid::Uuid::new_v4().into_bytes();
        let n = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        // rejection sampling to stay uniform
        if n < u32::MAX - (u32::MAX % MOD) {
            return format!("{:0width$}", n % MOD, width = CODE_DIGITS);
        }
    }
}

fn prune(q: &mut VecDeque<Instant>, now: Instant) {
    while q
        .front()
        .is_some_and(|t| now.duration_since(*t) > Duration::from_secs(60))
    {
        q.pop_front();
    }
}

fn eq_ct(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

impl RemoteAuth {
    /// Load `remote.json` (missing/corrupt → empty registry).
    pub fn open(path: PathBuf) -> RemoteAuth {
        let devices = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<RemoteFile>(&s).ok())
            .map(|f| f.devices)
            .unwrap_or_default();
        RemoteAuth {
            path,
            inner: Mutex::new(Inner {
                devices,
                code: None,
                attempts: HashMap::new(),
                global_attempts: VecDeque::new(),
                locked: false,
                persisted_seen: HashMap::new(),
            }),
        }
    }

    fn save(&self, devices: &[StoredDevice]) {
        let f = RemoteFile {
            devices: devices.to_vec(),
        };
        match serde_json::to_vec_pretty(&f) {
            Ok(data) => {
                if let Err(e) = bz_core::fsutil::atomic_write(&self.path, &data) {
                    tracing::warn!("writing {}: {e}", self.path.display());
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(
                        &self.path,
                        std::fs::Permissions::from_mode(0o600),
                    );
                }
            }
            Err(e) => tracing::warn!("serialize remote.json: {e}"),
        }
    }

    /// Paired devices (without secrets).
    pub fn devices(&self) -> Vec<PairedDevice> {
        self.inner
            .lock()
            .expect("remote")
            .devices
            .iter()
            .map(|d| PairedDevice {
                id: d.id.clone(),
                name: d.name.clone(),
                last_seen: d.last_seen.clone(),
            })
            .collect()
    }

    /// New pairing code (replaces any previous one), valid 5 minutes, single use.
    pub fn new_pairing_code(&self) -> PairingCode {
        let code = new_code();
        let expires_at: DateTime<Utc> =
            Utc::now() + chrono::Duration::from_std(CODE_TTL).expect("ttl");
        {
            let mut g = self.inner.lock().expect("remote");
            g.locked = false;
            g.code = Some(ActiveCode {
                code: code.clone(),
                expires: Instant::now() + CODE_TTL,
                failures: 0,
            });
        }
        PairingCode {
            code,
            expires_at: expires_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        }
    }

    /// Exchange a pairing code for a token. Returns `(device_id, token)`.
    pub fn pair(
        &self,
        from: IpAddr,
        code: &str,
        device_name: &str,
    ) -> Result<(String, String), PairError> {
        let mut g = self.inner.lock().expect("remote");
        let now = Instant::now();
        // keep the per-address table bounded (P2-3): drop idle addresses, then the
        // least recently active ones
        g.attempts.retain(|_, q| {
            prune(q, now);
            !q.is_empty()
        });
        if !g.attempts.contains_key(&from) && g.attempts.len() >= MAX_TRACKED_ADDRS {
            let oldest = g
                .attempts
                .iter()
                .min_by_key(|(_, q)| q.back().copied())
                .map(|(ip, _)| *ip);
            if let Some(ip) = oldest {
                g.attempts.remove(&ip);
            }
        }
        prune(&mut g.global_attempts, now);
        let q = g.attempts.entry(from).or_default();
        if q.len() >= MAX_ATTEMPTS_PER_MIN {
            return Err(PairError::RateLimited);
        }
        q.push_back(now);
        if g.global_attempts.len() >= MAX_GLOBAL_ATTEMPTS_PER_MIN {
            return Err(PairError::RateLimited);
        }
        g.global_attempts.push_back(now);
        if g.locked {
            return Err(PairError::Locked);
        }
        let name = device_name.trim();
        if name.is_empty() || name.len() > 100 {
            return Err(PairError::Invalid(
                "device_name must be 1-100 characters".into(),
            ));
        }
        let valid = match &g.code {
            Some(c) => c.expires > now && eq_ct(c.code.as_bytes(), code.trim().as_bytes()),
            None => false,
        };
        if !valid {
            if let Some(c) = g.code.as_mut() {
                c.failures += 1;
                if c.failures >= MAX_FAILURES_PER_CODE {
                    tracing::warn!(
                        "pairing code revoked after {MAX_FAILURES_PER_CODE} wrong guesses (last from {from})"
                    );
                    g.code = None;
                    g.locked = true;
                    return Err(PairError::Locked);
                }
            }
            return Err(PairError::BadCode);
        }
        g.code = None; // single use
        let token = new_token();
        let id = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
        let ts = now_str();
        g.devices.push(StoredDevice {
            id: id.clone(),
            name: name.to_string(),
            token_sha256: hash_token(&token),
            created_at: ts.clone(),
            last_seen: ts,
        });
        let devices = g.devices.clone();
        drop(g);
        self.save(&devices);
        Ok((id, token))
    }

    /// Check a bearer token; on success returns the device id and refreshes `last_seen`.
    pub fn authorize(&self, token: &str) -> Option<String> {
        let h = hash_token(token);
        let mut g = self.inner.lock().expect("remote");
        let idx = g
            .devices
            .iter()
            .position(|d| eq_ct(d.token_sha256.as_bytes(), h.as_bytes()))?;
        let id = g.devices[idx].id.clone();
        g.devices[idx].last_seen = now_str();
        let now = Instant::now();
        let persist = g
            .persisted_seen
            .get(&id)
            .is_none_or(|t| now.duration_since(*t) > LAST_SEEN_PERSIST);
        if persist {
            g.persisted_seen.insert(id.clone(), now);
            let devices = g.devices.clone();
            drop(g);
            self.save(&devices);
        }
        Some(id)
    }

    /// Forget a device (its token stops working immediately).
    pub fn remove(&self, id: &str) -> bool {
        let mut g = self.inner.lock().expect("remote");
        let before = g.devices.len();
        g.devices.retain(|d| d.id != id);
        let removed = g.devices.len() != before;
        let devices = g.devices.clone();
        drop(g);
        if removed {
            self.save(&devices);
        }
        removed
    }
}

/// Non-loopback IPv4 addresses of this machine (for `GET /remote` and mDNS).
pub fn lan_addresses() -> Vec<IpAddr> {
    let mut v: Vec<IpAddr> = if_addrs::get_if_addrs()
        .map(|l| {
            l.into_iter()
                .filter(|i| !i.is_loopback())
                .map(|i| i.ip())
                .filter(|ip| match ip {
                    IpAddr::V4(a) => !a.is_link_local(),
                    IpAddr::V6(_) => false,
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v.dedup();
    v
}

/// Best-effort hostname.
pub fn hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_string())
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "bozkurt".into())
}

/// mDNS advertisement of `_bozkurt._tcp` (best effort; dropped → unregistered).
pub struct MdnsAdvert {
    daemon: mdns_sd::ServiceDaemon,
    fullname: String,
}

impl MdnsAdvert {
    /// Advertise `port` on all LAN addresses. Errors are returned for logging only.
    pub fn start(port: u16) -> Result<MdnsAdvert, String> {
        let daemon = mdns_sd::ServiceDaemon::new().map_err(|e| e.to_string())?;
        let host = hostname();
        let label: String = host
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let instance = format!("BOZKURT CODE on {host}");
        let props = [("version", bz_core::engine::VERSION), ("api", "/api/v1")];
        let addrs = lan_addresses();
        let info = mdns_sd::ServiceInfo::new(
            "_bozkurt._tcp.local.",
            &instance,
            &format!("{label}.local."),
            &addrs[..],
            port,
            &props[..],
        )
        .map_err(|e| e.to_string())?
        .enable_addr_auto();
        let fullname = info.get_fullname().to_string();
        daemon.register(info).map_err(|e| e.to_string())?;
        Ok(MdnsAdvert { daemon, fullname })
    }
}

impl Drop for MdnsAdvert {
    fn drop(&mut self) {
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_and_tokens() {
        for _ in 0..50 {
            let c = new_code();
            assert_eq!(c.len(), CODE_DIGITS);
            assert!(c.chars().all(|c| c.is_ascii_digit()));
        }
        let t = new_token();
        assert_eq!(t.len(), 64);
        assert_eq!(hash_token("a").len(), 64);
    }

    #[test]
    fn pairing_flow() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remote.json");
        let r = RemoteAuth::open(path.clone());
        let ip: IpAddr = "10.0.0.2".parse().unwrap();
        assert_eq!(r.pair(ip, "123456", "x"), Err(PairError::BadCode));
        let c = r.new_pairing_code();
        let (id, tok) = r.pair(ip, &c.code, "Phone").unwrap();
        assert_eq!(r.pair(ip, &c.code, "Phone"), Err(PairError::BadCode));
        assert_eq!(r.authorize(&tok), Some(id.clone()));
        assert_eq!(r.authorize("nope"), None);
        let disk = std::fs::read_to_string(&path).unwrap();
        assert!(!disk.contains(&tok));
        assert!(disk.contains(&hash_token(&tok)));
        // reload from disk
        let r2 = RemoteAuth::open(path);
        assert_eq!(r2.devices().len(), 1);
        assert!(r2.authorize(&tok).is_some());
        assert!(r2.remove(&id));
        assert!(r2.authorize(&tok).is_none());
        // rate limit: 5 attempts per minute
        let ip2: IpAddr = "10.0.0.3".parse().unwrap();
        for _ in 0..5 {
            assert_eq!(r2.pair(ip2, "000000", "x"), Err(PairError::BadCode));
        }
        assert_eq!(r2.pair(ip2, "000000", "x"), Err(PairError::RateLimited));
    }

    #[test]
    fn code_is_revoked_after_global_failures() {
        let dir = tempfile::tempdir().unwrap();
        let r = RemoteAuth::open(dir.path().join("remote.json"));
        let c = r.new_pairing_code();
        let wrong = if c.code == "00000000" {
            "11111111"
        } else {
            "00000000"
        };
        // spread over many addresses so the per-address limit never triggers
        for i in 0..MAX_FAILURES_PER_CODE - 1 {
            let ip: IpAddr = format!("10.0.1.{i}").parse().unwrap();
            assert_eq!(r.pair(ip, wrong, "x"), Err(PairError::BadCode));
        }
        let ip: IpAddr = "10.0.2.1".parse().unwrap();
        assert_eq!(r.pair(ip, wrong, "x"), Err(PairError::Locked));
        // the right code no longer works either
        let ip: IpAddr = "10.0.2.2".parse().unwrap();
        assert_eq!(r.pair(ip, &c.code, "x"), Err(PairError::Locked));
        // a fresh code unlocks
        let c2 = r.new_pairing_code();
        let ip: IpAddr = "10.0.2.3".parse().unwrap();
        assert!(r.pair(ip, &c2.code, "Phone").is_ok());
    }

    #[test]
    fn global_rate_limit_and_bounded_table() {
        let dir = tempfile::tempdir().unwrap();
        let r = RemoteAuth::open(dir.path().join("remote.json"));
        let mut limited = 0;
        for i in 0..(MAX_GLOBAL_ATTEMPTS_PER_MIN + 5) {
            let ip: IpAddr = format!("10.1.{}.{}", i / 256, i % 256).parse().unwrap();
            if r.pair(ip, "00000000", "x") == Err(PairError::RateLimited) {
                limited += 1;
            }
        }
        assert_eq!(limited, 5);
        for i in 0..(MAX_TRACKED_ADDRS + 50) {
            let ip: IpAddr = format!("10.2.{}.{}", i / 256, i % 256).parse().unwrap();
            let _ = r.pair(ip, "00000000", "x");
        }
        assert!(r.inner.lock().unwrap().attempts.len() <= MAX_TRACKED_ADDRS);
    }
}
