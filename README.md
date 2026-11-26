# CPAN Overlay

A CPAN mirror overlay: it serves the public CPAN index with explicit
amendments (personal distributions, and patched replacements for upstream
ones) and delegates everything else upstream. Ordinary CPAN clients work
unmodified:

```sh
cpanm --from https://your-overlay.example Some::Module
```

The flagship target is Fastly Compute; the same build runs locally under
Viceroy. A native standalone server is planned. See
[docs/DESIGN.md](docs/DESIGN.md) for the full design.

## Layout

| Crate | Purpose |
|---|---|
| `crates/cpan-overlay-core` | Runtime-neutral, synchronous core: manifest schema and validation, index merge, routing, `CHECKSUMS` |
| `crates/cpan-overlay-compute` | Fastly Compute adapter (`wasm32-wasip1`) |

## Run locally (Viceroy)

Requires the [Fastly CLI](https://www.fastly.com/documentation/reference/tools/cli/),
`rustup target add wasm32-wasip1`, `jq` and `curl`.

```sh
examples/viceroy/prepare.sh   # builds example distributions + store fixtures
fastly compute serve          # http://127.0.0.1:7676
cpanm --from http://127.0.0.1:7676 Personal::Tools File::Which
```

The example overlay adds `Personal::Tools` 0.01 and replaces upstream
`File-Which` 1.27 with 1.2701. Note that if `PERL_CPANM_OPT` sets `--from`,
it overrides `--mirror`; use `--from` on the command line.

## Configuration

The overlay reads a JSON manifest (see `examples/viceroy/manifest.json`) from
the Config Store `cpan_overlay`:

* `active`: the generation to serve, e.g. `2026-11-26.1`
* `manifest_<generation>` (dots become `_`): the manifest JSON, or
  `manifest_<generation>_parts` = N plus `manifest_<generation>_0` …
  `_<N-1>`, which are concatenated, for manifests larger than one value.

Switching `active` changes the overlay without a new build; an older
generation can be reselected to roll back.

Logical backend names in the manifest are Fastly backend names (HTTP) or
KV store names (`kv` backends name their store explicitly).

KV Store backends are **best effort, patches welcome**. They are implemented
and tested under Viceroy, but can't be tested on Fastly itself (KV Stores need a
paid account). For hosted deployments, prefer an HTTP backend, such as GitHub
release assets with `"delivery": "redirect"`.

## Development

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo clippy --target wasm32-wasip1 -p cpan-overlay-compute -- -D warnings

# Merge benchmark against a real index
curl -O https://cpan.metacpan.org/modules/02packages.details.txt.gz
cargo run --release -p cpan-overlay-core --example merge -- \
    02packages.details.txt.gz examples/viceroy/manifest.json out.gz
```

Releases: bump `[workspace.package].version` in `Cargo.toml`; on merge to
`main` the release workflow builds the Compute package and publishes it as a
GitHub release `v<version>`.

## License

MIT
