use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

const RAW_PATTERNS: &[&str] = &[
    "executor::execute(",
    "executor::execute_with_inputs(",
    "executor::execute_with_inputs_and_env(",
    "cli::execute(",
];

#[test]
fn raw_executor_production_callsites_match_allowlist() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let allowlist = read_allowlist(&workspace.join("tests/raw_executor_callsite_allowlist.txt"));
    let mut rust_files = Vec::new();
    collect_rust_files(&workspace.join("crates"), &mut rust_files);
    collect_rust_files(&workspace.join("src-tauri/src"), &mut rust_files);

    let mut violations = Vec::new();
    for file in rust_files {
        let rel = file
            .strip_prefix(workspace)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if is_test_path(&rel) || allowlist.contains(rel.as_str()) {
            continue;
        }
        let content = fs::read_to_string(&file).unwrap();
        let mut pending_cfg_test = false;
        let mut pending_cfg_test_item = false;
        let mut cfg_test_item_depth = 0usize;
        for (line_index, line) in content.lines().enumerate() {
            let trimmed = line.trim_start();
            if cfg_test_item_depth > 0 {
                cfg_test_item_depth = update_brace_depth(cfg_test_item_depth, line);
                continue;
            }
            if pending_cfg_test_item {
                pending_cfg_test_item = !line.contains('{') && !line.contains(';');
                cfg_test_item_depth = update_brace_depth(0, line);
                continue;
            }
            if pending_cfg_test {
                if trimmed.starts_with("#[") || trimmed.is_empty() {
                    continue;
                }
                cfg_test_item_depth = update_brace_depth(0, line);
                pending_cfg_test_item = !line.contains('{') && !line.contains(';');
                pending_cfg_test = false;
                continue;
            }
            if is_cfg_test_attr(trimmed) {
                pending_cfg_test = true;
                continue;
            }
            if RAW_PATTERNS.iter().any(|pattern| line.contains(pattern)) {
                violations.push(format!("{}:{}: {}", rel, line_index + 1, line.trim()));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "raw executor production callsites outside allowlist:\n{}",
        violations.join("\n")
    );
}

fn read_allowlist(path: &Path) -> HashSet<String> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(ToOwned::to_owned)
        .collect()
}

fn collect_rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    if !dir.exists() {
        return;
    }
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect_rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

fn is_test_path(rel: &str) -> bool {
    rel.contains("/tests/")
        || rel.ends_with("_test.rs")
        || rel.ends_with("-test.rs")
        || rel.ends_with("tests.rs")
}

fn is_cfg_test_attr(trimmed: &str) -> bool {
    trimmed == "#[cfg(test)]"
}

fn update_brace_depth(depth: usize, line: &str) -> usize {
    let opens = line.chars().filter(|ch| *ch == '{').count();
    let closes = line.chars().filter(|ch| *ch == '}').count();
    depth.saturating_add(opens).saturating_sub(closes)
}
