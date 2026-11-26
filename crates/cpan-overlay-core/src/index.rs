//! Merge the overlay manifest into an upstream `02packages.details.txt`.
//!
//! The full CPAN index is ~271k rows / ~24 MB of text, and Fastly Compute
//! instances have a 128 MB heap. Rows therefore borrow `&str` slices of the
//! decompressed text (and of the manifest) instead of owning strings, the
//! sort does not allocate per-row keys, and output is streamed straight into
//! the gzip encoder. Peak memory is roughly the text plus 48 bytes per row.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{Read, Write};

use serde::Serialize;

use crate::manifest::{Distribution, Kind, Manifest, Policy};
use crate::{Error, distname, version};

/// Default ceiling for a decompressed upstream index (the real one is
/// ~25 MB today).
pub const DEFAULT_MAX_INDEX_BYTES: u64 = 128 * 1024 * 1024;

/// Decompress (if gzipped) an upstream index into text, refusing to
/// inflate beyond `max_bytes`.
pub fn decode_index(data: &[u8], max_bytes: u64) -> Result<String, Error> {
    let mut text = String::new();
    let result = if data.starts_with(&[0x1f, 0x8b]) {
        flate2::read::GzDecoder::new(data)
            .take(max_bytes + 1)
            .read_to_string(&mut text)
    } else {
        data.take(max_bytes + 1).read_to_string(&mut text)
    };
    result.map_err(|e| Error::Index(e.to_string()))?;
    if text.len() as u64 > max_bytes {
        return Err(Error::Index(format!(
            "decompressed index exceeds {max_bytes} bytes"
        )));
    }
    Ok(text)
}

/// One package row. `version` is the literal field, so `undef` stays `undef`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row<'a> {
    pub package: &'a str,
    pub version: &'a str,
    pub path: &'a str,
}

impl<'a> Row<'a> {
    pub fn version(&self) -> Option<&'a str> {
        (self.version != "undef").then_some(self.version)
    }
}

/// A parsed index borrowing its text.
#[derive(Debug, Clone)]
pub struct Index<'a> {
    /// Header fields in file order.
    pub header: Vec<(&'a str, String)>,
    /// Rows, sorted case-insensitively by package as PAUSE does.
    pub rows: Vec<Row<'a>>,
}

impl<'a> Index<'a> {
    pub fn parse(text: &'a str) -> Result<Self, Error> {
        let mut lines = text.lines().enumerate();
        let mut header = Vec::new();
        for (n, line) in &mut lines {
            if line.trim().is_empty() {
                break;
            }
            let (k, v) = line
                .split_once(':')
                .ok_or_else(|| Error::Index(format!("invalid header line {}: {line:?}", n + 1)))?;
            header.push((k.trim(), v.trim().to_string()));
        }
        let mut rows = Vec::with_capacity(
            header
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("Line-Count"))
                .and_then(|(_, v)| v.parse().ok())
                .unwrap_or(0usize)
                .min(1 << 20),
        );
        for (n, line) in lines {
            let mut f = line.split_ascii_whitespace();
            match (f.next(), f.next(), f.next(), f.next()) {
                (None, ..) => continue,
                (Some(package), Some(version), Some(path), None) => rows.push(Row {
                    package,
                    version,
                    path,
                }),
                _ => {
                    return Err(Error::Index(format!(
                        "invalid package line {}: {line:?}",
                        n + 1
                    )));
                }
            }
        }
        rows.sort_by(|a, b| package_order(a.package, b.package));
        if let Some(w) = rows.windows(2).find(|w| w[0].package == w[1].package) {
            return Err(Error::Index(format!("duplicate package {}", w[0].package)));
        }
        Ok(Index { header, rows })
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn header(&self, key: &str) -> Option<&str> {
        self.header
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }

    fn set_header(&mut self, key: &'static str, value: String) {
        match self
            .header
            .iter_mut()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
        {
            Some(slot) => slot.1 = value,
            None => self.header.push((key, value)),
        }
    }

