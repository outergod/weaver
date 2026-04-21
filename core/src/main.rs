//! `weaver` binary entry point — delegates to the library's `cli::run`.

fn main() -> miette::Result<()> {
    weaver_core::cli::run()
}
