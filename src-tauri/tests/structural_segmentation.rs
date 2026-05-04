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

#[test]
fn doomed_dirs_absent_at_head() {
    let stdout = git_stdout(&["ls-files"]);
    let entries: BTreeSet<_> = stdout
        .lines()
        .filter_map(|entry| entry.split('/').next())
        .collect();

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
    let mut violations = Vec::new();

    for tracked_entry in git_stdout(&["ls-files", "--stage"]).lines() {
        let (metadata, tracked_file) = tracked_entry
            .split_once('\t')
            .unwrap_or_else(|| panic!("unexpected git ls-files --stage output: {tracked_entry}"));
        let mode = metadata.split_whitespace().next().unwrap_or_else(|| {
            panic!("missing mode in git ls-files --stage output: {tracked_entry}")
        });

        if !mode.starts_with("100") {
            continue;
        }

        if tracked_file == SELF_PATH {
            continue;
        }

        let path = root.join(tracked_file);
        let content = fs::read(&path)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
        let content = String::from_utf8_lossy(&content);

        for (line_index, line) in content.lines().enumerate() {
            for found in regex.find_iter(line) {
                violations.push(format!(
                    "{tracked_file}:{}: {}",
                    line_index + 1,
                    found.as_str()
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "dangling doomed-dir links found:\n{}",
        violations.join("\n")
    );
}