    /// Binary search by package name.
    pub fn get(&self, package: &str) -> Option<&Row<'a>> {
        self.rows
            .binary_search_by(|r| package_order(r.package, package))
            .ok()
            .map(|i| &self.rows[i])
    }

    /// Write the plain-text file in PAUSE's layout.
    pub fn write_plain(&self, w: &mut impl Write) -> std::io::Result<()> {
        for (k, v) in &self.header {
            writeln!(w, "{:<13} {v}", format!("{k}:"))?;
        }
        writeln!(w)?;
        for r in &self.rows {
            // PAUSE: "%-30s %8s  %s", with the version field narrowed by
            // however far the package name overflows its 30 columns.
            let vw = 8usize.saturating_sub(r.package.len().saturating_sub(30));
            writeln!(w, "{:<30} {:>vw$}  {}", r.package, r.version, r.path)?;
        }
        Ok(())
    }

    pub fn to_plain_string(&self) -> String {
        let mut out = Vec::new();
        self.write_plain(&mut out)
            .expect("writing to a Vec cannot fail");
        String::from_utf8(out).expect("index text is UTF-8")
    }

    /// Gzip with a fixed (zero) mtime so output is deterministic.
    pub fn to_gz_bytes(&self) -> Result<Vec<u8>, Error> {
        let enc = flate2::GzBuilder::new()
            .mtime(0)
            .write(Vec::new(), flate2::Compression::default());
        let mut buf = std::io::BufWriter::with_capacity(64 * 1024, enc);
        self.write_plain(&mut buf)
            .and_then(|_| buf.into_inner().map_err(|e| e.into_error()))
            .and_then(|enc| enc.finish())
            .map_err(|e| Error::Index(e.to_string()))
    }
}

/// PAUSE's order: case-insensitive, ties broken bytewise. Allocation-free.
pub fn package_order(a: &str, b: &str) -> Ordering {
    a.bytes()
        .map(|c| c.to_ascii_lowercase())
        .cmp(b.bytes().map(|c| c.to_ascii_lowercase()))
        .then_with(|| a.cmp(b))
}

/// One difference between the merged index and the immediate upstream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "change", rename_all = "kebab-case")]
pub enum Change {
    Added {
        package: String,
        version: Option<String>,
        path: String,
    },
    Replaced {
        package: String,
        from_version: Option<String>,
        from_path: String,
        version: Option<String>,
        path: String,
    },
    Removed {
        package: String,
        from_version: Option<String>,
        from_path: String,
    },
}

impl Change {
    pub fn package(&self) -> &str {
        match self {
            Change::Added { package, .. }
            | Change::Replaced { package, .. }
            | Change::Removed { package, .. } => package,
        }
    }
}

/// How a configured distribution relates to the current upstream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DistributionStatus {
    pub name: String,
    pub active: bool,
    /// Why an override is inactive, when it is.
    pub reason: Option<String>,
    /// Upstream release path(s) currently indexed for this distribution.
    pub upstream_paths: Vec<String>,
    /// Whether upstream has released something newer than the patch base;
    /// `None` when that cannot be determined. Never evidence of a fix.
    pub newer_upstream: Option<bool>,
}

/// The effective overlay index.
#[derive(Debug, Clone)]
pub struct Merged<'a> {
    pub index: Index<'a>,
    pub changes: Vec<Change>,
    pub distributions: Vec<DistributionStatus>,
    /// Non-fatal findings worth showing an operator.
    pub warnings: Vec<String>,
}

impl Merged<'_> {
    pub fn to_gz_bytes(&self) -> Result<Vec<u8>, Error> {
        self.index.to_gz_bytes()
    }
}

#[derive(Default)]
struct UpstreamDist<'a> {
    packages: BTreeMap<&'a str, Row<'a>>,
    paths: BTreeSet<&'a str>,
}

