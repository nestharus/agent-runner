//! ## Declared roles
//!
//! Roles: orchestration, formatter, mapper, accessor, parser, validator, predicate, filter.
//!
//! TEST: structural segmentation fixtures and tracked-file lint helpers: git inventory accessors/parsers, regex/link formatters, tracked-path predicates
//! and filters, file-content scanners, and assertion validators for doomed-dir
//! absence and dangling-link rejection.
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: src-tauri/tests/structural_segmentation.rs
//!     role: intrinsic-surface
//!     Domain: structural doomed-directory lint
//!     Owns:
//!       - git tracked-file inventory
//!       - doomed-directory link regex matching
//!       - tracked-file scan allowlist
//!       - line-level violation reporting
//!       - structural lint assertions
//! ```

use regex::Regex;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const DOOMED_DIRS: [&str; 6] = [
    "initiatives",
    "proposals",
    "research",
    "review",
    "risk",
    "product-strategy",
];

const SELF_PATH: &str = "src-tauri/tests/structural_segmentation.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn git(args: &[&str]) -> Output {
    Command::new("git")
        .args(args)
        .current_dir(repo_root())
        .output()
        .unwrap_or_else(|_| panic!("BLOCKED: git not on PATH"))
}

fn git_stdout(args: &[&str]) -> String {
    let output = git(args);
    assert_git_success(args, &output);
    parse_stdout(output)
}

fn assert_git_success(args: &[&str], output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        git_failure_message(args, &git_stderr(output))
    );
}

fn git_failure_message(args: &[&str], stderr: &str) -> String {
    format!("git {args:?} failed: {stderr}")
}

fn git_stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn parse_stdout(output: Output) -> String {
    String::from_utf8(output.stdout).unwrap()
}

fn doomed_dir_link_regex() -> Regex {
    compile_regex(&doomed_dir_link_pattern())
}

fn doomed_dir_link_pattern() -> String {
    let boundary = r"(^|[^[:alnum:]_./~-])";
    let dirs = join_doomed_dir_parts(escaped_doomed_dir_parts());
    let path = r"/[A-Za-z0-9._~@%+=:,;/-]*\.[A-Za-z0-9][A-Za-z0-9._~@%+=:,;/-]*";
    format_doomed_dir_link_pattern(boundary, &dirs, path)
}

fn escaped_doomed_dir_parts() -> Vec<String> {
    DOOMED_DIRS.iter().map(|dir| regex::escape(dir)).collect()
}

fn join_doomed_dir_parts(dirs: Vec<String>) -> String {
    dirs.join("|")
}

fn format_doomed_dir_link_pattern(boundary: &str, dirs: &str, path: &str) -> String {
    format!("{boundary}({dirs}){path}")
}

fn compile_regex(pattern: &str) -> Regex {
    Regex::new(pattern).unwrap()
}

fn top_level_tracked_entries() -> BTreeSet<String> {
    collect_top_level_entries(&git_stdout(&["ls-files"]))
}

fn collect_top_level_entries(stdout: &str) -> BTreeSet<String> {
    own_top_level_entries(parse_top_level_entries(stdout))
}

fn parse_top_level_entries(stdout: &str) -> Vec<&str> {
    stdout.lines().map(top_level_entry).collect()
}

fn own_top_level_entries(entries: Vec<&str>) -> BTreeSet<String> {
    entries.into_iter().map(str::to_owned).collect()
}

fn top_level_entry(entry: &str) -> &str {
    entry.split('/').next().unwrap_or(entry)
}

fn stage_entry_path(tracked_entry: &str) -> (&str, &str) {
    let (metadata, tracked_file) =
        require_stage_entry_parts(parse_stage_entry_parts(tracked_entry), tracked_entry);
    let mode = require_stage_mode(parse_stage_mode(metadata), tracked_entry);

    (mode, tracked_file)
}

fn parse_stage_entry_parts(tracked_entry: &str) -> Option<(&str, &str)> {
    tracked_entry.split_once('\t')
}

fn require_stage_entry_parts<'a>(
    parts: Option<(&'a str, &'a str)>,
    tracked_entry: &str,
) -> (&'a str, &'a str) {
    parts.unwrap_or_else(|| panic!("{}", unexpected_stage_entry_message(tracked_entry)))
}

fn unexpected_stage_entry_message(tracked_entry: &str) -> String {
    format!("unexpected git ls-files --stage output: {tracked_entry}")
}

fn parse_stage_mode(metadata: &str) -> Option<&str> {
    metadata.split_whitespace().next()
}

fn require_stage_mode<'a>(mode: Option<&'a str>, tracked_entry: &str) -> &'a str {
    mode.unwrap_or_else(|| panic!("{}", missing_stage_mode_message(tracked_entry)))
}

fn missing_stage_mode_message(tracked_entry: &str) -> String {
    format!("missing mode in git ls-files --stage output: {tracked_entry}")
}

fn is_regular_blob(mode: &str) -> bool {
    mode.starts_with("100")
}

fn tracked_regular_files() -> Vec<String> {
    collect_existing_regular_files(&repo_root(), &git_stdout(&["ls-files", "--stage"]))
}

fn collect_existing_regular_files(root: &Path, stdout: &str) -> Vec<String> {
    let existing_regular_paths = existing_regular_stage_paths(root, stdout);
    owned_tracked_files(existing_regular_paths)
}

