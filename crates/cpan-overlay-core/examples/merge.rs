//! Merge a manifest into a local upstream index and report timings.
//!
//!     cargo run --release -p cpan-overlay-core --example merge -- \
//!         02packages.details.txt.gz manifest.json [out.gz]

use std::time::Instant;

use cpan_overlay_core::{Index, Manifest, decode_index, index::DEFAULT_MAX_INDEX_BYTES, merge};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [index, manifest, rest @ ..] = args.as_slice() else {
        return Err("usage: merge <02packages.details.txt[.gz]> <manifest.json> [out.gz]".into());
    };

    let t = Instant::now();
    let raw = std::fs::read(index)?;
    let manifest = Manifest::from_json(&std::fs::read_to_string(manifest)?)?;
    let text = decode_index(&raw, DEFAULT_MAX_INDEX_BYTES)?;
    drop(raw);
    let upstream = Index::parse(&text)?;
    let parsed = t.elapsed();

    let t = Instant::now();
    let merged = merge(&upstream, &manifest)?;
    let merged_at = t.elapsed();

    let t = Instant::now();
    let gz = merged.to_gz_bytes()?;
    let gz_at = t.elapsed();

    eprintln!(
        "rows {} -> {}, {} changes; decode+parse {parsed:?}, merge {merged_at:?}, gzip {gz_at:?} ({} bytes)",
        upstream.len(),
        merged.index.len(),
        merged.changes.len(),
        gz.len()
    );
    for w in &merged.warnings {
        eprintln!("warning: {w}");
    }
    if let Some(out) = rest.first() {
        std::fs::write(out, gz)?;
    }
    Ok(())
}
