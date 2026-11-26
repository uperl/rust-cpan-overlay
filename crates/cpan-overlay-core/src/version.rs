//! Perl version comparison, following the `version.pm` model.
//!
//! * Decimal versions (`1.24`, `0.001_002`) split the fraction into groups
//!   of three digits: `1.24` is `1.240`, i.e. `v1.240.0`.
//! * Dotted versions (`v1.2.3`, or any version with two or more dots) are
//!   compared component-wise.
//! * Underscores (developer releases) are ignored for ordering.
//!
//! Anything else (including `undef`) is not comparable and yields `None`;
//! callers must decide explicitly what that means.

use std::cmp::Ordering;

/// Normalise a version to its numeric components.
pub fn parse(raw: &str) -> Option<Vec<u64>> {
    let s = raw.trim();
    if s.is_empty() || s == "undef" {
        return None;
    }
    let dotted = s.starts_with('v') || s.matches('.').count() >= 2;
    let s = s.strip_prefix('v').unwrap_or(s).replace('_', "");
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return None;
    }
    if dotted {
        s.split('.').map(|c| c.parse().ok()).collect()
    } else {
        let (int, frac) = s.split_once('.').unwrap_or((&s, ""));
        if int.is_empty() && frac.is_empty() {
            return None;
        }
        let mut out = vec![if int.is_empty() { 0 } else { int.parse().ok()? }];
        let mut frac = frac.to_string();
        while frac.len() % 3 != 0 {
            frac.push('0');
        }
        for chunk in frac.as_bytes().chunks(3) {
            out.push(std::str::from_utf8(chunk).ok()?.parse().ok()?);
        }
        Some(out)
    }
}

/// Compare two Perl versions; `None` when either is not comparable.
pub fn compare(a: &str, b: &str) -> Option<Ordering> {
    let (a, b) = (parse(a)?, parse(b)?);
    let n = a.len().max(b.len());
    Some(
        (0..n)
            .map(|i| {
                a.get(i)
                    .copied()
                    .unwrap_or(0)
                    .cmp(&b.get(i).copied().unwrap_or(0))
            })
            .find(|o| o.is_ne())
            .unwrap_or(Ordering::Equal),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use Ordering::*;

    #[test]
    fn decimal() {
        assert_eq!(parse("1.24"), Some(vec![1, 240]));
        assert_eq!(compare("1.24", "1.3"), Some(Less));
        assert_eq!(compare("1.10", "1.1"), Some(Equal));
        assert_eq!(compare("1.27", "1.27"), Some(Equal));
        assert_eq!(compare("0.001002", "0.001_002"), Some(Equal));
        assert_eq!(compare("2", "1.999"), Some(Greater));
    }

    #[test]
    fn dotted() {
        // version.pm: 1.24 is v1.240.0, so 1.24.1 is *older* than 1.24.
        assert_eq!(compare("1.24.1", "1.24"), Some(Less));
        assert_eq!(compare("1.240.1", "1.24"), Some(Greater));
        assert_eq!(compare("v1.2.3", "1.2.3"), Some(Equal));
        assert_eq!(compare("v1.2", "1.002"), Some(Equal));
        assert_eq!(compare("1.24_01", "1.24"), Some(Greater));
        assert_eq!(compare("1.2.10", "1.2.9"), Some(Greater));
    }

    #[test]
    fn incomparable() {
        assert_eq!(compare("undef", "1.0"), None);
        assert_eq!(compare("1.0-TRIAL", "1.0"), None);
        assert_eq!(compare("", "1.0"), None);
        assert_eq!(compare(".", "1.0"), None);
    }
}