/// Apply `manifest` to `upstream`. Ambiguities and unexplained omissions
/// fail the whole merge: a partially applied overlay is never produced.
pub fn merge<'a>(upstream: &Index<'a>, manifest: &'a Manifest) -> Result<Merged<'a>, Error> {
    let mut problems = Vec::new();

    // Upstream package sets of every distribution we patch, by dist name.
    let patched: BTreeSet<&str> = manifest
        .distributions
        .iter()
        .filter(|d| d.kind == Kind::Patched)
        .map(|d| d.name.as_str())
        .collect();
    let mut by_dist: HashMap<&str, UpstreamDist<'a>> = HashMap::new();
    if !patched.is_empty() {
        for r in &upstream.rows {
            if let Some(info) = distname::parse(r.path)
                && let Some(&name) = patched.get(info.name)
            {
                let d = by_dist.entry(name).or_default();
                d.packages.insert(r.package, *r);
                d.paths.insert(r.path);
            }
        }
    }

    // Decide which distributions are active, and what each one claims.
    let mut statuses = Vec::new();
    let mut claims: BTreeMap<&'a str, &'a Distribution> = BTreeMap::new();
    let mut removals: BTreeMap<&'a str, &'a Distribution> = BTreeMap::new();
    for d in &manifest.distributions {
        let up = by_dist.get(d.name.as_str());
        let (active, reason, newer) = decide(d, up, &mut problems);
        statuses.push(DistributionStatus {
            name: d.name.clone(),
            active,
            reason,
            upstream_paths: up
                .map(|u| u.paths.iter().map(|p| p.to_string()).collect())
                .unwrap_or_default(),
            newer_upstream: newer,
        });
        if !active {
            continue;
        }

        for pkg in d.provides.keys() {
            if let Some(other) = claims.insert(pkg, d) {
                problems.push(format!(
                    "package {pkg} is claimed by both {} and {}",
                    other.name, d.name
                ));
            }
            let owned_here = up.is_some_and(|u| u.packages.contains_key(pkg.as_str()));
            if let Some(r) = upstream.get(pkg)
                && !owned_here
            {
                let owner = distname::parse(r.path).map_or(r.path, |i| i.name);
                problems.push(match d.kind {
                    Kind::Personal => format!(
                        "{}: package {pkg} collides with upstream {}; declare a patched distribution to override it",
                        d.name, r.path
                    ),
                    Kind::Patched => format!(
                        "{}: package {pkg} belongs to unrelated upstream distribution {owner} ({})",
                        d.name, r.path
                    ),
                });
            }
        }

        if let Some(up) = up {
            for &pkg in up.packages.keys() {
                if d.provides.contains_key(pkg) {
                    continue;
                }
                match d.removes.iter().find(|r| *r == pkg) {
                    Some(r) => {
                        removals.insert(r, d);
                    }
                    None => problems.push(format!(
                        "{}: upstream package {pkg} is neither provided nor listed in removes",
                        d.name
                    )),
                }
            }
        }
    }
    for (pkg, d) in &removals {
        if let Some(other) = claims.get(pkg) {
            problems.push(format!(
                "package {pkg} is removed by {} but provided by {}",
                d.name, other.name
            ));
        }
    }
    if !problems.is_empty() {
        return Err(Error::Merge(problems));
    }

    // A replacement whose version does not sort above what it replaces is
    // allowed (some patches keep versions on purpose) but clients that
    // already have the upstream version installed will skip it.
    let mut warnings = Vec::new();
    for (pkg, d) in &claims {
        let (Some(new), Some(old)) = (
            d.provides[*pkg].as_deref(),
            upstream.get(pkg).and_then(Row::version),
        ) else {
            continue;
        };
        match version::compare(new, old) {
            Some(Ordering::Greater) => {}
            Some(_) => warnings.push(format!(
                "{}: {pkg} {new} is not newer than upstream {old} under Perl version rules; \
                 clients with {old} installed will not upgrade to the patched release",
                d.name
            )),
            None => warnings.push(format!(
                "{}: {pkg} version {new} cannot be compared with upstream {old}",
                d.name
            )),
        }
    }

    // Apply. Only rows we touch are copied into `changes`.
    let local_row = |pkg: &'a str, d: &'a Distribution| Row {
        package: pkg,
        version: d.provides[pkg].as_deref().unwrap_or("undef"),
        path: &d.path,
    };
    let mut changes = Vec::new();
    let mut rows: Vec<Row<'a>> = Vec::with_capacity(upstream.len() + claims.len());
    for r in &upstream.rows {
        if let Some((&pkg, &d)) = claims.get_key_value(r.package) {
            let new = local_row(pkg, d);
            changes.push(Change::Replaced {
                package: pkg.to_string(),
                from_version: r.version().map(str::to_string),
                from_path: r.path.to_string(),
                version: new.version().map(str::to_string),
                path: new.path.to_string(),
            });
            rows.push(new);
        } else if removals.contains_key(r.package) {
            changes.push(Change::Removed {
                package: r.package.to_string(),
                from_version: r.version().map(str::to_string),
                from_path: r.path.to_string(),
            });
        } else {
            rows.push(*r);
        }
    }
    for (&pkg, &d) in &claims {
        if upstream.get(pkg).is_none() {
            let new = local_row(pkg, d);
            changes.push(Change::Added {
                package: pkg.to_string(),
                version: new.version().map(str::to_string),
                path: new.path.to_string(),
            });
            rows.push(new);
        }
    }
    // Upstream rows are already sorted, so this is a near-linear merge of
    // the few added rows.
    rows.sort_by(|a, b| package_order(a.package, b.package));
    changes.sort_by(|a, b| package_order(a.package(), b.package()));

    let mut index = Index {
        header: upstream.header.clone(),
        rows,
    };
    index.set_header("Line-Count", index.rows.len().to_string());
    let written_by = match upstream.header("Written-By") {
        Some(w) => format!("CPAN Overlay {} (upstream: {w})", crate::VERSION),
        None => format!("CPAN Overlay {}", crate::VERSION),
    };
    index.set_header("Written-By", written_by);
    if let Some(url) = &manifest.site.public_url {
        index.set_header(
            "URL",
            format!(
                "{}/modules/02packages.details.txt",
                url.trim_end_matches('/')
            ),
        );
    }

    Ok(Merged {
        index,
        changes,
        distributions: statuses,
        warnings,
    })
}

