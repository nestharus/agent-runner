pub mod support {
    pub mod contract_matrix;
}

use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use support::contract_matrix::{fixture_path, generated_rs_path, repo_root, schemas_rs_path};

const BASELINE_LINES: usize = 5270;
const BASELINE_SHA256: &str = "a2813a4ae943567430931b9f4ba87bb5867fe9b9fc32ef4db15a231ed534141d";

#[test]
fn provider_name_grep_baseline_does_not_increase() {
    let root = repo_root();
    let pattern = format!(
        "{}|{}|{}|{}",
        real_provider_token(&["cla", "ude"]),
        real_provider_token(&["cod", "ex"]),
        real_provider_token(&["gem", "ini"]),
        real_provider_token(&["open", "code"])
    );
    let output = Command::new("rg")
        .current_dir(&root)
        .args([
            "--no-heading",
            "--line-number",
            "--ignore-case",
            "--glob",
            "!src-tauri/target/**",
            "--glob",
            "!target/**",
            &pattern,
            ".",
        ])
        .output()
        .expect("rg must run for provider-name grep baseline");

    assert!(
        output.status.success() || output.status.code() == Some(1),
        "rg failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let numbered = String::from_utf8(output.stdout).expect("rg output must be UTF-8");
    let current_lines = numbered.lines().count();
    let s2_matches = numbered
        .lines()
        .filter(|line| is_s2_owned_match(line))
        .collect::<Vec<_>>();
    assert!(
        current_lines <= BASELINE_LINES || s2_matches.is_empty(),
        "provider-name grep line count increased from {BASELINE_LINES} to {current_lines}; S2-owned matches: {s2_matches:#?}"
    );

    let nonumber = numbered
        .lines()
        .map(strip_line_number)
        .collect::<Vec<_>>()
        .join("\n");
    let digest = format!("{:x}", Sha256::digest(nonumber.as_bytes()));
    if current_lines == BASELINE_LINES {
        assert_eq!(
            digest, BASELINE_SHA256,
            "provider-name grep baseline changed without a line-count change"
        );
    }
}

#[test]
fn s2_owned_files_use_only_neutral_provider_vocabulary() {
    let files = s2_owned_files();
    let denylist = [
        real_provider_token(&["cla", "ude"]),
        real_provider_token(&["cod", "ex"]),
        real_provider_token(&["gem", "ini"]),
        real_provider_token(&["open", "code"]),
    ];

    let mut violations = Vec::new();
    for path in files {
        if !path.exists() {
            continue;
        }
        let contents = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed reading {}: {err}", path.display()));
        let lower = contents.to_lowercase();
        for denied in &denylist {
            if lower.contains(denied) {
                violations.push(format!("{} contains {denied}", path.display()));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "S2-owned files must use neutral examples only: {violations:#?}"
    );
}

fn s2_owned_files() -> Vec<PathBuf> {
    let root = repo_root();
    let provider = root.join("crates/oulipoly-provider");
    let mut files = Vec::new();

    for relative in [
        "contract/v1/common.schema.json",
        "contract/v1/describe.schema.json",
        "contract/v1/schema.schema.json",
        "contract/v1/settings.schema.json",
        "contract/v1/launch.schema.json",
        "contract/v1/policy.schema.json",
        "contract/v1/quota.schema.json",
        "contract/v1/terminal.schema.json",
        "contract/v1/session.schema.json",
        "contract/v1/rotation.schema.json",
        "contract/v1/discovery.schema.json",
        "contract/v1/setup.schema.json",
        "contract/v1/migration.schema.json",
    ] {
        files.push(root.join(relative));
    }

    files.push(generated_rs_path());
    files.push(schemas_rs_path());
    files.push(fixture_path());

    collect_rs_files(&provider.join("tests"), &mut files);

    files
}

fn collect_rs_files(dir: &std::path::Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap_or_else(|err| panic!("failed reading {dir:?}: {err}")) {
        let path = entry.expect("test entry").path();
        if path.is_dir() {
            collect_rs_files(&path, files);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            files.push(path);
        }
    }
}

fn real_provider_token(parts: &[&str]) -> String {
    parts.concat()
}

fn is_s2_owned_match(line: &str) -> bool {
    let line = line.strip_prefix("./").unwrap_or(line);
    let Some(first_colon) = line.find(':') else {
        return false;
    };
    let path = &line[..first_colon];
    path.starts_with("contract/v1/")
        || path == "crates/oulipoly-provider/src/generated.rs"
        || path == "crates/oulipoly-provider/src/schemas.rs"
        || path.starts_with("crates/oulipoly-provider/tests/")
}

fn strip_line_number(line: &str) -> String {
    let line = line.strip_prefix("./").unwrap_or(line);
    let Some(first_colon) = line.find(':') else {
        return line.to_owned();
    };
    let rest = &line[first_colon + 1..];
    if rest
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_digit())
        && let Some(second_colon) = rest.find(':')
    {
        return format!("{}:{}", &line[..first_colon], &rest[second_colon + 1..]);
    }
    line.to_owned()
}
