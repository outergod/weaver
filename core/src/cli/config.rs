//! Configuration loading — XDG + env + CLI precedence per L2 Additional
//! Constraints and `specs/001-hello-fact/contracts/cli-surfaces.md`.

use std::path::{Path, PathBuf};

/// Effective configuration after XDG file + env vars + CLI flags.
#[derive(Clone, Debug)]
pub struct Config {
    pub socket_path: PathBuf,
    pub log_level: String,
}

impl Config {
    /// Resolve the default bus socket path.
    ///
    /// Precedence:
    /// 1. `$XDG_RUNTIME_DIR/weaver.sock`
    /// 2. `/tmp/weaver.sock` (fallback; XDG not always set outside login sessions)
    pub fn default_socket_path() -> PathBuf {
        if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR") {
            Path::new(&xdg).join("weaver.sock")
        } else {
            PathBuf::from("/tmp/weaver.sock")
        }
    }

    /// Build a `Config` from a CLI `socket` override (when `Some`) or
    /// the default. Slice 001 reads neither a config file nor env vars
    /// for the log level — tracing_setup handles that directly via
    /// `RUST_LOG` — but the type exists for later expansion.
    pub fn from_cli(socket_override: Option<PathBuf>) -> Self {
        Self {
            socket_path: socket_override.unwrap_or_else(Self::default_socket_path),
            log_level: std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_override_wins_over_default() {
        let c = Config::from_cli(Some(PathBuf::from("/custom/path.sock")));
        assert_eq!(c.socket_path, PathBuf::from("/custom/path.sock"));
    }
}
