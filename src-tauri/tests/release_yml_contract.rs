use serde_yaml_ng::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

#[test]
fn release_yml_restores_windows_and_target_suffixed_bare_binaries() {
    let workflow_path = Path::new("../.github/workflows/release.yml");
    let workflow_body = fs::read_to_string(workflow_path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", workflow_path.display()));
    let workflow: Value = serde_yaml_ng::from_str(&workflow_body)
        .unwrap_or_else(|err| panic!("failed to parse {}: {err}", workflow_path.display()));

    let matrix_include = sequence_at(
        &workflow,
        "jobs.build.strategy.matrix.include",
        &["jobs", "build", "strategy", "matrix", "include"],
    );
    assert_eq!(
        matrix_include.len(),
        3,
        "jobs.build.strategy.matrix.include must contain exactly 3 entries"
    );

    let expected_matrix = BTreeSet::from([
        (
            "ubuntu-latest".to_string(),
            "x86_64-unknown-linux-gnu".to_string(),
            "deb".to_string(),
        ),
        (
            "macos-latest".to_string(),
            "aarch64-apple-darwin".to_string(),
            "dmg".to_string(),
        ),
        (
            "windows-latest".to_string(),
            "x86_64-pc-windows-msvc".to_string(),
            "msi,nsis".to_string(),
        ),
    ]);
    let actual_matrix = matrix_entries(matrix_include);
    assert_eq!(
        actual_matrix, expected_matrix,
        "jobs.build.strategy.matrix.include entries must match the release target contract"
    );

    let build_steps = sequence_at(&workflow, "jobs.build.steps", &["jobs", "build", "steps"]);
    let linux = step_by_name(build_steps, "Collect artifacts (Linux)");
    assert_eq!(
        string_field(
            linux,
            "if",
            "jobs.build.steps[Collect artifacts (Linux)].if"
        ),
        "runner.os == 'Linux'",
        "jobs.build.steps[Collect artifacts (Linux)].if must be runner.os == 'Linux'"
    );
    assert!(
        string_field(
            linux,
            "run",
            "jobs.build.steps[Collect artifacts (Linux)].run"
        )
        .contains("oulipoly-agent-runner-${{ matrix.target }}"),
        "jobs.build.steps[Collect artifacts (Linux)].run must target-suffix the bare binary"
    );
    assert!(
        !string_field(
            linux,
            "run",
            "jobs.build.steps[Collect artifacts (Linux)].run"
        )
        .contains("oulipoly-agent-runner-${{ matrix.target }}.exe"),
        "jobs.build.steps[Collect artifacts (Linux)].run must not use the Windows .exe suffix"
    );
    assert!(
        string_field(
            linux,
            "run",
            "jobs.build.steps[Collect artifacts (Linux)].run"
        )
        .contains("bundle/deb/*.deb"),
        "jobs.build.steps[Collect artifacts (Linux)].run must copy the Linux bundle glob bundle/deb/*.deb"
    );
    assert!(
        !string_field(
            linux,
            "run",
            "jobs.build.steps[Collect artifacts (Linux)].run"
        )
        .contains("bundle/dmg"),
        "jobs.build.steps[Collect artifacts (Linux)].run must not contain forbidden macOS bundle substring bundle/dmg"
    );
    assert!(
        !string_field(
            linux,
            "run",
            "jobs.build.steps[Collect artifacts (Linux)].run"
        )
        .contains("bundle/msi"),
        "jobs.build.steps[Collect artifacts (Linux)].run must not contain forbidden Windows bundle substring bundle/msi"
    );

    let macos = step_by_name(build_steps, "Collect artifacts (macOS)");
    assert_eq!(
        string_field(
            macos,
            "if",
            "jobs.build.steps[Collect artifacts (macOS)].if"
        ),
        "runner.os == 'macOS'",
        "jobs.build.steps[Collect artifacts (macOS)].if must be runner.os == 'macOS'"
    );
    assert!(
        string_field(
            macos,
            "run",
            "jobs.build.steps[Collect artifacts (macOS)].run"
        )
        .contains("oulipoly-agent-runner-${{ matrix.target }}"),
        "jobs.build.steps[Collect artifacts (macOS)].run must target-suffix the bare binary"
    );
    assert!(
        string_field(
            macos,
            "run",
            "jobs.build.steps[Collect artifacts (macOS)].run"
        )
        .contains("bundle/dmg/*.dmg"),
        "jobs.build.steps[Collect artifacts (macOS)].run must copy the macOS bundle glob bundle/dmg/*.dmg"
    );
    assert!(
        !string_field(
            macos,
            "run",
            "jobs.build.steps[Collect artifacts (macOS)].run"
        )
        .contains("bundle/deb"),
        "jobs.build.steps[Collect artifacts (macOS)].run must not contain forbidden Linux bundle substring bundle/deb"
    );
    assert!(
        !string_field(
            macos,
            "run",
            "jobs.build.steps[Collect artifacts (macOS)].run"
        )
        .contains("bundle/msi"),
        "jobs.build.steps[Collect artifacts (macOS)].run must not contain forbidden Windows bundle substring bundle/msi"
    );

    let windows = step_by_name(build_steps, "Collect artifacts (Windows)");
    assert_eq!(
        string_field(
            windows,
            "if",
            "jobs.build.steps[Collect artifacts (Windows)].if"
        ),
        "runner.os == 'Windows'",
        "jobs.build.steps[Collect artifacts (Windows)].if must be runner.os == 'Windows'"
    );
    assert!(
        string_field(
            windows,
            "run",
            "jobs.build.steps[Collect artifacts (Windows)].run"
        )
        .contains("oulipoly-agent-runner-${{ matrix.target }}.exe"),
        "jobs.build.steps[Collect artifacts (Windows)].run must target-suffix the Windows bare binary with .exe"
    );
    assert!(
        string_field(
            windows,
            "run",
            "jobs.build.steps[Collect artifacts (Windows)].run"
        )
        .contains("bundle/msi/*.msi"),
        "jobs.build.steps[Collect artifacts (Windows)].run must copy the Windows MSI bundle glob bundle/msi/*.msi"
    );
    assert!(
        string_field(
            windows,
            "run",
            "jobs.build.steps[Collect artifacts (Windows)].run"
        )
        .contains("bundle/nsis/*.exe"),
        "jobs.build.steps[Collect artifacts (Windows)].run must copy the Windows NSIS bundle glob bundle/nsis/*.exe"
    );
    assert!(
        !string_field(
            windows,
            "run",
            "jobs.build.steps[Collect artifacts (Windows)].run"
        )
        .contains("bundle/deb"),
        "jobs.build.steps[Collect artifacts (Windows)].run must not contain forbidden Linux bundle substring bundle/deb"
    );
    assert!(
        !string_field(
            windows,
            "run",
            "jobs.build.steps[Collect artifacts (Windows)].run"
        )
        .contains("bundle/dmg"),
        "jobs.build.steps[Collect artifacts (Windows)].run must not contain forbidden macOS bundle substring bundle/dmg"
    );

    let upload = step_by_uses(build_steps, "actions/upload-artifact@v4");
    assert_eq!(
        string_at(
            upload,
            "jobs.build.steps[actions/upload-artifact@v4].with.name",
            &["with", "name"],
        ),
        "${{ matrix.target }}",
        "jobs.build.steps[actions/upload-artifact@v4].with.name must be the matrix target"
    );
    let upload_path = string_at(
        upload,
        "jobs.build.steps[actions/upload-artifact@v4].with.path",
        &["with", "path"],
    );
    assert!(
        upload_path == "artifacts/*" || upload_path == "artifacts",
        "jobs.build.steps[actions/upload-artifact@v4].with.path must be artifacts/* or artifacts, got {upload_path:?}"
    );

    let release_steps = sequence_at(
        &workflow,
        "jobs.release.steps",
        &["jobs", "release", "steps"],
    );
    let download = step_by_uses(release_steps, "actions/download-artifact@v4");
    assert_eq!(
        bool_at(
            download,
            "jobs.release.steps[actions/download-artifact@v4].with.merge-multiple",
            &["with", "merge-multiple"],
        ),
        true,
        "jobs.release.steps[actions/download-artifact@v4].with.merge-multiple must be true"
    );
    assert_eq!(
        string_at(
            download,
            "jobs.release.steps[actions/download-artifact@v4].with.path",
            &["with", "path"],
        ),
        "artifacts",
        "jobs.release.steps[actions/download-artifact@v4].with.path must be artifacts"
    );

    let gh_release = step_by_uses(release_steps, "softprops/action-gh-release@v2");
    let files = string_at(
        gh_release,
        "jobs.release.steps[softprops/action-gh-release@v2].with.files",
        &["with", "files"],
    );
    let actual_files = files
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        actual_files,
        BTreeSet::from([
            "artifacts/*".to_string(),
            "scripts/anthropic-usage".to_string(),
            "scripts/chatgpt-usage".to_string(),
            "scripts/claude-code-locate-transcript".to_string(),
            "scripts/claude-code-turns".to_string(),
            "scripts/codex-locate-transcript".to_string(),
            "scripts/codex-turns".to_string(),
            "scripts/zai-usage".to_string()
        ]),
        "jobs.release.steps[softprops/action-gh-release@v2].with.files must contain exactly the required release assets"
    );

    let bare_binary_hits = collect_step_run_bare_binary_hits(build_steps);
    assert_eq!(
        bare_binary_hits,
        BTreeSet::from([
            "Collect artifacts (Linux)".to_string(),
            "Collect artifacts (macOS)".to_string(),
            "Collect artifacts (Windows)".to_string()
        ]),
        "oulipoly-agent-runner-${{ matrix.target }} must appear only in Linux, macOS, and Windows collect steps"
    );
}

