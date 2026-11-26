//! Fastly Compute adapter: configuration from Config Store, artifacts from
//! HTTP backends or KV, merged index cached in the Fastly cache.

use std::io::{Read, Write};
use std::time::Duration;

use cpan_overlay_core::manifest::{Backend, Delivery, Distribution, Manifest};
use cpan_overlay_core::{Route, checksums, home, index, sha256_hex};
use fastly::cache::{core as cache_core, simple as cache};
use fastly::http::{Method, StatusCode, header};
use fastly::{Body, ConfigStore, KVStore, Request, Response};

/// Config Store holding the manifest generations.
const CONFIG_STORE: &str = "cpan_overlay";
/// Bump when the cached index representation changes.
const CACHE_FORMAT: &str = "v1";
/// How long a last-known-good index is kept for upstream outages.
const LAST_KNOWN_GOOD_TTL: Duration = Duration::from_secs(7 * 24 * 3600);

/// Request headers worth forwarding to artifact/upstream origins. Notably
/// not `Authorization` or `Cookie`.
const FORWARD_REQUEST_HEADERS: &[&str] = &[
    "range",
    "if-range",
    "if-none-match",
    "if-modified-since",
    "accept-encoding",
];
const FORWARD_RESPONSE_HEADERS: &[&str] = &[
    "content-type",
    "content-length",
    "content-range",
    "content-encoding",
    "accept-ranges",
    "etag",
    "last-modified",
    "cache-control",
    "expires",
];

struct Config {
    generation: String,
    manifest: Manifest,
    /// Identity of the exact manifest bytes, so cache keys change even if a
    /// generation name is reused.
    digest: String,
}

pub fn handle(req: Request) -> Response {
    let config = match load_config() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("configuration unavailable: {e}");
            return text(
                StatusCode::SERVICE_UNAVAILABLE,
                "overlay configuration is unavailable\n",
            );
        }
    };
    let head = req.get_method() == Method::HEAD;
    let path = req.get_path().to_string();
    let mut resp = match cpan_overlay_core::route(&config.manifest, req.get_method_str(), &path) {
        Route::Home => html(home::render(&config.manifest)),
        Route::Index { compressed } => serve_index(&config, &req, compressed),
        Route::Checksums => text(StatusCode::OK, &checksums::render(&config.manifest)),
        Route::Artifact(d) => serve_artifact(&config.manifest, d, &req),
        Route::Upstream(p) => serve_upstream(&config.manifest, p, &req),
        Route::NotFound => text(StatusCode::NOT_FOUND, "not found\n"),
        Route::MethodNotAllowed => text(StatusCode::METHOD_NOT_ALLOWED, "method not allowed\n")
            .with_header(header::ALLOW, "GET, HEAD"),
        Route::BadRequest => text(StatusCode::BAD_REQUEST, "bad request\n"),
    };
    resp.set_header("x-cpan-overlay-generation", &config.generation);
    if head {
        // Keep Content-Length from the full representation.
        let len = resp
            .get_header_str(header::CONTENT_LENGTH)
            .map(str::to_string);
        let _ = resp.take_body();
        if let Some(len) = len {
            resp.set_header(header::CONTENT_LENGTH, len);
        }
    }
    resp
}

/// Config Store layout:
///
/// * `active` — the generation to serve, e.g. `2026-11-26.1`
/// * `manifest_<generation>` — the manifest JSON, or
/// * `manifest_<generation>_parts` = N plus `manifest_<generation>_0` ..
///   `manifest_<generation>_<N-1>`, concatenated, for manifests larger than
///   one Config Store value.
fn load_config() -> Result<Config, String> {
    let store =
        ConfigStore::try_open(CONFIG_STORE).map_err(|e| format!("open {CONFIG_STORE}: {e:?}"))?;
    let get = |key: &str| store.try_get(key).map_err(|e| format!("read {key}: {e:?}"));
    let generation = get("active")?.ok_or("no active generation")?;
    if generation.is_empty()
        || !generation
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.'))
    {
        return Err(format!("invalid generation name {generation:?}"));
    }
    let key = format!("manifest_{}", generation.replace('.', "_"));
    let json = match get(&format!("{key}_parts"))? {
        Some(n) => {
            let n: usize = n
                .trim()
                .parse()
                .map_err(|_| format!("{key}_parts is not a number"))?;
            let mut json = String::new();
            for i in 0..n {
                json.push_str(&get(&format!("{key}_{i}"))?.ok_or(format!("missing {key}_{i}"))?);
            }
            json
        }
        None => get(&key)?.ok_or(format!("missing {key}"))?,
    };
    let manifest = Manifest::from_json(&json).map_err(|e| e.to_string())?;
    Ok(Config {
        generation,
        digest: sha256_hex(json.as_bytes())[..16].to_string(),
        manifest,
    })
}

// ---------------------------------------------------------------------------
// Merged index
// ---------------------------------------------------------------------------

