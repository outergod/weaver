//! `weaver --version` rendering.
//!
//! Outputs all P11-required fields (crate version, git SHA, dirty bit,
//! build timestamp, build profile, bus protocol version) in either
//! human or JSON form.
//!
//! Build-time provenance is supplied by `vergen` via the `core/build.rs`
//! script.

use serde::Serialize;

use crate::cli::args::OutputFormat;
use crate::types::message::BUS_PROTOCOL_VERSION_STR;

const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");
const GIT_SHA: &str = env!("VERGEN_GIT_SHA");
const GIT_DIRTY: &str = env!("VERGEN_GIT_DIRTY");
const BUILD_TIMESTAMP: &str = env!("VERGEN_BUILD_TIMESTAMP");
const BUILD_PROFILE: &str = env!("VERGEN_CARGO_DEBUG");
const RUSTC_SEMVER: &str = env!("VERGEN_RUSTC_SEMVER");

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
}

impl VersionInfo {
    pub fn current() -> Self {
        Self {
            name: "weaver",
            version: CRATE_VERSION,
            commit: GIT_SHA,
            dirty: GIT_DIRTY == "true",
            built_at: BUILD_TIMESTAMP,
            profile: profile_label(),
            rustc: RUSTC_SEMVER,
            bus_protocol: BUS_PROTOCOL_VERSION_STR,
        }
    }
}

/// Translate `VERGEN_CARGO_DEBUG` ("true"/"false") to the conventional
/// `"debug"`/`"release"` label.
fn profile_label() -> &'static str {
    if BUILD_PROFILE == "true" {
        "debug"
    } else {
        "release"
    }
}

/// Return the single-line version string used by `clap`'s `--version`
/// fallback (when no explicit format is requested).
pub fn version_line() -> String {
    let info = VersionInfo::current();
    format!(
        "{} {} (commit {}{})",
        info.name,
        info.version,
        info.commit,
        if info.dirty { ", dirty" } else { "" },
    )
}

/// Print `weaver --version` in the requested format.
pub fn print_version(format: OutputFormat) {
    let info = VersionInfo::current();
    match format {
        OutputFormat::Human => print_human(&info),
        OutputFormat::Json => print_json(&info),
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
}

fn print_json(info: &VersionInfo) {
    let s = serde_json::to_string_pretty(info).expect("VersionInfo always serializes");
    println!("{}", s);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_info_has_all_p11_fields() {
        let info = VersionInfo::current();
        assert!(!info.name.is_empty());
        assert!(!info.version.is_empty());
        assert!(!info.commit.is_empty());
        assert!(!info.built_at.is_empty());
        assert!(!info.profile.is_empty());
        assert!(!info.rustc.is_empty());
        assert!(!info.bus_protocol.is_empty());
        // `dirty` is a bool — both true and false are valid.
    }

    #[test]
    fn json_output_is_parseable() {
        let info = VersionInfo::current();
        let s = serde_json::to_string(&info).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&s).unwrap();
        // Verify all P11 fields are present in the JSON.
        for field in [
            "name",
            "version",
            "commit",
            "dirty",
            "built_at",
            "profile",
            "rustc",
            "bus_protocol",
        ] {
            assert!(
                parsed.get(field).is_some(),
                "JSON missing field `{field}`"
            );
        }
    }

    #[test]
    fn version_info_round_trip_via_serde_json() {
        // Round-trip via Value (deserializing back into VersionInfo
        // would require Deserialize derive; we only need round-trip
        // through the JSON model for the contract).
        let info = VersionInfo::current();
        let s = serde_json::to_string(&info).unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["name"], "weaver");
        assert_eq!(v["version"], CRATE_VERSION);
        assert_eq!(v["bus_protocol"], BUS_PROTOCOL_VERSION_STR);
    }
}