fn matrix_entries(include: &[Value]) -> BTreeSet<(String, String, String)> {
    include
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            (
                string_field(
                    entry,
                    "os",
                    &format!("jobs.build.strategy.matrix.include[{index}].os"),
                )
                .to_string(),
                string_field(
                    entry,
                    "target",
                    &format!("jobs.build.strategy.matrix.include[{index}].target"),
                )
                .to_string(),
                string_field(
                    entry,
                    "bundles",
                    &format!("jobs.build.strategy.matrix.include[{index}].bundles"),
                )
                .to_string(),
            )
        })
        .collect()
}

fn collect_step_run_bare_binary_hits(steps: &[Value]) -> BTreeSet<String> {
    steps
        .iter()
        .filter_map(|step| {
            let run = mapping_get(step, "run").and_then(Value::as_str)?;
            if !run.contains("oulipoly-agent-runner-${{ matrix.target }}") {
                return None;
            }
            mapping_get(step, "name")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

fn step_by_name<'a>(steps: &'a [Value], name: &str) -> &'a Value {
    steps
        .iter()
        .find(|step| mapping_get(step, "name").and_then(Value::as_str) == Some(name))
        .unwrap_or_else(|| panic!("jobs.build.steps must contain step named {name:?}"))
}

fn step_by_uses<'a>(steps: &'a [Value], uses: &str) -> &'a Value {
    steps
        .iter()
        .find(|step| mapping_get(step, "uses").and_then(Value::as_str) == Some(uses))
        .unwrap_or_else(|| panic!("steps must contain uses: {uses}"))
}

