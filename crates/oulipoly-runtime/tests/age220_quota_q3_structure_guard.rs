use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn quota_q3_facade_declares_freshness_module_and_reexports_public_surface() {
    let source = read("src/quota/mod.rs");

    assert!(
        source.contains("mod freshness;"),
        "quota facade must declare private freshness module"
    );

    let reexports = parse_freshness_reexport_source(&source);
    assert_freshness_reexports_present(&reexports);
    for public_item in [
        "TOPOLOGY_PROBE_COOLDOWN_SECS",
        "dynamic_ttl_secs",
        "is_routing_stale",
        "is_stale",
        "is_topology_probe_due",
    ] {
        assert!(
            reexports.contains(public_item),
            "quota facade must publicly re-export freshness item {public_item}"
        );
    }
}

#[test]
fn quota_q3_facade_no_longer_owns_freshness_function_bodies() {
    let source = read("src/quota/mod.rs");

    for function_body in [
        "pub fn is_stale(",
        "pub fn is_routing_stale(",
        "pub fn is_topology_probe_due(",
        "pub fn dynamic_ttl_secs(",
    ] {
        assert!(
            !source.contains(function_body),
            "quota facade must not own moved freshness function body {function_body}"
        );
    }
}

#[test]
fn quota_q3_freshness_module_owns_public_api_and_private_ttl_constants() {
    let source = read("src/quota/freshness.rs");

    for public_api in [
        "pub const TOPOLOGY_PROBE_COOLDOWN_SECS: u64 = 60 * 60",
        "pub fn is_stale(",
        "pub fn is_routing_stale(",
        "pub fn is_topology_probe_due(",
        "pub fn dynamic_ttl_secs(",
    ] {
        assert!(
            source.contains(public_api),
            "quota::freshness must own public moved API {public_api}"
        );
    }

    for private_constant in [
        "const MIN_TTL_SECS: i64 = 5 * 60",
        "const MAX_TTL_SECS: i64 = 24 * 3600",
        "const REFRESH_WINDOW_DIVISOR: i64 = 5",
        "const ROUTING_REFRESH_TTL_SECS: i64 = 30",
    ] {
        assert!(
            source.contains(private_constant),
            "quota::freshness must retain private TTL constant {private_constant}"
        );
    }

    for widened_constant in [
        "pub const MIN_TTL_SECS",
        "pub const MAX_TTL_SECS",
        "pub const REFRESH_WINDOW_DIVISOR",
        "pub const ROUTING_REFRESH_TTL_SECS",
    ] {
        assert!(
            !source.contains(widened_constant),
            "quota::freshness must not widen private TTL constant visibility: {widened_constant}"
        );
    }
}

fn parse_freshness_reexport_source(source: &str) -> String {
    let mut reexports = String::new();
    for prefix in ["pub use freshness::", "pub use self::freshness::"] {
        let mut rest = source;
        while let Some(offset) = rest.find(prefix) {
            let candidate = &rest[offset..];
            let end = candidate.find(';').unwrap_or(candidate.len());
            reexports.push_str(&candidate[..end]);
            reexports.push('\n');
            rest = &candidate[end..];
        }
    }
    reexports
}

fn assert_freshness_reexports_present(reexports: &str) {
    assert!(
        !reexports.is_empty(),
        "quota facade must re-export public freshness API from quota::freshness"
    );
}

fn read(relative: &str) -> String {
    let path = manifest_path(relative);
    fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "failed to read required AGE-220 Q3 source file {}: {error}",
            path.display()
        )
    })
}

fn manifest_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}
