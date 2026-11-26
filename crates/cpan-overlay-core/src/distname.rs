//! Split CPAN archive file names into distribution name and version.
//!
//! This is a deliberately conservative subset of `CPAN::DistnameInfo`: it
//! handles the conventional `Name-Version.ext` layout and returns `None`
//! for anything else rather than guessing.

const EXTENSIONS: &[&str] = &[".tar.gz", ".tgz", ".tar.bz2", ".tar.xz", ".tar", ".zip"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DistInfo<'a> {
    pub name: &'a str,
    pub version: &'a str,
}

/// Parse a file name (or a full `A/AU/AUTHOR/...` path; only the last
/// component is used).
pub fn parse(path: &str) -> Option<DistInfo<'_>> {
    let file = path.rsplit('/').next()?;
    let stem = EXTENSIONS.iter().find_map(|ext| {
        file.len()
            .checked_sub(ext.len())
            .filter(|&n| file[n..].eq_ignore_ascii_case(ext))
            .map(|n| &file[..n])
    })?;
    // The version starts after the last '-' that is followed by a digit or
    // by 'v' and a digit.
    let bytes = stem.as_bytes();
    let split = (0..bytes.len()).rev().find(|&i| {
        bytes[i] == b'-'
            && match bytes.get(i + 1) {
                Some(b) if b.is_ascii_digit() => true,
                Some(b'v') => bytes.get(i + 2).is_some_and(u8::is_ascii_digit),
                _ => false,
            }
    })?;
    let (name, version) = (&stem[..split], &stem[split + 1..]);
    (!name.is_empty()).then_some(DistInfo { name, version })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Option<(&str, &str)> {
        parse(s).map(|d| (d.name, d.version))
    }

    #[test]
    fn conventional_names() {
        assert_eq!(
            p("P/PL/PLICEASE/File-Which-1.27.tar.gz"),
            Some(("File-Which", "1.27"))
        );
        assert_eq!(p("Foo-Bar-1.24.1.tar.gz"), Some(("Foo-Bar", "1.24.1")));
        assert_eq!(p("Foo-v1.2.3.tgz"), Some(("Foo", "v1.2.3")));
        assert_eq!(p("Foo-Bar-0.01_01.zip"), Some(("Foo-Bar", "0.01_01")));
        assert_eq!(
            p("Net-IPv4Addr-0.10.tar.bz2"),
            Some(("Net-IPv4Addr", "0.10"))
        );
        assert_eq!(
            p("A1z-HTML5-Template-0.22.tar.gz"),
            Some(("A1z-HTML5-Template", "0.22"))
        );
    }

    #[test]
    fn unconventional_names_are_rejected() {
        assert_eq!(p("Foo.tar.gz"), None);
        assert_eq!(p("Foo-Bar.tar.gz"), None);
        assert_eq!(p("Foo-1.0.rpm"), None);
        assert_eq!(p("-1.0.tar.gz"), None);
    }
}