/// Returns (active, inactive reason, newer upstream release?).
fn decide(
    d: &Distribution,
    up: Option<&UpstreamDist<'_>>,
    problems: &mut Vec<String>,
) -> (bool, Option<String>, Option<bool>) {
    let Some(base) = &d.upstream else {
        // Personal distribution.
        return (true, None, None);
    };
    let Some(up) = up else {
        return if d.effective_policy() == Policy::ExactUpstream {
            (
                false,
                Some(format!("upstream distribution {} is not indexed", d.name)),
                None,
            )
        } else {
            (true, None, Some(false))
        };
    };
    let paths = || up.paths.iter().copied().collect::<Vec<_>>().join(", ");

    // Compare every currently indexed upstream release against the base.
    let mut newer = Some(false);
    for path in &up.paths {
        match distname::parse(path).and_then(|i| version::compare(i.version, &base.version)) {
            Some(Ordering::Greater) => {
                if newer.is_some() {
                    newer = Some(true);
                }
            }
            Some(_) => {}
            None => newer = None,
        }
    }

    match d.effective_policy() {
        Policy::Force | Policy::Private => (true, None, newer),
        Policy::ExactUpstream => {
            if up.paths.len() == 1 && up.paths.contains(base.path.as_str()) {
                (true, None, newer)
            } else {
                (
                    false,
                    Some(format!(
                        "upstream is {} rather than the declared base {}",
                        paths(),
                        base.path
                    )),
                    newer,
                )
            }
        }
        Policy::UntilNewer => match newer {
            Some(true) => (
                false,
                Some(format!(
                    "upstream has a release newer than the patch base {}",
                    base.version
                )),
                newer,
            ),
            Some(false) => (true, None, newer),
            None => {
                problems.push(format!(
                    "{}: policy until-newer cannot compare upstream {} with base version {}",
                    d.name,
                    paths(),
                    base.version
                ));
                (true, None, None)
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/02packages.details.txt");

    fn manifest_with(edit: impl FnOnce(&mut serde_json::Value)) -> Manifest {
        try_manifest_with(edit).unwrap()
    }

    fn try_manifest_with(edit: impl FnOnce(&mut serde_json::Value)) -> Result<Manifest, Error> {
        let mut v: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/manifest.json")).unwrap();
        edit(&mut v);
        Manifest::from_json(&v.to_string())
    }

    type MergeParts = (String, Vec<Change>, Vec<DistributionStatus>, Vec<String>);

    fn merge_text(text: &str, m: &Manifest) -> Result<MergeParts, Error> {
        let up = Index::parse(text)?;
        let merged = merge(&up, m)?;
        Ok((
            merged.index.to_plain_string(),
            merged.changes,
            merged.distributions,
            merged.warnings,
        ))
    }

    #[test]
    fn layout_matches_pause() {
        // Round-tripping an unmodified PAUSE index reproduces it exactly.
        let up = Index::parse(FIXTURE).unwrap();
        assert_eq!(up.to_plain_string(), FIXTURE);
    }

    #[test]
    fn merges_patched_and_personal() {
        let m = manifest_with(|_| {});
        let up = Index::parse(FIXTURE).unwrap();
        let merged = merge(&up, &m).unwrap();
        let ix = &merged.index;
        assert_eq!(
            ix.get("Foo::Bar").unwrap().path,
            "L/LO/LOCAL/Foo-Bar-1.24.1.tar.gz"
        );
        assert_eq!(ix.get("Foo::Bar::Util").unwrap().version(), Some("1.24.1"));
        assert!(ix.get("Foo::Bar::Legacy").is_none());
        assert_eq!(
            ix.get("Personal::Tools").unwrap().path,
            "L/LO/LOCAL/Personal-Tools-0.01.tar.gz"
        );
        // Unrelated packages untouched, including the similarly named one.
        assert_eq!(
            ix.get("Foo::Barista").unwrap().path,
            "B/BA/BARISTA/Foo-Barista-2.0.tar.gz"
        );
        assert_eq!(ix.header("Line-Count"), Some(ix.len().to_string().as_str()));
        assert!(
            ix.header("Written-By")
                .unwrap()
                .starts_with("CPAN Overlay ")
        );
        assert_eq!(
            ix.header("URL"),
            Some("https://cpan.example.com/modules/02packages.details.txt")
        );
        // The fixture's 1.24.1 sorts *below* 1.24 under version.pm rules.
        assert_eq!(merged.warnings.len(), 2, "{:?}", merged.warnings);
        assert!(merged.warnings[0].contains("Foo::Bar 1.24.1 is not newer than upstream 1.24"));

        assert_eq!(
            merged.changes,
            vec![
                Change::Replaced {
                    package: "Foo::Bar".into(),
                    from_version: Some("1.24".into()),
                    from_path: "A/AU/AUTHOR/Foo-Bar-1.24.tar.gz".into(),
                    version: Some("1.24.1".into()),
                    path: "L/LO/LOCAL/Foo-Bar-1.24.1.tar.gz".into(),
                },
                Change::Removed {
                    package: "Foo::Bar::Legacy".into(),
                    from_version: None,
                    from_path: "A/AU/AUTHOR/Foo-Bar-1.24.tar.gz".into(),
                },
                Change::Replaced {
                    package: "Foo::Bar::Util".into(),
                    from_version: Some("1.24".into()),
                    from_path: "A/AU/AUTHOR/Foo-Bar-1.24.tar.gz".into(),
                    version: Some("1.24.1".into()),
                    path: "L/LO/LOCAL/Foo-Bar-1.24.1.tar.gz".into(),
                },
                Change::Added {
                    package: "Personal::Tools".into(),
                    version: Some("0.01".into()),
                    path: "L/LO/LOCAL/Personal-Tools-0.01.tar.gz".into(),
                },
            ]
        );
    }

    #[test]
    fn output_is_sorted_case_insensitively_and_deterministic() {
        let m = manifest_with(|_| {});
        let up = Index::parse(FIXTURE).unwrap();
        let a = merge(&up, &m).unwrap();
        let b = merge(&up, &m).unwrap();
        assert_eq!(a.to_gz_bytes().unwrap(), b.to_gz_bytes().unwrap());
        let pkgs: Vec<&str> = a.index.rows.iter().map(|r| r.package).collect();
        let mut sorted = pkgs.clone();
        sorted.sort_by(|x, y| package_order(x, y));
        assert_eq!(pkgs, sorted);
        // '_' sorts before 'a' once lower-cased, as on PAUSE.
        let third = pkgs.iter().position(|p| *p == "A_Third_Package").unwrap();
        let aaa = pkgs.iter().position(|p| *p == "AAA::Demo").unwrap();
        assert!(third < aaa);
    }

    #[test]
    fn unexplained_omission_fails() {
        let m = manifest_with(|v| v["distributions"][0]["removes"] = serde_json::json!([]));
        let err = merge_text(FIXTURE, &m).unwrap_err().to_string();
        assert!(
            err.contains("Foo::Bar::Legacy is neither provided nor listed in removes"),
            "{err}"
        );
    }

    #[test]
    fn personal_collision_fails() {
        let m = manifest_with(|v| {
            v["distributions"][1]["provides"]["Foo::Barista"] = "0.01".into();
        });
        let err = merge_text(FIXTURE, &m).unwrap_err().to_string();
        assert!(err.contains("collides with upstream"), "{err}");
    }

    #[test]
    fn patched_cannot_take_unrelated_package() {
        let m = manifest_with(|v| {
            v["distributions"][0]["provides"]["Foo::Barista"] = "1.24.1".into();
        });
        let err = merge_text(FIXTURE, &m).unwrap_err().to_string();
        assert!(
            err.contains("unrelated upstream distribution Foo-Barista"),
            "{err}"
        );
    }

    #[test]
    fn conflicting_claims_fail() {
        let m = manifest_with(|v| {
            v["distributions"][1]["provides"]["Foo::Bar"] = "0.01".into();
        });
        let err = merge_text(FIXTURE, &m).unwrap_err().to_string();
        assert!(
            err.contains("claimed by both Foo-Bar and Personal-Tools"),
            "{err}"
        );
    }

    fn with_upstream_version(v: &str) -> String {
        FIXTURE.replace("Foo-Bar-1.24.tar.gz", &format!("Foo-Bar-{v}.tar.gz"))
    }

    #[test]
    fn force_stays_active_when_upstream_is_newer() {
        let (_, _, statuses, _) =
            merge_text(&with_upstream_version("1.25"), &manifest_with(|_| {})).unwrap();
        assert!(statuses[0].active);
        assert_eq!(statuses[0].newer_upstream, Some(true));
    }

    #[test]
    fn until_newer_deactivates() {
        let m = manifest_with(|v| v["distributions"][0]["policy"] = "until-newer".into());
        let text = with_upstream_version("1.25");
        let up = Index::parse(&text).unwrap();
        let merged = merge(&up, &m).unwrap();
        assert!(!merged.distributions[0].active);
        assert_eq!(
            merged.index.get("Foo::Bar").unwrap().path,
            "A/AU/AUTHOR/Foo-Bar-1.25.tar.gz"
        );
        assert!(merged.index.get("Foo::Bar::Legacy").is_some());

        let (_, _, statuses, _) = merge_text(FIXTURE, &m).unwrap();
        assert!(statuses[0].active);
    }

    #[test]
    fn until_newer_with_incomparable_version_fails() {
        let m = manifest_with(|v| v["distributions"][0]["policy"] = "until-newer".into());
        let err = merge_text(&with_upstream_version("1.25-TRIAL"), &m)
            .unwrap_err()
            .to_string();
        assert!(err.contains("cannot compare"), "{err}");
    }

    #[test]
    fn exact_upstream() {
        let m = manifest_with(|v| v["distributions"][0]["policy"] = "exact-upstream".into());
        assert!(merge_text(FIXTURE, &m).unwrap().2[0].active);
        let (_, _, statuses, _) = merge_text(&with_upstream_version("1.23"), &m).unwrap();
        assert!(!statuses[0].active);
        assert!(
            statuses[0]
                .reason
                .as_deref()
                .unwrap()
                .contains("declared base")
        );
    }

    #[test]
    fn invalid_upstream() {
        let dup = format!("{FIXTURE}Foo::Bar 1.0 X/XX/XXX/Foo-Bar-1.0.tar.gz\n");
        assert!(
            Index::parse(&dup)
                .unwrap_err()
                .to_string()
                .contains("duplicate package Foo::Bar")
        );
        let bad = format!("{FIXTURE}Foo::Baz 1.0\n");
        assert!(
            Index::parse(&bad)
                .unwrap_err()
                .to_string()
                .contains("invalid package line")
        );
        assert!(try_manifest_with(|_| {}).is_ok());
    }

    #[test]
    fn decode_is_bounded() {
        let gz = Index::parse(FIXTURE).unwrap().to_gz_bytes().unwrap();
        assert_eq!(decode_index(&gz, DEFAULT_MAX_INDEX_BYTES).unwrap(), FIXTURE);
        let err = decode_index(&gz, 100).unwrap_err().to_string();
        assert!(err.contains("exceeds 100 bytes"), "{err}");
    }
}
