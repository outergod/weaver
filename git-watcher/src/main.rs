//! `weaver-git-watcher` binary entry point.

use miette::Report;

fn main() -> Result<(), Report> {
    weaver_git_watcher::cli::run()
}
