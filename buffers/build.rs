//! Build-time provenance per L2 P11: emit git SHA, dirty bit, build
//! timestamp, and profile as compile-time environment variables.
//! `weaver-buffers --version` reads them via `env!()`.

fn main() {
    // Best-effort: failures inside a non-git source tree are tolerated
    // by vergen with a graceful fallback.
    let _ = vergen::EmitBuilder::builder()
        .build_timestamp()
        .cargo_debug()
        .git_sha(true)
        .git_dirty(true)
        .rustc_semver()
        .emit();
}
