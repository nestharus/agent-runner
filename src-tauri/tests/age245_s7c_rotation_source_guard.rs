//! Declared roles: accessor, predicate, validator.


use std::path::{Path, PathBuf};
#[path = "../../tests/support/provider_vocabulary_source_set.rs"]
mod provider_vocabulary_source_set;
use provider_vocabulary_source_set::SourceSet;

const BASELINE_COMMIT: &str = "f0844a90d73c9196fc6fe53d510caf4d2c56c076";
const MANAGER_BASELINE_PROVIDER_NAME_COUNT: usize = 4628;

#[test]
fn s7c_production_wiring_passes_registry_handle_into_migration_service() {
    let wiring = read_source("src-tauri/src/wiring.rs");
    let cli_defaults = source_between(
        &wiring,
        "pub fn cli_defaults() -> Result<Self, String>",
        "pub fn production(",
    );
    let production = source_between(
        &wiring,
        "pub fn production(",
        "fn default_cli_runtime_paths(",
    );

    for (context, body) in [
        ("wiring.rs::cli_defaults", cli_defaults),
        ("wiring.rs::production", production),
    ] {
        let migration_service_constructor = collapse_rust_whitespace(
            "ProductionMigrationService::with_registry_handle(provider_registry_handle.clone(),",
        );
        assert_contains(
            context,
            body,
            "let provider_registry_handle = ProviderRegistryHandle::new(",
        );
        assert_contains(
            context,
            &collapse_rust_whitespace(body),
            &migration_service_constructor,
        );
    }
}

#[test]
fn s7c_resume_and_repl_delegate_rotation_identity_to_runtime_service_only() {
    let resume = read_source("src-tauri/src/run/resume/orchestration.rs");
    let resume_execution = read_source("src-tauri/src/run/resume/execution.rs");
    let resume_migration = read_source("src-tauri/src/run/resume/migration.rs");
    let resume_mapper = read_source("src-tauri/src/run/resume/mapper.rs");
    let repl = read_source("src-tauri/src/run/repl/orchestration.rs");
    let repl_mapper = read_source("src-tauri/src/run/repl/mapper.rs");
    let repl_migration = read_source("src-tauri/src/run/repl/migration.rs");
    for (context, source) in [
        ("run/resume/orchestration.rs", resume.as_str()),
        ("run/resume/execution.rs", resume_execution.as_str()),
        ("run/resume/migration.rs", resume_migration.as_str()),
        ("run/resume/mapper.rs", resume_mapper.as_str()),
        ("run/repl/orchestration.rs", repl.as_str()),
        ("run/repl/mapper.rs", repl_mapper.as_str()),
        ("run/repl/migration.rs", repl_migration.as_str()),
    ] {
        assert_not_contains(context, source, "external_provider");
        assert_not_contains(
            context,
            source,
            "resolve_rotation_external_provider_identity",
        );
        assert_not_contains(context, source, "ProviderClient");
        assert_not_contains(context, source, "invoke_typed");
        assert_not_contains(context, source, "describe_model_provider");
    }
    assert_contains(
        "run/resume/mapper.rs",
        &resume_mapper,
        "MigrationServiceRequest",
    );
    assert_contains(
        "run/repl/mapper.rs",
        &repl_mapper,
        "MigrationServiceRequest",
    );
}

#[test]
fn s7c_legacy_migration_commands_remain_distinct_from_provider_migration_plan_apply() {
    for relative in [
        "src-tauri/src/commands/migrate",
        "src-tauri/src/commands/config_migration",
    ] {
        for source in read_sources_under(relative) {
            assert_not_contains(relative, &source, "\"migration.plan\"");
            assert_not_contains(relative, &source, "\"migration.apply\"");
            assert_not_contains(relative, &source, "ProviderClient");
            assert_not_contains(relative, &source, "host_state_plan");
        }
    }

    let settings = read_source("crates/oulipoly-runtime/src/provider_settings/mod.rs");
    assert_not_contains("provider_settings/mod.rs", &settings, "\"migration.plan\"");
    assert_not_contains("provider_settings/mod.rs", &settings, "\"migration.apply\"");
}

#[test]
fn s7c_provider_name_grep_invariant_uses_authoritative_manager_baseline() {
    let root = workspace_root();
    let pattern = format!(
        "{}|{}",
        real_provider_token(&["cla", "ude"]),
        real_provider_token(&["cod", "ex"])
    );
    assert_eq!(
        MANAGER_BASELINE_PROVIDER_NAME_COUNT, 4628,
        "AGE-245 S7c must preserve the manager-approved provider-name baseline"
    );
    let sources = SourceSet::load(&root);
    let current_count = sources.full_occurrences(&pattern);
    let count = sources.added_occurrences(Some(BASELINE_COMMIT), &pattern);
    assert!(
        current_count <= MANAGER_BASELINE_PROVIDER_NAME_COUNT,
        "provider-name invariant found {current_count} current occurrence(s), above manager baseline {MANAGER_BASELINE_PROVIDER_NAME_COUNT}"
    );
    assert!(
        count == 0,
        "provider-name invariant found {count} new provider-name occurrence(s) after AGE-245 baseline"
    );
}

fn read_sources_under(relative: &str) -> Vec<String> {
    let root = workspace_root().join(relative);
    if !root.exists() {
        return Vec::new();
    }
    let mut files = Vec::new();
    collect_rs_files(&root, &mut files);
    files.into_iter().map(read_source_path).collect()
}

fn collect_rs_files(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|error| panic!("read {dir:?}: {error}")) {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            collect_rs_files(&path, files);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            files.push(path);
        }
    }
}

fn read_source(relative: &str) -> String {
    read_source_path(workspace_root().join(relative))
}

fn read_source_path(path: PathBuf) -> String {
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
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
    assert!(
        source.contains(needle),
        "{context} must contain {needle:?} for AGE-245 S7c source guard"
    );
}

fn collapse_rust_whitespace(source: &str) -> String {
    source.split_whitespace().collect::<String>()
}

fn assert_not_contains(context: &str, source: &str, needle: &str) {
    assert!(
        !source.contains(needle),
        "{context} must not contain {needle:?} for AGE-245 S7c boundary guard"
    );
}

fn real_provider_token(parts: &[&str]) -> String {
    parts.concat()
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf()
}