fn existing_regular_stage_paths<'a>(root: &Path, stdout: &'a str) -> Vec<&'a str> {
    regular_stage_paths(stdout)
        .into_iter()
        .filter(|tracked_file| tracked_file_exists(root, tracked_file))
        .collect()
}

fn tracked_file_exists(root: &Path, tracked_file: &str) -> bool {
    path_exists(&tracked_file_path(root, tracked_file))
}

fn tracked_file_path(root: &Path, tracked_file: &str) -> PathBuf {
    root.join(tracked_file)
}

fn path_exists(path: &Path) -> bool {
    path.exists()
}

fn owned_tracked_files(tracked_files: Vec<&str>) -> Vec<String> {
    tracked_files.into_iter().map(str::to_owned).collect()
}

fn regular_stage_paths(stdout: &str) -> Vec<&str> {
    stdout.lines().filter_map(regular_stage_path).collect()
}

fn regular_stage_path(tracked_entry: &str) -> Option<&str> {
    let (mode, tracked_file) = stage_entry_path(tracked_entry);
    is_regular_blob(mode).then_some(tracked_file)
}

fn is_quoted_gate_diff_artifact(tracked_file: &str) -> bool {
    tracked_file.starts_with("planning/") && tracked_file.ends_with(".patch")
}

fn is_code_quality_log_artifact(tracked_file: &str) -> bool {
    tracked_file.starts_with("planning/")
        && tracked_file.contains("/code-quality/")
        && tracked_file.ends_with(".log")
}

fn should_scan_for_doomed_dir_links(tracked_file: &str) -> bool {
    tracked_file != SELF_PATH
        && !is_quoted_gate_diff_artifact(tracked_file)
        && !is_code_quality_log_artifact(tracked_file)
}

fn line_doomed_dir_links<'a>(regex: &'a Regex, line: &'a str) -> impl Iterator<Item = &'a str> {
    doomed_dir_link_matches(regex, line).map(doomed_dir_match_str)
}

fn doomed_dir_link_matches<'a>(regex: &'a Regex, line: &'a str) -> regex::Matches<'a, 'a> {
    regex.find_iter(line)
}

fn doomed_dir_match_str(found: regex::Match<'_>) -> &str {
    found.as_str()
}

fn file_doomed_dir_link_violations(root: &Path, regex: &Regex, tracked_file: &str) -> Vec<String> {
    let path = root.join(tracked_file);
    let content = read_tracked_file(&path);
    let content = decode_tracked_file(&content);

    collect_line_violations(regex, tracked_file, &content)
}

fn read_tracked_file(path: &Path) -> Vec<u8> {
    fs::read(path).unwrap_or_else(|err| panic!("{}", read_tracked_file_message(path, &err)))
}

fn read_tracked_file_message(path: &Path, err: &std::io::Error) -> String {
    format!("failed to read {}: {err}", path.display())
}

fn decode_tracked_file(content: &[u8]) -> String {
    String::from_utf8_lossy(content).into_owned()
}

fn collect_line_violations(regex: &Regex, tracked_file: &str, content: &str) -> Vec<String> {
    numbered_lines(content)
        .into_iter()
        .flat_map(|(line_index, line)| line_violations(regex, tracked_file, line_index, line))
        .collect()
}

fn numbered_lines(content: &str) -> Vec<(usize, &str)> {
    content
        .lines()
        .enumerate()
        .map(|(line_index, line)| (line_index + 1, line))
        .collect()
}

fn line_violations(
    regex: &Regex,
    tracked_file: &str,
    line_number: usize,
    line: &str,
) -> Vec<String> {
    format_doomed_dir_link_violations(
        tracked_file,
        line_number,
        line_doomed_dir_links(regex, line),
    )
}

fn format_doomed_dir_link_violations<'a>(
    tracked_file: &str,
    line_number: usize,
    links: impl Iterator<Item = &'a str>,
) -> Vec<String> {
    links
        .map(|found| doomed_dir_link_violation(tracked_file, line_number, found))
        .collect()
}

fn doomed_dir_link_violation(tracked_file: &str, line_number: usize, found: &str) -> String {
    format!("{tracked_file}:{line_number}: {found}")
}

fn tracked_doomed_dir_link_violations(root: &Path, regex: &Regex) -> Vec<String> {
    eligible_tracked_files(tracked_regular_files())
        .into_iter()
        .flat_map(|tracked_file| file_doomed_dir_link_violations(root, regex, &tracked_file))
        .collect()
}

fn eligible_tracked_files(tracked_files: Vec<String>) -> Vec<String> {
    tracked_files
        .into_iter()
        .filter(|tracked_file| should_scan_for_doomed_dir_links(tracked_file))
        .collect()
}

#[test]
fn doomed_dirs_absent_at_head() {
    let entries = top_level_tracked_entries();

    for doomed_dir in DOOMED_DIRS {
        assert!(
            !entries.contains(doomed_dir),
            "{doomed_dir} is still present as a top-level tracked directory"
        );
    }
}

#[test]
fn no_dangling_doomed_dir_link_in_tracked_files() {
    let root = repo_root();
    let regex = doomed_dir_link_regex();
    let violations = tracked_doomed_dir_link_violations(&root, &regex);

    assert!(
        violations.is_empty(),
        "dangling doomed-dir links found:\n{}",
        violations.join("\n")
    );
}
