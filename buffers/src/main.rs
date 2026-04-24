//! `weaver-buffers` binary entry point.

use miette::Report;

fn main() -> Result<(), Report> {
    weaver_buffers::cli::run()
}
