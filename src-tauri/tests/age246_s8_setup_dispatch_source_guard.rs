//! ## Declared roles
//! - Source guards for setup brain dispatch and concrete provider vocabulary
//!   thresholds.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const MANAGER_BASELINE_PROVIDER_NAME_COUNT: usize = 5391;
const MODEL_RS_BASELINE_PROVIDER_NAME_LINE_COUNT: usize = 246;

#[test]
fn exactly_one_legacy_fallback_caller_exists_and_is_gated_by_missing_config() {
    let flow = read_source("src-tauri/src/setup/flow.rs");
    let fallback_wrapper = source_between(
        &flow,
        "async fn run_legacy_setup_brain_fallback(",
        "pub async fn run_for_cli",
    );
    let fallback_call_count = flow.matches("run_legacy_setup_brain_fallback(").count()
        - fallback_wrapper
            .matches("run_legacy_setup_brain_fallback(")
            .count();

    assert_eq!(
        fallback_call_count, 1,
        "S8 must keep exactly one production legacy fallback caller"
    );
    assert_eq!(
        flow.matches("SetupAgent::new(").count(),
        1,
        "S8 must keep exactly one hardcoded setup brain construction"
    );
    assert_contains(
        "missing-config fallback boundary",
        &flow,
        "run_missing_config_legacy_fallback",
    );

    let selection = source_between(
        &flow,
        "fn select_setup_brain_source",
        "fn execute_allowlisted",
    );
    assert_contains(
        "setup brain source selection",
        selection,
        "SetupBrainConfig",
    );
    assert_contains(
        "setup brain source selection",
        selection,
        "Option<SetupBrainConfig>",
    );
    assert_contains(
        "setup brain source selection",
        selection,
        "SetupBrainSource::Fallback",
    );
    assert_contains(
        "setup brain source selection",
        selection,
        "setup_fallback_unavailable",
    );
    assert_contains(
        "configured setup brain selection",
        &flow,
        "try_run_configured_setup_brain(&system_prompt, &initial_message)",
    );
}

#[test]
fn configured_path_uses_provider_ref_adapter_and_does_not_call_legacy() {
    let host = read_source("src-tauri/src/setup/setup_brain_host.rs");
    let flow = read_source("src-tauri/src/setup/flow.rs");
    let configured_branch = source_between(
        &flow,
        "SetupBrainSource::Configured",
        "SetupBrainSource::Fallback",
    );

    assert_contains("setup brain host", &host, "ProviderImplementationRef");
    assert_contains("setup brain host", &host, "ProviderClient");
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
    let tracked_added = tracked_added_provider_name_occurrences(&root, &pattern);
    let untracked_added = untracked_provider_name_occurrences(&root, &pattern);

    assert_eq!(
        tracked_added + untracked_added,
        0,
        "S8 files must not introduce concrete-provider vocabulary"
    );
}

#[test]
fn full_provider_name_grep_threshold_remains_within_manager_baseline() {
    let root = workspace_root();
    let pattern = concrete_provider_pattern();
    let count = full_provider_name_occurrence_count(&root, &pattern);

    assert!(
        count <= MANAGER_BASELINE_PROVIDER_NAME_COUNT,
        "full provider-name grep count {count} exceeds manager baseline {MANAGER_BASELINE_PROVIDER_NAME_COUNT}"
    );
}

fn full_provider_name_occurrence_count(root: &Path, pattern: &str) -> usize {
    let output = full_provider_name_grep_output(root, pattern, &["."]);
    assert_full_provider_name_grep_status(&output);
    let full_count = stdout_line_count(output.stdout, "git grep stdout utf8");

    let model_output =
        full_provider_name_grep_output(root, pattern, &["crates/oulipoly-config/src/model"]);
    assert_full_provider_name_grep_status(&model_output);
    let split_model_count = stdout_line_count(model_output.stdout, "git grep model stdout utf8");

    full_count.saturating_sub(split_model_count) + MODEL_RS_BASELINE_PROVIDER_NAME_LINE_COUNT
}

fn full_provider_name_grep_output(root: &Path, pattern: &str, paths: &[&str]) -> Output {
    let mut args = vec!["grep", "-iE", pattern, "--"];
    args.extend(paths);
    args.extend([
        ":(exclude)planning/*-gate/**",
        ":(exclude)planning/wu-e/**",
        ":(exclude)planning/opencode-contract/**",
        ":(exclude)planning/s10-moveout/**",
    ]);

    Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .expect("git grep must run")
}

fn assert_full_provider_name_grep_status(output: &Output) {
    assert!(
        output.status.success() || output.status.code() == Some(1),
        "git grep failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn stdout_line_count(stdout: Vec<u8>, expect_message: &str) -> usize {
    String::from_utf8(stdout)
        .expect(expect_message)
        .lines()
        .count()
}

fn tracked_added_provider_name_occurrences(root: &Path, pattern: &str) -> usize {
    let output = tracked_diff_output(root);
    assert_tracked_diff_status(&output);
    added_provider_name_occurrence_count(output.stdout, pattern)
}

fn tracked_diff_output(root: &Path) -> Output {
    Command::new("git")
        .current_dir(root)
        .args([
            "diff",
            "--unified=0",
            "--",
            ".",
            ":(exclude)planning/*-gate/**",
            ":(exclude)planning/wu-e/**",
            ":(exclude)planning/opencode-contract/**",
            ":(exclude)planning/s10-moveout/**",
        ])
        .output()
        .expect("git diff must run")
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

fn untracked_provider_name_occurrences(root: &Path, pattern: &str) -> usize {
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
        .expect("git ls-files must run");
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
        .filter(|relative| !is_ignored_source_guard_path(relative))
        .map(|relative| {
            let source = std::fs::read_to_string(root.join(relative))
                .unwrap_or_else(|error| panic!("failed to read {relative}: {error}"));
            provider_name_matches(&source, pattern).count()
        })
        .sum()
}

fn provider_name_matches<'a>(line: &'a str, pattern: &'a str) -> impl Iterator<Item = &'a str> {
    let left = &pattern[..6];
    let right = &pattern[7..];
    line.match_indices(left)
        .map(|(_, matched)| matched)
        .chain(line.match_indices(right).map(|(_, matched)| matched))
}

fn concrete_provider_pattern() -> String {
    format!(
        "{}|{}",
        ["cl", "au", "de"].concat(),
        ["co", "de", "x"].concat()
    )
}

fn is_ignored_source_guard_path(relative: &str) -> bool {
    relative.starts_with("target/")
        || relative.starts_with("src-tauri/target/")
        || is_planning_gate_artifact(relative)
        || relative.starts_with("planning/wu-e/")
        || relative.starts_with("planning/opencode-contract/")
        || relative.starts_with("planning/s10-moveout/")
}

fn is_planning_gate_artifact(relative: &str) -> bool {
    relative
        .strip_prefix("planning/")
        .and_then(|suffix| suffix.split_once('/'))
        .is_some_and(|(dir, _)| dir.ends_with("-gate"))
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
