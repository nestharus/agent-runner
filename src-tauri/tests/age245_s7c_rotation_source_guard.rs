//! Declared roles: accessor, predicate, validator.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BASELINE_COMMIT: &str = "8718b547557cddfc79c47f30a0146fd7d994c00b";
const MANAGER_BASELINE_PROVIDER_NAME_COUNT: usize = 5391;

#[test]
fn s7c_production_wiring_passes_registry_handle_into_migration_service() {
    let wiring = read_source("src-tauri/src/wiring.rs");
    let cli_defaults = source_between(
        &wiring,
        "pub fn cli_defaults() -> Self",
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
    let resume_mapper = read_source("src-tauri/src/run/resume/mapper.rs");
    let repl = read_source("src-tauri/src/run/repl/orchestration.rs");
    for (context, source) in [
        ("run/resume/orchestration.rs", resume.as_str()),
        ("run/resume/mapper.rs", resume_mapper.as_str()),
        ("run/repl/orchestration.rs", repl.as_str()),
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
        "run/repl/orchestration.rs",
        &repl,
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
        MANAGER_BASELINE_PROVIDER_NAME_COUNT, 5391,
        "AGE-245 S7c must preserve the manager-approved provider-name baseline"
    );
    let current_count = provider_name_occurrence_count(&root, &pattern);
    assert!(
        current_count <= MANAGER_BASELINE_PROVIDER_NAME_COUNT,
        "provider-name invariant found {current_count} current occurrence(s), above manager baseline {MANAGER_BASELINE_PROVIDER_NAME_COUNT}"
    );
    let tracked_added_count =
        tracked_added_provider_name_occurrences_since_baseline(&root, &pattern);
    let untracked_added_count = untracked_provider_name_occurrence_count(&root, &pattern);
    let count = tracked_added_count + untracked_added_count;
    assert!(
        count == 0,
        "provider-name invariant found {count} new provider-name occurrence(s) after AGE-245 baseline"
    );
}

fn tracked_added_provider_name_occurrences_since_baseline(root: &Path, pattern: &str) -> usize {
    let output = tracked_diff_output_since_baseline(root);
    assert_tracked_diff_status(&output);
    added_provider_name_occurrence_count(output.stdout, pattern)
}

fn tracked_diff_output_since_baseline(root: &Path) -> Output {
    Command::new("git")
        .current_dir(root)
        .args([
            "diff",
            "--unified=0",
            "--find-renames=80%",
            BASELINE_COMMIT,
            "--",
            ".",
            ":(exclude)src-tauri/target/**",
            ":(exclude)target/**",
            ":(exclude)planning/*-gate/**",
            ":(exclude)planning/wu-e/**",
            ":(exclude)planning/opencode-contract/**",
            ":(exclude)planning/s10-moveout/**",
            ":(exclude)crates/oulipoly-state/src/db/tests.rs",
        ])
        .output()
        .expect("git diff must run for AGE-245 S7c provider-name invariant")
}

fn assert_tracked_diff_status(output: &Output) {
    assert!(
        output.status.success(),
        "git diff failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn added_provider_name_occurrence_count(stdout: Vec<u8>, pattern: &str) -> usize {
    String::from_utf8(stdout)
        .expect("git diff stdout utf8")
        .lines()
        .filter(|line| line.starts_with('+') && !line.starts_with("+++"))
        .flat_map(|line| provider_name_matches(line, pattern))
        .count()
}

fn provider_name_occurrence_count(root: &Path, pattern: &str) -> usize {
    let output = provider_name_occurrence_output(root, pattern);
    assert_provider_name_occurrence_status(&output);
    stdout_line_count(output.stdout, "rg stdout utf8")
}

fn provider_name_occurrence_output(root: &Path, pattern: &str) -> Output {
    Command::new("rg")
        .current_dir(root)
        .args([
            "--hidden",
            "-o",
            pattern,
            "-g",
            "!src-tauri/target/**",
            "-g",
            "!target/**",
            "-g",
            "!planning/*-gate/**",
            "-g",
            "!planning/wu-e/**",
            "-g",
            "!planning/opencode-contract/**",
            "-g",
            "!planning/s10-moveout/**",
        ])
        .output()
        .expect("rg must run for AGE-245 S7c provider-name invariant")
}

fn assert_provider_name_occurrence_status(output: &Output) {
    assert!(
        output.status.success() || output.status.code() == Some(1),
        "rg failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn stdout_line_count(stdout: Vec<u8>, expect_message: &str) -> usize {
    String::from_utf8(stdout)
        .expect(expect_message)
        .lines()
        .count()
}

fn untracked_provider_name_occurrence_count(root: &Path, pattern: &str) -> usize {
    let output = Command::new("git")
        .current_dir(root)
        .args([
            "ls-files",
            "--others",
            "--exclude-standard",
            "-z",
            "--",
            ".",
        ])
        .output()
        .expect("git ls-files must run for AGE-245 S7c provider-name invariant");
    assert!(
        output.status.success(),
        "git ls-files failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .filter_map(|relative| std::str::from_utf8(relative).ok())
        .filter(|relative| !is_ignored_generated_path(relative))
        .map(|relative| {
            let bytes = std::fs::read(root.join(relative))
                .unwrap_or_else(|error| panic!("read untracked source {relative}: {error}"));
            provider_name_matches(&String::from_utf8_lossy(&bytes), pattern).count()
        })
        .sum()
}

fn is_ignored_generated_path(relative: &str) -> bool {
    relative.starts_with("src-tauri/target/")
        || relative.starts_with("target/")
        || is_planning_gate_artifact(relative)
        || relative.starts_with("planning/wu-e/")
        || relative.starts_with("planning/opencode-contract/")
        || relative.starts_with("planning/s10-moveout/")
        || is_relocated_state_db_test_body(relative)
}

fn is_relocated_state_db_test_body(relative: &str) -> bool {
    relative == "crates/oulipoly-state/src/db/tests.rs"
}

fn is_planning_gate_artifact(relative: &str) -> bool {
    relative
        .strip_prefix("planning/")
        .and_then(|suffix| suffix.split_once('/'))
        .is_some_and(|(dir, _)| dir.ends_with("-gate"))
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

fn provider_name_matches<'a>(line: &'a str, pattern: &'a str) -> impl Iterator<Item = &'a str> {
    let left = &pattern[..6];
    let right = &pattern[7..];
    line.match_indices(left)
        .map(|(_, matched)| matched)
        .chain(line.match_indices(right).map(|(_, matched)| matched))
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf()
}
