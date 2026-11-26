//! Classify an incoming request path.

use crate::manifest::{Distribution, Manifest};

#[derive(Debug, PartialEq, Eq)]
pub enum Route<'a> {
    /// The human-readable homepage.
    Home,
    /// The merged `02packages.details.txt[.gz]`.
    Index {
        compressed: bool,
    },
    /// `CHECKSUMS` for the overlay author directory.
    Checksums,
    /// A custom artifact in the overlay namespace.
    Artifact(&'a Distribution),
    /// Delegate to the upstream mirror unchanged (path keeps its leading `/`).
    Upstream(&'a str),
    /// Reserved local path with nothing at it (never delegated upstream).
    NotFound,
    MethodNotAllowed,
    BadRequest,
}

/// Route a request. `path` is the URL path without query string.
pub fn route<'a>(manifest: &'a Manifest, method: &str, path: &'a str) -> Route<'a> {
    if !is_safe_path(path) {
        return Route::BadRequest;
    }
    if method != "GET" && method != "HEAD" {
        return Route::MethodNotAllowed;
    }

    match path {
        "/" => return Route::Home,
        "/modules/02packages.details.txt.gz" => return Route::Index { compressed: true },
        "/modules/02packages.details.txt" => return Route::Index { compressed: false },
        _ => {}
    }
    if path == "/api" || path.starts_with("/api/") {
        return Route::NotFound;
    }

    let ns_prefix = format!("/authors/id/{}/", manifest.namespace_dir());
    if let Some(file) = path.strip_prefix(&ns_prefix) {
        if file == "CHECKSUMS" {
            return Route::Checksums;
        }
        let rel = &path["/authors/id/".len()..];
        return match manifest.distribution_by_path(rel) {
            Some(d) => Route::Artifact(d),
            None => Route::NotFound,
        };
    }
    if path.trim_end_matches('/') == ns_prefix.trim_end_matches('/') {
        return Route::NotFound;
    }

    Route::Upstream(path)
}

/// Absolute, normalised, printable ASCII path with no traversal, empty
/// segments, backslashes or percent-encoded separators/dots.
fn is_safe_path(path: &str) -> bool {
    if !path.starts_with('/') || path.len() > 1024 {
        return false;
    }
    if !path.bytes().all(|b| b.is_ascii_graphic()) || path.contains('\\') {
        return false;
    }
    let lower = path.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "%00"]
        .iter()
        .any(|s| lower.contains(s))
    {
        return false;
    }
    let inner = path[1..].strip_suffix('/').unwrap_or(&path[1..]);
    inner.is_empty()
        || inner
            .split('/')
            .all(|s| !s.is_empty() && s != "." && s != "..")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m() -> Manifest {
        Manifest::from_json(include_str!("../tests/fixtures/manifest.json")).unwrap()
    }

    #[test]
    fn routes() {
        let m = m();
        assert_eq!(route(&m, "GET", "/"), Route::Home);
        assert_eq!(
            route(&m, "HEAD", "/modules/02packages.details.txt.gz"),
            Route::Index { compressed: true }
        );
        assert_eq!(
            route(&m, "GET", "/modules/02packages.details.txt"),
            Route::Index { compressed: false }
        );
        assert_eq!(
            route(&m, "GET", "/authors/id/L/LO/LOCAL/CHECKSUMS"),
            Route::Checksums
        );
        assert!(matches!(
            route(&m, "GET", "/authors/id/L/LO/LOCAL/Personal-Tools-0.01.tar.gz"),
            Route::Artifact(d) if d.name == "Personal-Tools"
        ));
        assert_eq!(
            route(&m, "GET", "/authors/id/L/LO/LOCAL/Nope-1.0.tar.gz"),
            Route::NotFound
        );
        assert_eq!(route(&m, "GET", "/authors/id/L/LO/LOCAL/"), Route::NotFound);
        assert_eq!(route(&m, "GET", "/authors/id/L/LO/LOCAL"), Route::NotFound);
        assert_eq!(route(&m, "GET", "/api/v1/status"), Route::NotFound);
        assert_eq!(
            route(&m, "GET", "/authors/id/A/AU/AUTHOR/Foo-Bar-1.24.tar.gz"),
            Route::Upstream("/authors/id/A/AU/AUTHOR/Foo-Bar-1.24.tar.gz")
        );
        assert_eq!(
            route(&m, "GET", "/modules/03modlist.data.gz"),
            Route::Upstream("/modules/03modlist.data.gz")
        );
    }

    #[test]
    fn rejects() {
        let m = m();
        assert_eq!(route(&m, "POST", "/"), Route::MethodNotAllowed);
        for bad in [
            "",
            "authors",
            "/authors/../etc/passwd",
            "/authors//id",
            "/authors/./id",
            "/authors/%2e%2e/x",
            "/authors/id\\x",
            "/a b",
        ] {
            assert_eq!(route(&m, "GET", bad), Route::BadRequest, "{bad:?}");
        }
    }
}
