//! Runtime-neutral core of CPAN Overlay.
//!
//! Everything here is synchronous and free of transport/runtime types so
//! the same semantics run on Fastly Compute, Viceroy and (later) a native
//! server. Runtime adapters fetch bytes and hand them in; the core decides
//! what the overlay means.

pub mod checksums;
pub mod distname;
pub mod home;
pub mod index;
pub mod manifest;
pub mod route;
pub mod version;

pub use index::{Change, DistributionStatus, Index, Merged, decode_index, merge};
pub use manifest::Manifest;
pub use route::{Route, route};

/// Crate version, used in generated headers.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid manifest:\n  {}", .0.join("\n  "))]
    Manifest(Vec<String>),
    #[error("cannot apply overlay to upstream index:\n  {}", .0.join("\n  "))]
    Merge(Vec<String>),
    #[error("invalid upstream index: {0}")]
    Index(String),
}

/// Hex-encoded SHA-256, used for ETags and cache identities.
pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
