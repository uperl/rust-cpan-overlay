//! CPAN Overlay for Fastly Compute. Build for `wasm32-wasip1`; run locally
//! with `fastly compute serve` (Viceroy).

#[cfg(target_arch = "wasm32")]
mod app;

#[cfg(target_arch = "wasm32")]
#[fastly::main]
fn main(req: fastly::Request) -> Result<fastly::Response, fastly::Error> {
    Ok(app::handle(req))
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!(
        "cpan-overlay-compute only runs on Fastly Compute; build it with --target wasm32-wasip1"
    );
    std::process::exit(2);
}