fn sequence_at<'a>(root: &'a Value, label: &str, path: &[&str]) -> &'a [Value] {
    match value_at(root, label, path) {
        Value::Sequence(sequence) => sequence,
        other => panic!("{label} must be a sequence, got {other:?}"),
    }
}

fn string_at<'a>(root: &'a Value, label: &str, path: &[&str]) -> &'a str {
    match value_at(root, label, path) {
        Value::String(value) => value,
        other => panic!("{label} must be a string, got {other:?}"),
    }
}

fn bool_at(root: &Value, label: &str, path: &[&str]) -> bool {
    match value_at(root, label, path) {
        Value::Bool(value) => *value,
        other => panic!("{label} must be a boolean, got {other:?}"),
    }
}

fn value_at<'a>(root: &'a Value, label: &str, path: &[&str]) -> &'a Value {
    let mut current = root;
    for key in path {
        current = mapping_get(current, key)
            .unwrap_or_else(|| panic!("{label} missing required field {key:?}"));
    }
    current
}

fn string_field<'a>(root: &'a Value, field: &str, label: &str) -> &'a str {
    match mapping_get(root, field) {
        Some(Value::String(value)) => value,
        Some(other) => panic!("{label} must be a string, got {other:?}"),
        None => panic!("{label} missing required field {field:?}"),
    }
}

fn mapping_get<'a>(root: &'a Value, key: &str) -> Option<&'a Value> {
    match root {
        Value::Mapping(mapping) => mapping.get(Value::String(key.to_string())),
        _ => None,
    }
}
