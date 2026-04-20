//! Configuration loading — XDG + env + CLI precedence per L2 Additional
//! Constraints and `specs/001-hello-fact/contracts/cli-surfaces.md`.
//!
//! # Precedence (highest wins)
//!
//! 1. CLI flag (`--socket=<path>`; clap also folds `WEAVER_SOCKET` env
//!    into the same slot via `#[arg(env = ...)]`).
//! 2. `$XDG_CONFIG_HOME/weaver/config.toml` (falls back to
//!    `$HOME/.config/weaver/config.toml`).
//! 3. Built-in default (`$XDG_RUNTIME_DIR/weaver.sock`, else
//!    `/tmp/weaver.sock`).
//!
//! Log level follows the same pattern with `RUST_LOG` taking highest
//! precedence (consumed directly by `tracing_setup`, not by this
//! module). The file's `log_level` field is surfaced here for future
//! `tracing_setup` integration.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Effective configuration after XDG file + env vars + CLI flags.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub socket_path: PathBuf,
    pub log_level: String,
}

/// On-disk shape of `config.toml`. All keys optional; defaults applied
/// when a key is missing.
#[derive(Debug, Default, Deserialize)]
struct FileConfig {
    #[serde(default)]
    socket_path: Option<PathBuf>,
    #[serde(default)]
    log_level: Option<String>,
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

    /// Resolve the config-file path per XDG Base Directory Spec.
    fn config_file_path() -> Option<PathBuf> {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            if !xdg.is_empty() {
                return Some(Path::new(&xdg).join("weaver/config.toml"));
            }
        }
        if let Ok(home) = std::env::var("HOME") {
            return Some(Path::new(&home).join(".config/weaver/config.toml"));
        }
        None
    }

    /// Read and parse the XDG config file. Returns a defaulted
    /// `FileConfig` (all fields `None`) if:
    /// * the XDG/HOME env vars are both unset (can't compute a path);
    /// * the file does not exist;
    /// * the file exists but is unreadable or malformed.
    ///
    /// Parse/read failures log a `tracing::warn!` so the operator
    /// sees the problem; they do NOT abort startup, because a broken
    /// config file shouldn't prevent the binary from running with
    /// CLI flags alone.
    fn load_file() -> FileConfig {
        let Some(path) = Self::config_file_path() else {
            return FileConfig::default();
        };
        Self::load_file_from(&path)
    }

    fn load_file_from(path: &Path) -> FileConfig {
        match std::fs::read_to_string(path) {
            Ok(s) => toml::from_str::<FileConfig>(&s).unwrap_or_else(|e| {
                tracing::warn!(
                    target: "weaver::config",
                    path = %path.display(),
                    error = %e,
                    "failed to parse config; falling back to defaults"
                );
                FileConfig::default()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => FileConfig::default(),
            Err(e) => {
                tracing::warn!(
                    target: "weaver::config",
                    path = %path.display(),
                    error = %e,
                    "failed to read config; falling back to defaults"
                );
                FileConfig::default()
            }
        }
    }

    /// Build a `Config` with full precedence resolution.
    ///
    /// `socket_override` is the post-clap value — already folded with
    /// `WEAVER_SOCKET` env via the clap derive's `#[arg(env = ...)]`
    /// attribute. Highest precedence.
    pub fn from_cli(socket_override: Option<PathBuf>) -> Self {
        let file = Self::load_file();
        Self::from_parts(socket_override, file)
    }

    fn from_parts(socket_override: Option<PathBuf>, file: FileConfig) -> Self {
        let socket_path = socket_override
            .or(file.socket_path)
            .unwrap_or_else(Self::default_socket_path);
        let log_level = std::env::var("RUST_LOG")
            .ok()
            .or(file.log_level)
            .unwrap_or_else(|| "info".into());
        Self {
            socket_path,
            log_level,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_override_wins_over_default() {
        let c = Config::from_parts(
            Some(PathBuf::from("/custom/path.sock")),
            FileConfig::default(),
        );
        assert_eq!(c.socket_path, PathBuf::from("/custom/path.sock"));
    }

    #[test]
    fn file_socket_path_used_when_cli_none() {
        let file = FileConfig {
            socket_path: Some(PathBuf::from("/from-file.sock")),
            log_level: None,
        };
        let c = Config::from_parts(None, file);
        assert_eq!(c.socket_path, PathBuf::from("/from-file.sock"));
    }

    #[test]
    fn cli_overrides_file() {
        let file = FileConfig {
            socket_path: Some(PathBuf::from("/from-file.sock")),
            log_level: None,
        };
        let c = Config::from_parts(Some(PathBuf::from("/cli.sock")), file);
        assert_eq!(c.socket_path, PathBuf::from("/cli.sock"));
    }

    #[test]
    fn default_when_neither_cli_nor_file() {
        let c = Config::from_parts(None, FileConfig::default());
        // Can't assert equality (depends on `$XDG_RUNTIME_DIR`), but it
        // should land on one of the two documented defaults.
        assert!(
            c.socket_path.ends_with("weaver.sock"),
            "default must name `weaver.sock`; got {:?}",
            c.socket_path,
        );
    }

    #[test]
    fn load_file_from_parses_full_config() {
        let tmp = std::env::temp_dir().join(format!("weaver-cfg-{}.toml", std::process::id()));
        std::fs::write(
            &tmp,
            b"socket_path = \"/tmp/custom.sock\"\nlog_level = \"debug\"\n",
        )
        .unwrap();
        let file = Config::load_file_from(&tmp);
        assert_eq!(file.socket_path, Some(PathBuf::from("/tmp/custom.sock")));
        assert_eq!(file.log_level.as_deref(), Some("debug"));
        std::fs::remove_file(&tmp).unwrap();
    }

    #[test]
    fn load_file_from_missing_returns_default() {
        let tmp =
            std::env::temp_dir().join(format!("weaver-cfg-missing-{}.toml", std::process::id()));
        // Ensure it doesn't exist.
        let _ = std::fs::remove_file(&tmp);
        let file = Config::load_file_from(&tmp);
        assert!(file.socket_path.is_none());
        assert!(file.log_level.is_none());
    }

    #[test]
    fn load_file_from_malformed_falls_back_to_default() {
        let tmp = std::env::temp_dir().join(format!("weaver-cfg-bad-{}.toml", std::process::id()));
        std::fs::write(&tmp, b"this is = not valid toml [[").unwrap();
        let file = Config::load_file_from(&tmp);
        assert!(file.socket_path.is_none());
        std::fs::remove_file(&tmp).unwrap();
    }

    #[test]
    fn partial_file_config_only_overrides_set_keys() {
        // `log_level` set but `socket_path` missing — CLI default for
        // socket still wins; log_level from file flows through.
        let file = FileConfig {
            socket_path: None,
            log_level: Some("trace".into()),
        };
        // Unset RUST_LOG for determinism of this test.
        // Safety: single-threaded test process; no other thread
        // observes env during this check.
        let saved = std::env::var_os("RUST_LOG");
        unsafe {
            std::env::remove_var("RUST_LOG");
        }
        let c = Config::from_parts(None, file);
        assert_eq!(c.log_level, "trace");
        assert!(c.socket_path.ends_with("weaver.sock"));
        if let Some(v) = saved {
            unsafe {
                std::env::set_var("RUST_LOG", v);
            }
        }
    }
}