/// Cached as `<etag>\n<gzip bytes>`.
fn serve_index(config: &Config, req: &Request, compressed: bool) -> Response {
    let key = format!(
        "cpan-overlay:index:{CACHE_FORMAT}:{}:{}",
        config.generation, config.digest
    );
    let lkg_key = format!("{key}:last-known-good");
    let ttl = Duration::from_secs(config.manifest.upstream.refresh_seconds.into());

    let mut rebuilt = None;
    let cached = cache::get_or_set_with(key, || {
        let blob = build_index(&config.manifest)?;
        rebuilt = Some(blob.clone());
        Ok(cache::CacheEntry {
            value: blob.into(),
            ttl,
        })
    });
    let blob = match cached {
        Ok(Some(body)) => body.into_bytes(),
        Ok(None) => unreachable!("closure returns a value"),
        Err(e) => {
            eprintln!("merged index unavailable: {e}");
            match cache::get(lkg_key.clone()) {
                Ok(Some(body)) => {
                    eprintln!("serving last-known-good index");
                    body.into_bytes()
                }
                _ => {
                    return text(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "the overlay index cannot be built right now; refusing to serve an unpatched index\n",
                    )
                    .with_header(header::RETRY_AFTER, "60");
                }
            }
        }
    };
    if let Some(fresh) = rebuilt {
        store_last_known_good(&lkg_key, fresh);
    }

    let Some(nl) = blob.iter().position(|&b| b == b'\n') else {
        return text(StatusCode::INTERNAL_SERVER_ERROR, "corrupt cached index\n");
    };
    let etag = String::from_utf8_lossy(&blob[..nl]).to_string();
    let gz = &blob[nl + 1..];

    let (etag, body, content_type) = if compressed {
        (format!("\"{etag}\""), gz.to_vec(), "application/x-gzip")
    } else {
        let mut plain = Vec::new();
        if let Err(e) = flate2::read::GzDecoder::new(gz).read_to_end(&mut plain) {
            eprintln!("cannot decompress cached index: {e}");
            return text(StatusCode::INTERNAL_SERVER_ERROR, "corrupt cached index\n");
        }
        (
            format!("\"{etag}-plain\""),
            plain,
            "text/plain; charset=utf-8",
        )
    };

    if req
        .get_header_str(header::IF_NONE_MATCH)
        .is_some_and(|v| v.split(',').any(|t| t.trim() == etag || t.trim() == "*"))
    {
        return Response::from_status(StatusCode::NOT_MODIFIED).with_header(header::ETAG, etag);
    }
    Response::from_status(StatusCode::OK)
        .with_header(header::CONTENT_TYPE, content_type)
        .with_header(header::CONTENT_LENGTH, body.len().to_string())
        .with_header(header::ETAG, etag)
        .with_header(
            header::CACHE_CONTROL,
            format!(
                "public, max-age={}",
                config.manifest.upstream.refresh_seconds
            ),
        )
        .with_body(body)
}

fn build_index(manifest: &Manifest) -> Result<Vec<u8>, fastly::Error> {
    let (backend, base_url) = http_backend(manifest, &manifest.upstream.backend)
        .ok_or_else(|| fastly::Error::msg("upstream backend is not http"))?;
    let url = format!("{base_url}/modules/02packages.details.txt.gz");
    let resp = Request::get(&url).send(backend)?;
    if resp.get_status() != StatusCode::OK {
        return Err(fastly::Error::msg(format!(
            "upstream index {url} returned {}",
            resp.get_status()
        )));
    }
    // Keep peak memory down: drop each stage's input as soon as possible.
    let text = {
        let gz = resp.into_body_bytes();
        index::decode_index(&gz, index::DEFAULT_MAX_INDEX_BYTES)?
    };
    let upstream = index::Index::parse(&text)?;
    let merged = index::merge(&upstream, manifest)?;
    drop(upstream);
    for w in &merged.warnings {
        eprintln!("warning: {w}");
    }
    let gz = merged.to_gz_bytes()?;
    let mut blob = sha256_hex(&gz).as_bytes()[..32].to_vec();
    blob.push(b'\n');
    blob.extend_from_slice(&gz);
    Ok(blob)
}

fn store_last_known_good(key: &str, blob: Vec<u8>) {
    let result: Result<(), fastly::Error> = (|| {
        let mut body = cache_core::insert(key.to_string().into(), LAST_KNOWN_GOOD_TTL).execute()?;
        body.write_all(&blob)?;
        body.finish()?;
        Ok(())
    })();
    if let Err(e) = result {
        eprintln!("cannot store last-known-good index: {e}");
    }
}

// ---------------------------------------------------------------------------
// Artifacts and upstream delegation
// ---------------------------------------------------------------------------

fn http_backend<'a>(manifest: &'a Manifest, name: &'a str) -> Option<(&'a str, &'a str)> {
    match manifest.backends.get(name)? {
        Backend::Http { base_url, .. } => Some((name, base_url.trim_end_matches('/'))),
        Backend::Kv { .. } => None,
    }
}

