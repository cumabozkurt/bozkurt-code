//! Lightweight version handling: numeric comparison, prefix matching (`22` matches
//! `22.11.0`) and extracting a usable version prefix from constraints such as
//! composer's `^8.2 || ^8.3`, `.nvmrc` `lts/*`/`v20.11.1`, or engines `>=18`.

use std::cmp::Ordering;

/// Parse the numeric components of a version (`v22.1.0` → `[22,1,0]`, `3.13.1+2025` →
/// `[3,13,1]`, `go1.23.2` → `[1,23,2]`). Pre-release suffixes are ignored for ordering.
pub fn parse(v: &str) -> Vec<u64> {
    let v = v.trim();
    let start = v.find(|c: char| c.is_ascii_digit()).unwrap_or(v.len());
    let v = &v[start..];
    let core = v.split(['+', '-', ' ']).next().unwrap_or("");
    let mut out = vec![];
    for part in core.split('.') {
        let digits: String = part.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            break;
        }
        out.push(digits.parse().unwrap_or(0));
        if digits.len() != part.len() {
            break; // e.g. `1.24rc1`
        }
    }
    out
}

fn compare_vec(a: &[u64], b: &[u64]) -> Ordering {
    let n = a.len().max(b.len());
    for i in 0..n {
        match a.get(i).unwrap_or(&0).cmp(b.get(i).unwrap_or(&0)) {
            Ordering::Equal => continue,
            o => return o,
        }
    }
    Ordering::Equal
}

/// Compare two versions numerically (missing components count as 0).
pub fn compare(a: &str, b: &str) -> Ordering {
    compare_vec(&parse(a), &parse(b))
}

/// Sort descending (newest first).
pub fn sort_desc(v: &mut [String]) {
    v.sort_by(|a, b| compare(b, a));
}

/// Does `version` match the requested `prefix` component-wise (`8.3` ⇒ `8.3.12`, not `8.30.0`)?
pub fn matches_prefix(version: &str, prefix: &str) -> bool {
    let (v, p) = (parse(version), parse(prefix));
    !p.is_empty() && p.len() <= v.len() && v[..p.len()] == p[..]
}

/// The newest of `candidates` matching `want` (exact or prefix).
pub fn best_match<'a, I>(candidates: I, want: &str) -> Option<String>
where
    I: IntoIterator<Item = &'a String>,
{
    let want = want.trim().trim_start_matches('v');
    let mut m: Vec<String> = candidates
        .into_iter()
        .filter(|c| c.as_str() == want || matches_prefix(c, want))
        .cloned()
        .collect();
    sort_desc(&mut m);
    m.into_iter().next()
}

/// Reduce a constraint to a version prefix we can resolve against installed versions.
///
/// * `8.3`, `v20.11.1`, `20` → kept (`v` stripped)
/// * `^8.2`, `~8.2.0`, `>=8.1 <9`, `^8.1 || ^8.2` → the highest lower bound's
///   `major.minor` among `||` alternatives (`8.2`)
/// * `lts/*`, `node`, `*`, `latest` → `None` (caller picks newest)
pub fn constraint_prefix(c: &str) -> Option<String> {
    let c = c.trim();
    if c.is_empty() || c == "*" || c.starts_with("lts") || c == "node" || c == "latest" {
        return None;
    }
    let mut best: Option<Vec<u64>> = None;
    for alt in c.split("||") {
        let alt = alt.trim();
        // first comparator of the alternative carries the lower bound
        let first = alt.split([' ', ',']).find(|s| !s.is_empty()).unwrap_or("");
        if first.starts_with('<') {
            continue; // upper bound only
        }
        let p = parse(first);
        if p.is_empty() {
            continue;
        }
        let exact = first
            .trim_start_matches(['v', '='])
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit())
            && !alt.contains(['^', '~', '>', '<', ' ', '*', 'x']);
        let p = if exact {
            p
        } else {
            p.into_iter().take(2).collect()
        };
        if best
            .as_ref()
            .is_none_or(|b| compare_vec(&p, b) == Ordering::Greater)
        {
            best = Some(p);
        }
    }
    best.map(|v| {
        v.iter()
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(".")
    })
}

/// `8.3.12` → `8.3`.
pub fn major_minor(v: &str) -> String {
    parse(v)
        .iter()
        .take(2)
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join(".")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsing_and_ordering() {
        assert_eq!(parse("v22.1.0"), vec![22, 1, 0]);
        assert_eq!(parse("3.13.16+20261003"), vec![3, 13, 16]);
        assert_eq!(parse("go1.23.2"), vec![1, 23, 2]);
        assert_eq!(parse("1.24rc1"), vec![1, 24]);
        assert_eq!(compare("8.10.0", "8.9.9"), Ordering::Greater);
        assert_eq!(compare("8.3", "8.3.0"), Ordering::Equal);
        let mut v = vec!["1.2.0".to_string(), "1.10.0".into(), "1.9.3".into()];
        sort_desc(&mut v);
        assert_eq!(v, vec!["1.10.0", "1.9.3", "1.2.0"]);
    }

    #[test]
    fn prefix() {
        assert!(matches_prefix("8.3.12", "8.3"));
        assert!(!matches_prefix("8.30.0", "8.3"));
        assert!(matches_prefix("22.11.0", "22"));
        let c = vec!["22.1.0".to_string(), "22.11.0".into(), "20.0.0".into()];
        assert_eq!(best_match(&c, "22").as_deref(), Some("22.11.0"));
        assert_eq!(best_match(&c, "v20.0.0").as_deref(), Some("20.0.0"));
        assert_eq!(best_match(&c, "18"), None);
    }

    #[test]
    fn constraints() {
        assert_eq!(constraint_prefix("^8.2").as_deref(), Some("8.2"));
        assert_eq!(constraint_prefix("^8.1 || ^8.2").as_deref(), Some("8.2"));
        assert_eq!(constraint_prefix(">=8.1 <9.0").as_deref(), Some("8.1"));
        assert_eq!(constraint_prefix("~8.2.0").as_deref(), Some("8.2"));
        assert_eq!(constraint_prefix("v20.11.1").as_deref(), Some("20.11.1"));
        assert_eq!(constraint_prefix("20").as_deref(), Some("20"));
        assert_eq!(constraint_prefix(">=18").as_deref(), Some("18"));
        assert_eq!(constraint_prefix("8.2.*").as_deref(), Some("8.2"));
        assert_eq!(constraint_prefix("lts/*"), None);
        assert_eq!(constraint_prefix("*"), None);
        assert_eq!(major_minor("8.3.12"), "8.3");
    }
}
