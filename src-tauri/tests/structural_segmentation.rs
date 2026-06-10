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
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn doomed_dir_link_regex() -> Regex {
    let boundary = r"(^|[^[:alnum:]_./~-])";
    let dirs = DOOMED_DIRS
        .iter()
        .map(|dir| regex::escape(dir))
        .collect::<Vec<_>>()
        .join("|");
    let path = r"/[A-Za-z0-9._~@%+=:,;/-]*\.[A-Za-z0-9][A-Za-z0-9._~@%+=:,;/-]*";
    Regex::new(&format!("{boundary}({dirs}){path}")).unwrap()
}

fn top_level_tracked_entries() -> BTreeSet<String> {
    git_stdout(&["ls-files"])
        .lines()
        .filter_map(|entry| entry.split('/').next())
        .map(str::to_owned)
        .collect()
}

fn stage_entry_path(tracked_entry: &str) -> (&str, &str) {
    let (metadata, tracked_file) = tracked_entry
        .split_once('\t')
        .unwrap_or_else(|| panic!("unexpected git ls-files --stage output: {tracked_entry}"));
    let mode = metadata
        .split_whitespace()
        .next()
        .unwrap_or_else(|| panic!("missing mode in git ls-files --stage output: {tracked_entry}"));

    (mode, tracked_file)
}

fn is_regular_blob(mode: &str) -> bool {
    mode.starts_with("100")
}

fn tracked_regular_files() -> Vec<String> {
    git_stdout(&["ls-files", "--stage"])
        .lines()
        .filter_map(|tracked_entry| {
            let (mode, tracked_file) = stage_entry_path(tracked_entry);
            is_regular_blob(mode).then(|| tracked_file.to_owned())
        })
        .collect()
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
    regex.find_iter(line).map(|found| found.as_str())
}

fn file_doomed_dir_link_violations(root: &Path, regex: &Regex, tracked_file: &str) -> Vec<String> {
    let path = root.join(tracked_file);
    let content =
        fs::read(&path).unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
    let content = String::from_utf8_lossy(&content);

    content
        .lines()
        .enumerate()
        .flat_map(|(line_index, line)| {
            line_doomed_dir_links(regex, line)
                .map(move |found| format!("{tracked_file}:{}: {found}", line_index + 1))
        })
        .collect()
}

fn tracked_doomed_dir_link_violations(root: &Path, regex: &Regex) -> Vec<String> {
    tracked_regular_files()
        .into_iter()
        .filter(|tracked_file| should_scan_for_doomed_dir_links(tracked_file))
        .flat_map(|tracked_file| file_doomed_dir_link_violations(root, regex, &tracked_file))
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
