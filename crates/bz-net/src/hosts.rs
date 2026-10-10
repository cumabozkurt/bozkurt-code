//! Managed block inside the OS hosts file (fallback when wildcard DNS routing is unavailable).
//!
//! Only lines between [`BEGIN_MARK`] and [`END_MARK`] are ever touched. Each managed host gets
//! `127.0.0.1 <host>` and `::1 <host>` lines. All functions here are pure.

use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::plan::Os;

/// Opening marker of the managed block.
pub const BEGIN_MARK: &str = "# BEGIN BOZKURT CODE (managed, do not edit)";
/// Closing marker of the managed block.
pub const END_MARK: &str = "# END BOZKURT CODE";

/// Path of the hosts file on `os`.
pub fn hosts_path(os: Os) -> PathBuf {
    match os {
        Os::Windows => {
            let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
            PathBuf::from(format!(r"{root}\System32\drivers\etc\hosts"))
        }
        _ => PathBuf::from("/etc/hosts"),
    }
}

/// Host names currently in the managed block (sorted, de-duplicated).
pub fn managed_hosts(current: &str) -> Vec<String> {
    let mut set = BTreeSet::new();
    let mut inside = false;
    for line in current.lines() {
        let t = line.trim();
        if t == BEGIN_MARK {
            inside = true;
            continue;
        }
        if t == END_MARK {
            inside = false;
            continue;
        }
        if inside && !t.starts_with('#') {
            for name in t.split_whitespace().skip(1) {
                set.insert(name.to_ascii_lowercase());
            }
        }
    }
    set.into_iter().collect()
}

/// Return `current` with the managed block replaced by one listing exactly `hosts`
/// (block removed entirely when `hosts` is empty). Content outside the block is preserved.
pub fn render_hosts(current: &str, hosts: &[String]) -> String {
    let mut outside: Vec<&str> = Vec::new();
    let mut inside = false;
    for line in current.lines() {
        let t = line.trim();
        if t == BEGIN_MARK {
            inside = true;
            continue;
        }
        if t == END_MARK {
            inside = false;
            continue;
        }
        if !inside {
            outside.push(line);
        }
    }
    while outside.last().is_some_and(|l| l.trim().is_empty()) {
        outside.pop();
    }
    let mut out = outside.join("\n");
    let names: BTreeSet<String> = hosts
        .iter()
        .map(|h| h.trim().trim_end_matches('.').to_ascii_lowercase())
        .filter(|h| !h.is_empty())
        .collect();
    if !names.is_empty() {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(BEGIN_MARK);
        out.push('\n');
        for n in &names {
            out.push_str(&format!("127.0.0.1 {n}\n::1 {n}\n"));
        }
        out.push_str(END_MARK);
    }
    out.push('\n');
    out
}

/// `current` with `add` merged into the managed block.
pub fn add_hosts(current: &str, add: &[String]) -> String {
    let mut all = managed_hosts(current);
    all.extend(add.iter().map(|h| h.to_ascii_lowercase()));
    render_hosts(current, &all)
}

/// `current` with `remove` dropped from the managed block.
pub fn remove_hosts(current: &str, remove: &[String]) -> String {
    let rm: BTreeSet<String> = remove.iter().map(|h| h.to_ascii_lowercase()).collect();
    let keep: Vec<String> = managed_hosts(current)
        .into_iter()
        .filter(|h| !rm.contains(h))
        .collect();
    render_hosts(current, &keep)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "127.0.0.1 localhost\n::1 localhost\n";

    #[test]
    fn add_remove_roundtrip() {
        let a = add_hosts(BASE, &["Shop.test".into(), "api.shop.test".into()]);
        assert!(a.starts_with(BASE));
        assert!(a.contains(BEGIN_MARK));
        assert!(a.contains("127.0.0.1 shop.test\n::1 shop.test\n"));
        assert_eq!(managed_hosts(&a), vec!["api.shop.test", "shop.test"]);
        let b = add_hosts(&a, &["shop.test".into(), "blog.test".into()]);
        assert_eq!(
            managed_hosts(&b),
            vec!["api.shop.test", "blog.test", "shop.test"]
        );
        assert_eq!(b.matches(BEGIN_MARK).count(), 1);
        let c = remove_hosts(
            &b,
            &[
                "api.shop.test".into(),
                "blog.test".into(),
                "shop.test".into(),
            ],
        );
        assert_eq!(c, BASE);
    }

    #[test]
    fn preserves_content_after_block() {
        let s = format!("{BASE}\n{BEGIN_MARK}\n127.0.0.1 x.test\n{END_MARK}\n10.0.0.1 nas\n");
        let r = render_hosts(&s, &["y.test".into()]);
        assert!(r.contains("10.0.0.1 nas"));
        assert!(!r.contains("x.test"));
        assert!(r.contains("::1 y.test"));
    }
}
