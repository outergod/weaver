//! CLI surface for `weaver-git-watcher`. See
//! `specs/002-git-watcher-actor/contracts/cli-surfaces.md`.
//!
//! Phase 1 scaffold: prints a TODO and exits. Real CLI + publisher
//! wiring lands in Phase 3 (US1) tasks T036–T040.

use miette::Report;

pub fn run() -> Result<(), Report> {
    eprintln!(
        "weaver-git-watcher — Phase 1 scaffold. \
         Implementation lands in slice 002 Phase 3 (US1)."
    );
    Ok(())
}
