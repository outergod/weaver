//! Build script — emits build-time provenance per L2 constitution P11.
//!
//! Sets `VERGEN_BUILD_TIMESTAMP`, `VERGEN_CARGO_DEBUG`, `VERGEN_GIT_SHA`,
//! `VERGEN_GIT_DIRTY`, `VERGEN_RUSTC_SEMVER` (and friends) as
//! `cargo:rustc-env` so the binary can read them at runtime via `env!()`.

use vergen::EmitBuilder;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    EmitBuilder::builder()
        .all_build()
        .all_cargo()
        .all_git()
        .all_rustc()
        .emit()?;
    Ok(())
}