fn serve_artifact(manifest: &Manifest, d: &Distribution, req: &Request) -> Response {
    match manifest.backends.get(&d.backend) {
        Some(Backend::Http { base_url, delivery }) => {
            let url = format!("{}/{}", base_url.trim_end_matches('/'), d.object);
            match delivery {
                Delivery::Redirect => redirect(&url),
                Delivery::Proxy => proxy(&d.backend, &url, req, true),
            }
        }
        Some(Backend::Kv { store }) => serve_kv(store, d),
        None => text(StatusCode::INTERNAL_SERVER_ERROR, "backend missing\n"),
    }
}

fn serve_kv(store: &str, d: &Distribution) -> Response {
    let store_handle = match KVStore::open(store) {
        Ok(Some(s)) => s,
        Ok(None) => {
            eprintln!("KV store {store} is not linked");
            return text(StatusCode::BAD_GATEWAY, "artifact store unavailable\n");
        }
        Err(e) => {
            eprintln!("KV store {store} unavailable: {e:?}");
            return text(StatusCode::BAD_GATEWAY, "artifact store unavailable\n");
        }
    };
    match store_handle.lookup(&d.object) {
        Ok(mut found) => {
            let body = found.take_body();
            let mut resp = Response::from_status(StatusCode::OK)
                .with_header(header::CONTENT_TYPE, content_type(&d.path))
                .with_header(header::ETAG, format!("\"{}\"", &d.sha256[..32]))
                .with_header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
                .with_body(body);
            if let Some(size) = d.size {
                resp.set_header(header::CONTENT_LENGTH, size.to_string());
            }
            resp
        }
        Err(e) => {
            // The artifact is in the index; its absence is a server fault and
            // must never fall back to upstream bytes.
            eprintln!("artifact {} missing from KV store {store}: {e:?}", d.object);
            text(
                StatusCode::BAD_GATEWAY,
                "configured artifact is unavailable\n",
            )
        }
    }
}

fn serve_upstream(manifest: &Manifest, path: &str, req: &Request) -> Response {
    let name = &manifest.upstream.backend;
    let Some(Backend::Http { base_url, delivery }) = manifest.backends.get(name) else {
        return text(
            StatusCode::INTERNAL_SERVER_ERROR,
            "upstream misconfigured\n",
        );
    };
    let mut url = format!("{}{path}", base_url.trim_end_matches('/'));
    if let Some(q) = req.get_query_str() {
        url.push('?');
        url.push_str(q);
    }
    match delivery {
        Delivery::Redirect => redirect(&url),
        Delivery::Proxy => proxy(name, &url, req, false),
    }
}

/// Fetch `url` from `backend` and stream it back. For overlay artifacts
/// (`custom`), any failure is a 502: an indexed custom artifact that cannot
/// be fetched is a fault, not a "not found".
fn proxy(backend: &str, url: &str, req: &Request, custom: bool) -> Response {
    let mut out = Request::new(req.get_method().clone(), url);
    for name in FORWARD_REQUEST_HEADERS {
        if let Some(v) = req.get_header(*name) {
            out.set_header(*name, v.clone());
        }
    }
    let resp = match out.send(backend) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("backend {backend} request for {url} failed: {e}");
            return text(StatusCode::BAD_GATEWAY, "upstream request failed\n");
        }
    };
    let status = resp.get_status();
    let ok = status.is_success() || status == StatusCode::NOT_MODIFIED;
    if custom && !ok {
        eprintln!("backend {backend} returned {status} for custom artifact {url}");
        return text(
            StatusCode::BAD_GATEWAY,
            "configured artifact is unavailable\n",
        );
    }
    if status.is_redirection() && status != StatusCode::NOT_MODIFIED {
        // Do not hand clients an origin redirect we have not vetted.
        eprintln!("backend {backend} redirected {url}; not followed");
        return text(
            StatusCode::BAD_GATEWAY,
            "upstream redirected unexpectedly\n",
        );
    }
    let mut client = Response::from_status(status);
    for name in FORWARD_RESPONSE_HEADERS {
        if let Some(v) = resp.get_header(*name) {
            client.set_header(*name, v.clone());
        }
    }
    client.set_body(resp.into_body());
    client
}

fn redirect(url: &str) -> Response {
    Response::from_status(StatusCode::FOUND)
        .with_header(header::LOCATION, url)
        .with_header(header::CACHE_CONTROL, "public, max-age=300")
}

// ---------------------------------------------------------------------------

fn content_type(path: &str) -> &'static str {
    if path.ends_with(".zip") {
        "application/zip"
    } else if path.ends_with(".tar.bz2") {
        "application/x-bzip2"
    } else if path.ends_with(".tar.xz") {
        "application/x-xz"
    } else if path.ends_with(".tar") {
        "application/x-tar"
    } else {
        "application/x-gzip"
    }
}

fn text(status: StatusCode, body: &str) -> Response {
    Response::from_status(status)
        .with_header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .with_header(header::CONTENT_LENGTH, body.len().to_string())
        .with_body(Body::from(body))
}

fn html(body: String) -> Response {
    Response::from_status(StatusCode::OK)
        .with_header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .with_header(header::CONTENT_LENGTH, body.len().to_string())
        .with_body(body)
}
