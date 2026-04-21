//! Build script — emits build-time provenance per L2 constitution P11.
//!
//! Sets `VERGEN_BUILD_TIMESTAMP`, `VERGEN_CARGO_DEBUG`, `VERGEN_GIT_SHA`,
//! `VERGEN_GIT_DIRTY`, `VERGEN_RUSTC_SEMVER` (and friends) as
//! `cargo:rustc-env` so the binary can read them at runtime via `env!()`.
//!
//! # Nix `SOURCE_DATE_EPOCH` workaround
//!
//! `nixpkgs` pre-sets `SOURCE_DATE_EPOCH=315532800` (1980-01-01T00:00:00Z)
//! in every `mkShell` environment as a reproducible-build default.
//! `vergen` respects `SOURCE_DATE_EPOCH`, which would otherwise force
//! `weaver --version` to display `1980-01-01T00:00:00Z` in dev builds —
//! a direct regression against L2 P11 (build timestamp must be
//! informative).
//!
//! The compromise: if and only if `SOURCE_DATE_EPOCH` matches the nix
//! stdenv default sentinel, clear it before invoking `vergen`. Any
//! intentional value (e.g., a CI release build setting it to the
//! commit timestamp for bit-reproducibility) is still respected.

use vergen::EmitBuilder;

/// Nix stdenv's sentinel "reproducible-build" timestamp: 1980-01-01T00:00:00Z.
const NIX_STDENV_SENTINEL: &str = "315532800";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("SOURCE_DATE_EPOCH").as_deref() == Ok(NIX_STDENV_SENTINEL) {
        // Safety: build.rs runs single-threaded before any Cargo worker
        // has been spawned; no other thread can observe the env.
        unsafe {
            std::env::remove_var("SOURCE_DATE_EPOCH");
        }
    }

    EmitBuilder::builder()
        .all_build()
        .all_cargo()
        .all_git()
        .all_rustc()
        .emit()?;
    Ok(())
}
