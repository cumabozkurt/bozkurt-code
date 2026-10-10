//! Per-install local API token (`$BOZKURT_HOME/api-token`).
//!
//! bozkurtd creates the token at startup (32 random bytes, hex, file mode 0600) and
//! requires it on the local listener from every client that is not an allow-listed
//! browser origin (`docs/API.md` → "Local API protection"). `bz`, `bozkurt-mcp` and the
//! desktop shell read the file; it never leaves the machine.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// File name inside the BOZKURT home.
pub const FILE: &str = "api-token";
/// Alternative header to `Authorization: Bearer <token>`.
pub const HEADER: &str = "x-bozkurt-token";

/// `<home>/api-token`.
pub fn path(home: &Path) -> PathBuf {
    home.join(FILE)
}

fn valid(t: &str) -> bool {
    t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Read the token, if the daemon has created one.
pub fn read(home: &Path) -> Option<String> {
    let t = std::fs::read_to_string(path(home)).ok()?;
    let t = t.trim();
    valid(t).then(|| t.to_string())
}

/// 256 random bits as 64 hex chars (two v4 UUIDs = 244 bits from the OS RNG, plus the
/// version bits; hashed so every output bit is uniform).
pub fn generate() -> String {
    let mut h = Sha256::new();
    h.update(uuid::Uuid::new_v4().as_bytes());
    h.update(uuid::Uuid::new_v4().as_bytes());
    h.update(uuid::Uuid::new_v4().as_bytes());
    hex::encode(h.finalize())
}

/// Return the existing token or create a new one (mode 0600 on Unix).
pub fn load_or_create(home: &Path) -> std::io::Result<String> {
    if let Some(t) = read(home) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path(home), std::fs::Permissions::from_mode(0o600));
        }
        return Ok(t);
    }
    let t = generate();
    crate::fsutil::atomic_write_private(&path(home), format!("{t}\n").as_bytes())?;
    Ok(t)
}

/// Constant-time comparison (on SHA-256 digests, so length does not leak either).
pub fn matches(expected: &str, given: &str) -> bool {
    let a = Sha256::digest(expected.as_bytes());
    let b = Sha256::digest(given.trim().as_bytes());
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_read_match() {
        let d = tempfile::tempdir().unwrap();
        assert!(read(d.path()).is_none());
        let t = load_or_create(d.path()).unwrap();
        assert_eq!(t.len(), 64);
        assert_eq!(read(d.path()).as_deref(), Some(t.as_str()));
        assert_eq!(load_or_create(d.path()).unwrap(), t);
        assert!(matches(&t, &t));
        assert!(!matches(&t, "x"));
        assert_ne!(generate(), generate());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let m = std::fs::metadata(path(d.path()))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(m & 0o777, 0o600);
        }
    }
}
