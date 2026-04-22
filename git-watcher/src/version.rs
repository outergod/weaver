//! `weaver-git-watcher --version` rendering.
//!
//! Mirrors `core/src/cli/version.rs` with:
//! - `name = "weaver-git-watcher"`
//! - an added `service_id = "git-watcher"` field naming the watcher's
//!   service identity on the bus.
//!
//! clap's built-in `version` action was dropped so `--version` can
//! honour `--output=human|json` per
//! `specs/002-git-watcher-actor/contracts/cli-surfaces.md`. Without
//! this module, clap's one-liner default exited before `cli::run`,
//! breaking scripts consuming the JSON shape.

use serde::Serialize;

use weaver_core::types::message::BUS_PROTOCOL_VERSION_STR;

const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");
const GIT_SHA: &str = env!("VERGEN_GIT_SHA");
const GIT_DIRTY: &str = env!("VERGEN_GIT_DIRTY");
const BUILD_TIMESTAMP: &str = env!("VERGEN_BUILD_TIMESTAMP");
const BUILD_PROFILE: &str = env!("VERGEN_CARGO_DEBUG");
const RUSTC_SEMVER: &str = env!("VERGEN_RUSTC_SEMVER");

/// The watcher's service identifier. Kept as a single source of
/// truth here; other call sites that need it (publisher's
/// `ActorIdentity::service("git-watcher", ...)`) reference the same
/// literal string. A future refactor can share a single constant
/// between modules.
pub const SERVICE_ID: &str = "git-watcher";

#[derive(Debug, Serialize)]
pub struct VersionInfo {
    pub name: &'static str,
    pub version: &'static str,
    pub commit: &'static str,
    pub dirty: bool,
    pub built_at: &'static str,
    pub profile: &'static str,
    pub rustc: &'static str,
    pub bus_protocol: &'static str,
    pub service_id: &'static str,
}

impl VersionInfo {
    pub fn current() -> Self {
        Self {
            name: "weaver-git-watcher",
            version: CRATE_VERSION,
            commit: GIT_SHA,
            dirty: GIT_DIRTY == "true",
            built_at: BUILD_TIMESTAMP,
            profile: profile_label(),
            rustc: RUSTC_SEMVER,
            bus_protocol: BUS_PROTOCOL_VERSION_STR,
            service_id: SERVICE_ID,
        }
    }
}

fn profile_label() -> &'static str {
    if BUILD_PROFILE == "true" {
        "debug"
    } else {
        "release"
    }
}

/// Print the contracted version output in the requested format.
/// `format` accepts the raw CLI `--output` string ("human" or "json");
/// anything else falls back to the human form.
pub fn print_version(format: &str) {
    let info = VersionInfo::current();
    if format == "json" {
        print_json(&info);
    } else {
        print_human(&info);
    }
}

fn print_human(info: &VersionInfo) {
    println!("{} {}", info.name, info.version);
    println!(
        "  commit: {}{}",
        info.commit,
        if info.dirty { " (dirty)" } else { "" }
    );
    println!("  built:  {}", info.built_at);
    println!("  profile: {}", info.profile);
    println!("  rustc: {}", info.rustc);
    println!("  bus protocol: v{}", info.bus_protocol);
    println!("  service-id: {}", info.service_id);
}

fn print_json(info: &VersionInfo) {
    let s = serde_json::to_string_pretty(info).expect("VersionInfo always serializes");
    println!("{s}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_info_has_all_contracted_fields() {
        let info = VersionInfo::current();
        assert!(!info.name.is_empty());
        assert!(!info.version.is_empty());
        assert!(!info.commit.is_empty());
        assert!(!info.built_at.is_empty());
        assert!(!info.profile.is_empty());
        assert!(!info.rustc.is_empty());
        assert!(!info.bus_protocol.is_empty());
        assert_eq!(info.name, "weaver-git-watcher");
        assert_eq!(info.service_id, "git-watcher");
    }

    #[test]
    fn json_output_has_all_contracted_fields() {
        let info = VersionInfo::current();
        let s = serde_json::to_string(&info).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&s).unwrap();
        for field in [
            "name",
            "version",
            "commit",
            "dirty",
            "built_at",
            "profile",
            "rustc",
            "bus_protocol",
            "service_id",
        ] {
            assert!(parsed.get(field).is_some(), "JSON missing field `{field}`");
        }
        assert_eq!(parsed["name"], "weaver-git-watcher");
        assert_eq!(parsed["service_id"], "git-watcher");
        assert_eq!(parsed["bus_protocol"], BUS_PROTOCOL_VERSION_STR);
    }
}
