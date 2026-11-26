//! The overlay manifest: a versioned, validated description of one overlay
//! instance (site identity, backends, upstream and custom distributions).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{Error, distname};

/// The only manifest schema version this build understands.
pub const SCHEMA_VERSION: u32 = 1;

/// The default overlay author namespace (`L/LO/LOCAL`), reserved for local use.
pub const DEFAULT_NAMESPACE: &str = "LOCAL";

/// A parsed and validated overlay manifest.
///
/// Construct one with [`Manifest::from_json`], which rejects anything that
/// does not pass [`Manifest::validate`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema_version: u32,
    #[serde(default)]
    pub site: Site,
    #[serde(default)]
    pub authentication: Authentication,
    /// PAUSE-style author ID that owns every custom artifact path.
    #[serde(default = "default_namespace")]
    pub namespace: String,
    pub upstream: Upstream,
    pub backends: BTreeMap<String, Backend>,
    #[serde(default)]
    pub distributions: Vec<Distribution>,
}

fn default_namespace() -> String {
    DEFAULT_NAMESPACE.to_string()
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Site {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Canonical public URL, used for generated instructions instead of the
    /// request `Host` header.
    #[serde(default)]
    pub public_url: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Authentication {
    #[serde(default)]
    pub mode: AuthMode,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthMode {
    #[default]
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Upstream {
    /// Name of an `http` backend serving a CPAN mirror (or another overlay).
    pub backend: String,
    /// How long a merged index stays fresh before the upstream is re-checked.
    #[serde(default = "default_refresh_seconds")]
    pub refresh_seconds: u32,
}

fn default_refresh_seconds() -> u32 {
    300
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Backend {
    Http {
        base_url: String,
        #[serde(default)]
        delivery: Delivery,
    },
    Kv {
        store: String,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Delivery {
    #[default]
    Proxy,
    Redirect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// A distribution that does not exist upstream.
    Personal,
    /// A patched replacement for an upstream distribution.
    Patched,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Policy {
    Private,
    Force,
    UntilNewer,
    ExactUpstream,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Distribution {
    pub name: String,
    pub kind: Kind,
    pub version: String,
    /// Artifact path relative to `authors/id/`, inside the overlay namespace.
    pub path: String,
    pub backend: String,
    /// Backend-relative object name (URL path suffix or KV key).
    pub object: String,
    /// Package name to version (`null` for `undef`).
    pub provides: BTreeMap<String, Option<String>>,
    /// Upstream packages of the replaced distribution that the patched
    /// release deliberately drops from the index.
    #[serde(default)]
    pub removes: Vec<String>,
    #[serde(default)]
    pub policy: Option<Policy>,
    #[serde(default)]
    pub upstream: Option<UpstreamBase>,
    /// Lowercase hex SHA-256 of the artifact bytes.
    pub sha256: String,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub md5: Option<String>,
    /// Artifact date as `YYYY-MM-DD`, as used in `CHECKSUMS`.
    #[serde(default)]
    pub mtime: Option<String>,
    #[serde(default)]
    pub patches: Vec<Patch>,
    #[serde(default)]
    pub source: Option<Source>,
}

impl Distribution {
    /// The policy in force, applying the per-kind default.
    pub fn effective_policy(&self) -> Policy {
        self.policy.unwrap_or(match self.kind {
            Kind::Personal => Policy::Private,
            Kind::Patched => Policy::Force,
        })
    }

    /// The artifact file name (last path component).
    pub fn file_name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpstreamBase {
    pub path: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Patch {
    pub summary: String,
    #[serde(default)]
    pub issue_url: Option<String>,
    #[serde(default)]
    pub pull_request_url: Option<String>,
    #[serde(default)]
    pub patch_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub revision: Option<String>,
}

impl Manifest {
    /// Parse and validate a manifest from JSON.
    pub fn from_json(text: &str) -> Result<Self, Error> {
        // Check the schema version first so an unknown version reports that,
        // rather than whatever field it no longer recognises.
        let raw: serde_json::Value =
            serde_json::from_str(text).map_err(|e| Error::Manifest(vec![e.to_string()]))?;
        match raw.get("schema_version").and_then(|v| v.as_u64()) {
            Some(v) if v == u64::from(SCHEMA_VERSION) => {}
            Some(v) => {
                return Err(Error::Manifest(vec![format!(
                    "unsupported schema_version {v} (this build supports {SCHEMA_VERSION})"
                )]));
            }
            None => {
                return Err(Error::Manifest(vec![
                    "missing or non-integer schema_version".into(),
                ]));
            }
        }
        let manifest: Manifest =
            serde_json::from_value(raw).map_err(|e| Error::Manifest(vec![e.to_string()]))?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// The author directory for custom artifacts, e.g. `L/LO/LOCAL`.
    pub fn namespace_dir(&self) -> String {
        author_dir(&self.namespace)
    }

    pub fn distribution_by_path(&self, path: &str) -> Option<&Distribution> {
        self.distributions.iter().find(|d| d.path == path)
    }

    /// Check every structural rule that does not need the upstream index.
    /// All problems are reported together.
    pub fn validate(&self) -> Result<(), Error> {
        let mut p = Vec::new();

        if self.schema_version != SCHEMA_VERSION {
            p.push(format!(
                "unsupported schema_version {}",
                self.schema_version
            ));
        }
        if !valid_author_id(&self.namespace) {
            p.push(format!(
                "namespace {:?} is not a valid PAUSE-style author ID",
                self.namespace
            ));
        }
        if let Some(url) = &self.site.public_url
            && !valid_http_url(url)
        {
            p.push(format!("site.public_url {url:?} is not an http(s) URL"));
        }

        for (name, backend) in &self.backends {
            if !valid_identifier(name) {
                p.push(format!("backend name {name:?} is invalid"));
            }
            match backend {
                Backend::Http { base_url, .. } => {
                    if !valid_http_url(base_url) {
                        p.push(format!(
                            "backend {name}: base_url {base_url:?} must be an http(s) URL without query or fragment"
                        ));
                    }
                }
                Backend::Kv { store } => {
                    if !valid_identifier(store) {
                        p.push(format!("backend {name}: store name {store:?} is invalid"));
                    }
                }
            }
        }

        match self.backends.get(&self.upstream.backend) {
            Some(Backend::Http { .. }) => {}
            Some(_) => p.push(format!(
                "upstream backend {:?} must be an http backend",
                self.upstream.backend
            )),
            None => p.push(format!(
                "upstream backend {:?} is not defined",
                self.upstream.backend
            )),
        }

        let ns_dir = self.namespace_dir();
        let mut names = BTreeSet::new();
        let mut paths = BTreeSet::new();
        for d in &self.distributions {
            let who = format!("distribution {}", d.name);
            if !names.insert(d.name.as_str()) {
                p.push(format!("{who}: duplicate distribution name"));
            }
            if !valid_dist_name(&d.name) {
                p.push(format!("{who}: invalid distribution name"));
            }
            if d.version.is_empty() || d.version.chars().any(char::is_whitespace) {
                p.push(format!("{who}: invalid version {:?}", d.version));
            }

            match d
                .path
                .strip_prefix(&ns_dir)
                .and_then(|r| r.strip_prefix('/'))
            {
                Some(file) if valid_file_name(file) && file != "CHECKSUMS" => {
                    match distname::parse(file) {
                        Some(info) if info.name == d.name => {}
                        _ => p.push(format!(
                            "{who}: file name {file:?} does not look like a release of {}",
                            d.name
                        )),
                    }
                }
                _ => p.push(format!(
                    "{who}: path {:?} must be a plain archive file name directly under {ns_dir}/",
                    d.path
                )),
            }
            if !paths.insert(d.path.as_str()) {
                p.push(format!("{who}: path {:?} is used more than once", d.path));
            }

            if !self.backends.contains_key(&d.backend) {
                p.push(format!("{who}: backend {:?} is not defined", d.backend));
            }
            if !valid_object(&d.object) {
                p.push(format!(
                    "{who}: object {:?} is not a safe object name",
                    d.object
                ));
            }

            if d.provides.is_empty() {
                p.push(format!("{who}: provides must list at least one package"));
            }
            for (pkg, ver) in &d.provides {
                if !valid_package(pkg) {
                    p.push(format!("{who}: invalid package name {pkg:?}"));
                }
                if let Some(v) = ver
                    && (v.is_empty() || v == "undef" || v.chars().any(char::is_whitespace))
                {
                    p.push(format!(
                        "{who}: package {pkg} has invalid version {v:?} (use null for undef)"
                    ));
                }
            }
            for pkg in &d.removes {
                if !valid_package(pkg) {
                    p.push(format!("{who}: invalid package name {pkg:?} in removes"));
                }
                if d.provides.contains_key(pkg) {
                    p.push(format!("{who}: package {pkg} is both provided and removed"));
                }
            }

            if !(d.sha256.len() == 64
                && d.sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
            {
                p.push(format!("{who}: sha256 must be 64 lowercase hex digits"));
            }
            if let Some(md5) = &d.md5
                && !(md5.len() == 32
                    && md5
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
            {
                p.push(format!("{who}: md5 must be 32 lowercase hex digits"));
            }
            if let Some(mtime) = &d.mtime
                && !valid_date(mtime)
            {
                p.push(format!("{who}: mtime {mtime:?} must be YYYY-MM-DD"));
            }

            let policy = d.effective_policy();
            match d.kind {
                Kind::Personal => {
                    if policy != Policy::Private {
                        p.push(format!(
                            "{who}: personal distributions must use policy private"
                        ));
                    }
                    if d.upstream.is_some() {
                        p.push(format!(
                            "{who}: personal distributions have no upstream base"
                        ));
                    }
                    if !d.removes.is_empty() {
                        p.push(format!(
                            "{who}: personal distributions cannot remove packages"
                        ));
                    }
                }
                Kind::Patched => {
                    if policy == Policy::Private {
                        p.push(format!(
                            "{who}: patched distributions cannot use policy private"
                        ));
                    }
                    match &d.upstream {
                        None => p.push(format!(
                            "{who}: patched distributions need an upstream base"
                        )),
                        Some(base) => {
                            if !valid_author_path(&base.path) {
                                p.push(format!(
                                    "{who}: upstream path {:?} is not a valid author path",
                                    base.path
                                ));
                            } else if base.path.starts_with(&format!("{ns_dir}/")) {
                                p.push(format!(
                                    "{who}: upstream path must not be inside the overlay namespace"
                                ));
                            }
                        }
                    }
                }
            }
        }

        if p.is_empty() {
            Ok(())
        } else {
            Err(Error::Manifest(p))
        }
    }
}

/// `LOCAL` -> `L/LO/LOCAL`.
pub fn author_dir(id: &str) -> String {
    let first: String = id.chars().take(1).collect();
    let two: String = id.chars().take(2).collect();
    format!("{first}/{two}/{id}")
}

fn valid_author_id(id: &str) -> bool {
    id.len() >= 2
        && id.as_bytes()[0].is_ascii_uppercase()
        && id
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-')
}

fn valid_identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 255
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn valid_dist_name(s: &str) -> bool {
    !s.is_empty()
        && s.as_bytes()[0].is_ascii_alphanumeric()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'+'))
}

/// A single path component made of characters safe in CPAN archive names.
pub(crate) fn valid_file_name(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && s.as_bytes()[0] != b'.'
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'+'))
}

/// `A/AU/AUTHOR/[subdirs/]File.tar.gz`.
fn valid_author_path(s: &str) -> bool {
    let parts: Vec<&str> = s.split('/').collect();
    if parts.len() < 4 {
        return false;
    }
    let id = parts[2];
    valid_author_id(id)
        && author_dir(id) == parts[..3].join("/")
        && parts[3..].iter().all(|c| valid_file_name(c))
}

fn valid_object(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('/')
        && s.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
        && s.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'+' | b'/' | b'~')
        })
}

pub(crate) fn valid_package(s: &str) -> bool {
    !s.is_empty()
        && s.split("::").all(|seg| {
            !seg.is_empty() && seg.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        })
        && !s.as_bytes()[0].is_ascii_digit()
}

fn valid_http_url(s: &str) -> bool {
    let rest = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"));
    match rest {
        Some(rest) => {
            let host = rest.split('/').next().unwrap_or("");
            !host.is_empty()
                && !host.contains('@')
                && !s.contains(['?', '#', ' ', '\\'])
                && s.bytes().all(|b| b.is_ascii_graphic())
        }
        None => false,
    }
}

fn valid_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn sample() -> &'static str {
        include_str!("../tests/fixtures/manifest.json")
    }

    #[test]
    fn sample_manifest_is_valid() {
        let m = Manifest::from_json(sample()).unwrap();
        assert_eq!(m.namespace_dir(), "L/LO/LOCAL");
        assert_eq!(m.distributions.len(), 2);
        assert_eq!(m.distributions[0].effective_policy(), Policy::Force);
        assert_eq!(m.distributions[1].effective_policy(), Policy::Private);
    }

    #[test]
    fn unknown_schema_version_is_reported_clearly() {
        let err = Manifest::from_json(r#"{"schema_version": 2, "whatever": 1}"#).unwrap_err();
        assert!(
            err.to_string().contains("unsupported schema_version 2"),
            "{err}"
        );
    }

    #[test]
    fn collects_all_problems() {
        let mut v: serde_json::Value = serde_json::from_str(sample()).unwrap();
        v["namespace"] = "local".into();
        v["upstream"]["backend"] = "nope".into();
        v["distributions"][0]["object"] = "../etc/passwd".into();
        v["distributions"][1]["sha256"] = "XYZ".into();
        let Error::Manifest(problems) = Manifest::from_json(&v.to_string()).unwrap_err() else {
            panic!("expected manifest error");
        };
        let all = problems.join("\n");
        for needle in ["namespace", "upstream backend", "safe object", "sha256"] {
            assert!(all.contains(needle), "missing {needle:?} in:\n{all}");
        }
    }

    #[test]
    fn path_must_be_in_namespace() {
        let mut v: serde_json::Value = serde_json::from_str(sample()).unwrap();
        v["distributions"][1]["path"] = "P/PL/PLICEASE/Personal-Tools-0.01.tar.gz".into();
        let err = Manifest::from_json(&v.to_string()).unwrap_err().to_string();
        assert!(err.contains("directly under L/LO/LOCAL/"), "{err}");
    }

    #[test]
    fn package_names() {
        assert!(valid_package("Foo::Bar_Baz"));
        assert!(valid_package("A_Third_Package"));
        assert!(!valid_package("Foo::"));
        assert!(!valid_package("Foo:Bar"));
        assert!(!valid_package("1Foo"));
    }

    #[test]
    fn author_paths() {
        assert!(valid_author_path("P/PL/PLICEASE/File-Which-1.27.tar.gz"));
        assert!(valid_author_path(
            "P/PL/PLICEASE/sub/File-Which-1.27.tar.gz"
        ));
        assert!(!valid_author_path("P/PX/PLICEASE/File-Which-1.27.tar.gz"));
        assert!(!valid_author_path("P/PL/PLICEASE/../x.tar.gz"));
    }
}
