//! Small filesystem helpers: atomic writes, backups, tailing files, directory links.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Write `data` to `path` atomically: write a sibling temp file, fsync, rename over.
pub fn atomic_write(path: &Path, data: &[u8]) -> std::io::Result<()> {
    atomic_write_mode(path, data, None)
}

/// [`atomic_write`] for secrets: on Unix the temp file is created with mode `0600`, so
/// the content is never readable by other users, not even between write and rename.
/// (On Windows files under `%LOCALAPPDATA%` inherit a user-only ACL.)
pub fn atomic_write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    atomic_write_mode(path, data, Some(0o600))
}

fn atomic_write_mode(path: &Path, data: &[u8], mode: Option<u32>) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    let tmp = dir.join(format!(".{name}.{}.tmp", uuid::Uuid::new_v4().simple()));
    {
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        if let Some(m) = mode {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(m);
        }
        #[cfg(not(unix))]
        let _ = mode;
        let mut f = o.open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Copy `file` (which lives outside the BOZKURT home) into `backup_dir`, mirroring its
/// absolute path, before it is modified. Returns the backup path, or `None` if the file
/// did not exist.
pub fn backup_file(file: &Path, backup_dir: &Path) -> std::io::Result<Option<PathBuf>> {
    if !file.exists() {
        return Ok(None);
    }
    let rel: PathBuf = file
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => Some(s.to_owned()),
            _ => None,
        })
        .collect();
    let dest = backup_dir.join(rel);
    if let Some(p) = dest.parent() {
        std::fs::create_dir_all(p)?;
    }
    std::fs::copy(file, &dest)?;
    Ok(Some(dest))
}

/// Return the last `n` lines of a text file (reads at most the last 1 MiB).
pub fn tail_lines(path: &Path, n: usize) -> std::io::Result<Vec<String>> {
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e),
    };
    let len = f.metadata()?.len();
    let window = 1024 * 1024u64;
    let start = len.saturating_sub(window);
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    let text = String::from_utf8_lossy(&buf);
    let mut lines: Vec<&str> = text.lines().collect();
    if start > 0 && !lines.is_empty() {
        lines.remove(0); // partial first line
    }
    let skip = lines.len().saturating_sub(n);
    Ok(lines[skip..].iter().map(|s| s.to_string()).collect())
}

/// Point `link` at directory `target` atomically where the OS allows it.
///
/// * unix: create a symlink at a temp name then `rename` it over `link` (atomic).
/// * Windows: a directory junction (needs no privileges), created natively with
///   `FSCTL_SET_REPARSE_POINT` (the `junction` crate) at a temp name and then moved over
///   `link`, so a failure never leaves `link` missing. No shell is involved, so paths with
///   `&`, `%`, `^`, `(`, `)` or spaces are safe (P1-9).
pub fn replace_dir_link(target: &Path, link: &Path) -> std::io::Result<()> {
    if let Some(p) = link.parent() {
        std::fs::create_dir_all(p)?;
    }
    #[cfg(unix)]
    {
        let tmp = link.with_file_name(format!(
            ".{}.{}.tmp",
            link.file_name().unwrap_or_default().to_string_lossy(),
            uuid::Uuid::new_v4().simple()
        ));
        std::os::unix::fs::symlink(target, &tmp)?;
        if let Err(e) = std::fs::rename(&tmp, link) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        let tmp = link.with_file_name(format!(
            ".{}.{}.tmp",
            link.file_name().unwrap_or_default().to_string_lossy(),
            uuid::Uuid::new_v4().simple()
        ));
        // junction targets must be absolute
        let target = if target.is_absolute() {
            target.to_path_buf()
        } else {
            std::env::current_dir()?.join(target)
        };
        junction::create(&target, &tmp)?;
        if link.symlink_metadata().is_err() {
            return std::fs::rename(&tmp, link).inspect_err(|_| {
                let _ = std::fs::remove_dir(&tmp);
            });
        }
        // MoveFileEx cannot replace a directory, so swap with the shortest gap possible:
        // the new junction already exists when the old one is removed.
        if std::fs::rename(&tmp, link).is_err() {
            // A junction is removed with remove_dir (does not touch the target).
            if let Err(e) = std::fs::remove_dir(link).or_else(|_| std::fs::remove_file(link)) {
                let _ = std::fs::remove_dir(&tmp);
                return Err(e);
            }
            if let Err(e) = std::fs::rename(&tmp, link) {
                let _ = std::fs::remove_dir(&tmp);
                return Err(e);
            }
        }
        Ok(())
    }
}

/// Resolve where a directory link points (None if missing).
pub fn read_dir_link(link: &Path) -> Option<PathBuf> {
    if link.symlink_metadata().is_err() {
        return None;
    }
    std::fs::read_link(link)
        .ok()
        .or_else(|| std::fs::canonicalize(link).ok())
}

/// Remove a directory link without touching its target.
pub fn remove_dir_link(link: &Path) -> std::io::Result<()> {
    match link.symlink_metadata() {
        Err(_) => Ok(()),
        Ok(m) if m.file_type().is_symlink() => {
            std::fs::remove_file(link).or_else(|_| std::fs::remove_dir(link))
        }
        Ok(_) => std::fs::remove_dir(link),
    }
}

/// Mark a file executable (no-op on Windows).
pub fn make_executable(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut p = std::fs::metadata(path)?.permissions();
        p.set_mode(p.mode() | 0o755);
        std::fs::set_permissions(path, p)?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_and_tail() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("a/b.txt");
        atomic_write(&f, b"one\ntwo\nthree\n").unwrap();
        assert_eq!(tail_lines(&f, 2).unwrap(), vec!["two", "three"]);
        atomic_write(&f, b"x\n").unwrap();
        assert_eq!(tail_lines(&f, 5).unwrap(), vec!["x"]);
        assert!(tail_lines(&t.path().join("missing"), 5).unwrap().is_empty());
        // no temp files left behind
        let n = std::fs::read_dir(t.path().join("a")).unwrap().count();
        assert_eq!(n, 1);
    }

    #[test]
    fn backup_mirrors_path() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("etc/hosts");
        std::fs::create_dir_all(src.parent().unwrap()).unwrap();
        std::fs::write(&src, "127.0.0.1 x").unwrap();
        let b = backup_file(&src, &t.path().join("backups/1"))
            .unwrap()
            .unwrap();
        assert_eq!(std::fs::read_to_string(b).unwrap(), "127.0.0.1 x");
        assert!(backup_file(&t.path().join("nope"), t.path())
            .unwrap()
            .is_none());
    }

    #[test]
    fn dir_link_replace() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("a");
        let b = t.path().join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("f"), "A").unwrap();
        std::fs::write(b.join("f"), "B").unwrap();
        let link = t.path().join("cur/node");
        replace_dir_link(&a, &link).unwrap();
        assert_eq!(std::fs::read_to_string(link.join("f")).unwrap(), "A");
        replace_dir_link(&b, &link).unwrap();
        assert_eq!(std::fs::read_to_string(link.join("f")).unwrap(), "B");
        assert_eq!(read_dir_link(&link).unwrap(), b);
        remove_dir_link(&link).unwrap();
        assert!(b.join("f").exists());
        assert!(link.symlink_metadata().is_err());
    }
}
