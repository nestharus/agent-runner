use std::path::{Path, PathBuf};
#[path = "../../tests/support/provider_vocabulary_source_set.rs"]
mod provider_vocabulary_source_set;
use provider_vocabulary_source_set::SourceSet;

const MANAGER_BASELINE_PROVIDER_NAME_COUNT: usize = 4628;

#[test]
fn setup_requires_the_configured_family_endpoint_without_legacy_fallback() {
    let flow = read_source("src-tauri/src/setup/flow.rs");
    assert_eq!(
        flow.matches("SetupAgent::new(").count(),
        0,
        "setup must not construct the legacy hardcoded brain"
    );
    assert_not_contains(
        "endpoint-authoritative setup flow",
        &flow,
        "run_missing_config_legacy_fallback",
    );
    assert_not_contains(
        "endpoint-authoritative setup flow",
        &flow,
        "detection::detect_all()",
    );
    assert_not_contains(
        "endpoint-authoritative setup flow",
        &flow,
        "detection::detect_single_cli(",
    );
    assert_contains(
        "missing-config fail-closed boundary",
        &flow,
        "fn require_setup_brain(",
    );
    assert_contains(
        "missing-config fail-closed boundary",
        &flow,
        "setup_brain_not_configured",
    );
    assert_contains(
        "configured setup brain dispatch",
        &flow,
        "try_run_configured_setup_brain(",
    );
}

#[test]
fn configured_path_uses_provider_ref_adapter_and_does_not_call_legacy() {
    let host = read_source("src-tauri/src/setup/setup_brain_host.rs");
    let flow = read_source("src-tauri/src/setup/flow.rs");
    let configured_branch = source_between(&flow, "Ok(setup_brain) =>", "Err(error) =>");

    assert_contains("setup brain host", &host, "ProviderRegistry::convert_ref");
    assert_contains("setup brain host", &host, "preflight_bootstrap_family");
    assert_contains("setup brain host", &host, "PinnedFamilyEndpoint");
    assert_contains("setup brain host", &host, "build_setup_brain_turn_request");
    assert_contains("setup brain host", &host, "require_setup_brain_capability");
    assert_contains("setup brain host", &host, "decode_setup_brain_turn_result");
    assert_contains("setup brain host", &host, "setup_brain.turn");
    assert_not_contains(
        "configured setup brain branch",
        configured_branch,
        "LegacySetupBrainFallback",
    );
    assert_not_contains(
        "configured setup brain branch",
        configured_branch,
        "run_legacy_setup_brain_fallback(",
    );
}

#[test]
fn new_setup_brain_host_does_not_construct_direct_process_command() {
    let host = read_source("src-tauri/src/setup/setup_brain_host.rs");

    assert_not_contains("setup brain host", &host, "std::process::Command");
    assert_not_contains("setup brain host", &host, "Command::new(");
    assert_not_contains("setup brain host", &host, ".spawn(");
    assert_not_contains("setup brain host", &host, ".output(");
}

#[test]
fn s8_files_introduce_zero_new_concrete_provider_vocabulary() {
    let root = workspace_root();
    let pattern = concrete_provider_pattern();
    let added = SourceSet::load(&root).added_occurrences(None, &pattern);

    assert_eq!(
        added, 0,
        "S8 files must not introduce concrete-provider vocabulary"
    );
}

#[test]
fn full_provider_name_grep_threshold_remains_within_manager_baseline() {
    let root = workspace_root();
    let count = SourceSet::load(&root).full_occurrences(&concrete_provider_pattern());

    assert!(
        count <= MANAGER_BASELINE_PROVIDER_NAME_COUNT,
        "full provider-name grep count {count} exceeds manager baseline {MANAGER_BASELINE_PROVIDER_NAME_COUNT}"
    );
}

fn concrete_provider_pattern() -> String {
    format!(
        "{}|{}",
        ["cl", "au", "de"].concat(),
        ["co", "de", "x"].concat()
    )
}

fn read_source(relative: &str) -> String {
    std::fs::read_to_string(workspace_root().join(relative))
        .unwrap_or_else(|error| panic!("failed to read {relative}: {error}"))
}

fn source_between<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let start_index = source
        .find(start)
        .unwrap_or_else(|| panic!("missing start marker {start:?}"));
    let end_index = source[start_index..]
        .find(end)
        .map(|offset| start_index + offset)
        .unwrap_or_else(|| panic!("missing end marker {end:?} after {start:?}"));
    &source[start_index..end_index]
}

fn assert_contains(context: &str, source: &str, needle: &str) {
    assert!(source.contains(needle), "{context} missing {needle:?}");
}

fn assert_not_contains(context: &str, source: &str, needle: &str) {
    assert!(
        !source.contains(needle),
        "{context} must not contain {needle:?}"
    );
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf()
}
