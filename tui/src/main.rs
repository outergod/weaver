//! `weaver-tui` binary entry point — delegates to the library's `run`.

fn main() -> miette::Result<()> {
    weaver_tui::run()
}
