use regex::Regex;
use serde_yml::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

fn path_from_test_file(relative_path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(relative_path)
}

fn read_text(relative_path: &str) -> String {
    let path = path_from_test_file(relative_path);
    fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()))
}

fn parse_workflow(relative_path: &str) -> Value {
    let body = read_text(relative_path);
    serde_yml::from_str(&body).unwrap_or_else(|err| {
        panic!(
            "failed to parse {} as workflow YAML: {err}",
            path_from_test_file(relative_path).display()
        )
    })
}

fn parse_toml(relative_path: &str) -> toml::Value {
    let body = read_text(relative_path);
    toml::from_str(&body).unwrap_or_else(|err| {
        panic!(
            "failed to parse {} as TOML: {err}",
            path_from_test_file(relative_path).display()
        )
    })
}

fn ci_workflow() -> Value {
    parse_workflow("../../.github/workflows/ci.yml")
}

fn release_workflow() -> Value {
    parse_workflow("../../.github/workflows/release.yml")
}

fn workflow_pairs() -> [(&'static str, Value); 2] {
    [
        ("ci.yml", ci_workflow()),
        ("release.yml", release_workflow()),
    ]
}

fn mapping_get<'a>(root: &'a Value, key: &str) -> Option<&'a Value> {
    let Value::Mapping(mapping) = root else {
        return None;
    };
    mapping.get(Value::String(key.to_string())).or_else(|| {
        if key == "on" {
            mapping.get(Value::Bool(true))
        } else {
            None
        }
    })
}

fn toml_table<'a>(root: &'a toml::Value, label: &str) -> &'a toml::map::Map<String, toml::Value> {
    root.as_table()
        .unwrap_or_else(|| panic!("{label} must be a TOML table"))
}

fn toml_value_at<'a>(root: &'a toml::Value, label: &str, path: &[&str]) -> &'a toml::Value {
    let mut current = root;
    for key in path {
        current = toml_table(current, label)
            .get(*key)
            .unwrap_or_else(|| panic!("{label} missing required TOML field {key:?}"));
    }
    current
}

fn value_at<'a>(root: &'a Value, label: &str, path: &[&str]) -> &'a Value {
    let mut current = root;
    for key in path {
        current = mapping_get(current, key)
            .unwrap_or_else(|| panic!("{label} missing required YAML field {key:?}"));
    }
    current
}

fn mapping_at<'a>(root: &'a Value, label: &str, path: &[&str]) -> &'a serde_yml::Mapping {
    match value_at(root, label, path) {
        Value::Mapping(mapping) => mapping,
        other => panic!("{label} must be a mapping, got {other:?}"),
    }
}

fn sequence_at<'a>(root: &'a Value, label: &str, path: &[&str]) -> &'a [Value] {
    match value_at(root, label, path) {
        Value::Sequence(sequence) => sequence,
        other => panic!("{label} must be a sequence, got {other:?}"),
    }
}

fn string_field<'a>(root: &'a Value, field: &str, label: &str) -> &'a str {
    match mapping_get(root, field) {
        Some(Value::String(value)) => value,
        Some(other) => panic!("{label} must be a string, got {other:?}"),
        None => panic!("{label} missing required field {field:?}"),
    }
}

fn jobs(workflow: &Value) -> &serde_yml::Mapping {
    mapping_at(workflow, "jobs", &["jobs"])
}

fn job<'a>(workflow: &'a Value, job_name: &str, label: &str) -> &'a Value {
    mapping_get(value_at(workflow, label, &["jobs"]), job_name)
        .unwrap_or_else(|| panic!("{label} missing required job {job_name:?}"))
}

fn job_steps<'a>(workflow: &'a Value, job_name: &str, label: &str) -> &'a [Value] {
    sequence_at(
        job(workflow, job_name, label),
        &format!("{label} jobs.{job_name}.steps"),
        &["steps"],
    )
}

fn step_by_uses<'a>(
    workflow: &'a Value,
    workflow_name: &str,
    job_name: &str,
    uses: &str,
) -> &'a Value {
    job_steps(workflow, job_name, workflow_name)
        .iter()
        .find(|step| mapping_get(step, "uses").and_then(Value::as_str) == Some(uses))
        .unwrap_or_else(|| panic!("{workflow_name} {job_name} steps must contain uses: {uses}"))
}

fn string_at<'a>(root: &'a Value, label: &str, path: &[&str]) -> &'a str {
    match value_at(root, label, path) {
        Value::String(value) => value,
        other => panic!("{label} must be a string, got {other:?}"),
    }
}

fn step_run_blocks<'a>(workflow: &'a Value, job_name: &str, label: &str) -> Vec<&'a str> {
    job_steps(workflow, job_name, label)
        .iter()
        .filter_map(|step| mapping_get(step, "run").and_then(Value::as_str))
        .collect()
}

fn standalone_binary_matrix_entries(
    workflow: &Value,
    workflow_name: &str,
    job_name: &str,
) -> BTreeSet<(String, String)> {
    sequence_at(
        job(workflow, job_name, workflow_name),
        &format!("{workflow_name} jobs.{job_name}.strategy.matrix.include"),
        &["strategy", "matrix", "include"],
    )
    .iter()
    .enumerate()
    .map(|(index, entry)| {
        (
            string_field(
                entry,
                "os",
                &format!("{workflow_name} jobs.{job_name}.strategy.matrix.include[{index}].os"),
            )
            .to_string(),
            string_field(
                entry,
                "target",
                &format!("{workflow_name} jobs.{job_name}.strategy.matrix.include[{index}].target"),
            )
            .to_string(),
        )
    })
    .collect()
}

fn assert_standalone_binary_release_job(
    workflow: &Value,
    job_name: &str,
    package_name: &str,
    binary_name: &str,
    artifact_prefix: &str,
    assertion: &str,
) {
    let workflow_name = "release.yml";
    assert_eq!(
        needs_list(job(workflow, job_name, workflow_name)),
        vec!["version".to_string()],
        "{assertion}: {job_name}.needs must be exactly [version]"
    );
    assert_eq!(
        standalone_binary_matrix_entries(workflow, workflow_name, job_name),
        BTreeSet::from([
            (
                "ubuntu-latest".to_string(),
                "x86_64-unknown-linux-gnu".to_string()
            ),
            (
                "macos-latest".to_string(),
                "aarch64-apple-darwin".to_string()
            ),
            (
                "windows-latest".to_string(),
                "x86_64-pc-windows-msvc".to_string()
            )
        ]),
        "{assertion}: {job_name} must build the three standalone binary targets"
    );

    let job_text = value_text(job(workflow, job_name, workflow_name));
    assert!(
        job_text.contains(&format!("key: {artifact_prefix}-${{{{ matrix.target }}}}")),
        "{assertion}: {job_name} must use a target-scoped rust-cache key for {artifact_prefix}"
    );
    assert!(
        job_text.contains(&format!(
            "cargo build --release --target ${{{{ matrix.target }}}} --bin {binary_name} -p {package_name}"
        )),
        "{assertion}: {job_name} must build the expected package/bin"
    );
    assert!(
        job_text.contains(&format!(
            "cp target/${{{{ matrix.target }}}}/release/{binary_name} artifacts/{artifact_prefix}-${{{{ matrix.target }}}}"
        )),
        "{assertion}: {job_name} must copy the non-Windows binary with target suffix"
    );
    assert!(
        job_text.contains(&format!(
            "Copy-Item target/${{{{ matrix.target }}}}/release/{binary_name}.exe artifacts/{artifact_prefix}-${{{{ matrix.target }}}}.exe"
        )),
        "{assertion}: {job_name} must copy the Windows .exe with target suffix"
    );

    let upload = step_by_uses(
        workflow,
        workflow_name,
        job_name,
        "actions/upload-artifact@v4",
    );
    assert_eq!(
        string_at(
            upload,
            &format!("{workflow_name} jobs.{job_name}.steps[upload].with.name"),
            &["with", "name"],
        ),
        format!("{artifact_prefix}-${{{{ matrix.target }}}}"),
        "{assertion}: {job_name} upload name must be target-suffixed"
    );
    let upload_path = string_at(
        upload,
        &format!("{workflow_name} jobs.{job_name}.steps[upload].with.path"),
        &["with", "path"],
    );
    assert!(
        upload_path == "artifacts/*" || upload_path == "artifacts",
        "{assertion}: {job_name} upload path must be artifacts/* or artifacts, got {upload_path:?}"
    );
}

fn all_step_run_blocks(workflow: &Value) -> Vec<(&str, &str)> {
    jobs(workflow)
        .iter()
        .filter_map(|(job_key, job_value)| Some((job_key.as_str()?, job_value)))
        .flat_map(|(job_name, job_value)| {
            let steps = mapping_get(job_value, "steps").and_then(Value::as_sequence);
            steps
                .into_iter()
                .flatten()
                .filter_map(move |step| Some((job_name, mapping_get(step, "run")?.as_str()?)))
        })
        .collect()
}

fn value_text(value: &Value) -> String {
    serde_yml::to_string(value).unwrap_or_else(|err| panic!("failed to render YAML value: {err}"))
}

fn regex_is_match(pattern: &str, haystack: &str) -> bool {
    Regex::new(pattern)
        .unwrap_or_else(|err| panic!("invalid regex {pattern:?}: {err}"))
        .is_match(haystack)
}

fn assert_job_has_run_matching(
    workflow_name: &str,
    workflow: &Value,
    job_name: &str,
    assertion: &str,
    pattern: &str,
    description: &str,
) {
    let runs = step_run_blocks(workflow, job_name, workflow_name);
    assert!(
        runs.iter().any(|run| regex_is_match(pattern, run)),
        "{assertion}: {workflow_name} {job_name} must include {description}, but run blocks were: {runs:?}"
    );
}

fn assert_prepare_matrix_metadata_jq(workflow_name: &str, workflow: &Value, assertion: &str) {
    let prepare = job(workflow, "prepare-matrix", workflow_name);
    let outputs = mapping_at(
        prepare,
        &format!("{workflow_name} jobs.prepare-matrix.outputs"),
        &["outputs"],
    );
    assert!(
        outputs.contains_key(Value::String("libs".to_string())),
        "{assertion}: {workflow_name} prepare-matrix must expose a libs output"
    );
    assert!(
        outputs.contains_key(Value::String("clients".to_string())),
        "{assertion}: {workflow_name} prepare-matrix must expose a clients output"
    );

    let pattern = r"(?s)cargo\s+metadata\s+--format-version\s+1\s+--no-deps.{0,500}\|\s*jq\b";
    let runs = step_run_blocks(workflow, "prepare-matrix", workflow_name);
    assert!(
        runs.iter().any(|run| regex_is_match(pattern, run)),
        "{assertion}: {workflow_name} prepare-matrix step must invoke cargo metadata + jq, but found: {runs:?}"
    );
}

fn assert_prepare_matrix_libs_filter_excludes_bins(
    workflow_name: &str,
    workflow: &Value,
    assertion: &str,
) {
    let runs = step_run_blocks(workflow, "prepare-matrix", workflow_name);
    assert!(
        runs.iter().any(|run| {
            run.lines().any(|line| {
                line.contains("libs=$(echo")
                    && line.contains(r#"test("/crates/")"#)
                    && line.contains(r#"select(any(.targets[]; .kind | index("bin")) | not)"#)
            })
        }),
        "{assertion}: {workflow_name} prepare-matrix libs jq filter must select /crates/ members and exclude packages with bin targets, but found: {runs:?}"
    );
}

fn collect_string_leaves(value: &Value, output: &mut BTreeSet<String>) {
    match value {
        Value::String(text) => {
            output.insert(text.to_string());
        }
        Value::Sequence(items) => {
            for item in items {
                collect_string_leaves(item, output);
            }
        }
        Value::Mapping(mapping) => {
            for value in mapping.values() {
                collect_string_leaves(value, output);
            }
        }
        _ => {}
    }
}

fn contains_hardcoded_lib_matrix(value: &Value) -> bool {
    const LIBS: [&str; 5] = [
        "oulipoly-core",
        "oulipoly-config",
        "oulipoly-state",
        "oulipoly-runtime",
        "oulipoly-setup",
    ];
    const MATRIX_KEYS: [&str; 9] = [
        "crate", "crates", "package", "packages", "member", "members", "lib", "libs", "include",
    ];

    match value {
        Value::Mapping(mapping) => mapping.iter().any(|(key, child)| {
            let child_has_static_libs = match key.as_str() {
                Some(key_text) if MATRIX_KEYS.contains(&key_text) => {
                    let mut leaves = BTreeSet::new();
                    collect_string_leaves(child, &mut leaves);
                    LIBS.iter().all(|lib| leaves.contains(*lib))
                }
                _ => false,
            };
            child_has_static_libs || contains_hardcoded_lib_matrix(child)
        }),
        Value::Sequence(items) => items.iter().any(contains_hardcoded_lib_matrix),
        _ => false,
    }
}

fn needs_list(job_value: &Value) -> Vec<String> {
    match mapping_get(job_value, "needs") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::String(need)) => vec![need.to_string()],
        Some(Value::Sequence(needs)) => needs
            .iter()
            .map(|need| {
                need.as_str()
                    .unwrap_or_else(|| panic!("needs entry must be a string, got {need:?}"))
                    .to_string()
            })
            .collect(),
        Some(other) => panic!("needs must be a string or sequence, got {other:?}"),
    }
}

fn edge_set(workflow: &Value) -> BTreeSet<(String, String)> {
    jobs(workflow)
        .iter()
        .filter_map(|(job_key, job_value)| Some((job_key.as_str()?, job_value)))
        .flat_map(|(job_name, job_value)| {
            needs_list(job_value)
                .into_iter()
                .map(move |need| (need, job_name.to_string()))
        })
        .collect()
}

fn job_name_set(workflow: &Value) -> BTreeSet<String> {
    jobs(workflow)
        .keys()
        .map(|key| {
            key.as_str()
                .unwrap_or_else(|| panic!("job key must be a string, got {key:?}"))
                .to_string()
        })
        .collect()
}

fn assert_acyclic(workflow_name: &str, workflow: &Value, assertion: &str) {
    let names = job_name_set(workflow);
    let edges = edge_set(workflow);
    for (from, to) in &edges {
        assert!(
            names.contains(from),
            "{assertion}: {workflow_name} job {to:?} depends on unknown job {from:?}"
        );
    }

    let mut inbound_counts = BTreeMap::<String, usize>::new();
    let mut outbound = BTreeMap::<String, Vec<String>>::new();
    for name in &names {
        inbound_counts.insert(name.clone(), 0);
        outbound.insert(name.clone(), Vec::new());
    }
    for (from, to) in edges {
        *inbound_counts
            .get_mut(&to)
            .unwrap_or_else(|| panic!("{assertion}: {workflow_name} missing inbound node {to}")) +=
            1;
        outbound
            .get_mut(&from)
            .unwrap_or_else(|| panic!("{assertion}: {workflow_name} missing outbound node {from}"))
            .push(to);
    }

    let mut ready = inbound_counts
        .iter()
        .filter_map(|(name, count)| (*count == 0).then_some(name.clone()))
        .collect::<VecDeque<_>>();
    let mut visited = 0;
    while let Some(name) = ready.pop_front() {
        visited += 1;
        for target in outbound.get(&name).into_iter().flatten() {
            let count = inbound_counts
                .get_mut(target)
                .unwrap_or_else(|| panic!("{assertion}: {workflow_name} missing node {target}"));
            *count -= 1;
            if *count == 0 {
                ready.push_back(target.clone());
            }
        }
    }

    assert_eq!(
        visited,
        names.len(),
        "{assertion}: {workflow_name} dependency graph must be acyclic"
    );
}

fn assert_needs_include(
    workflow_name: &str,
    workflow: &Value,
    job_name: &str,
    expected: &[&str],
    assertion: &str,
) {
    let needs = needs_list(job(workflow, job_name, workflow_name));
    for expected_need in expected {
        assert!(
            needs.iter().any(|need| need == expected_need),
            "{assertion}: {workflow_name} {job_name}.needs must include {expected_need:?}, got {needs:?}"
        );
    }
}

fn frontend_command_lines(workflow: &Value, workflow_name: &str) -> Vec<String> {
    step_run_blocks(workflow, "frontend-check", workflow_name)
        .into_iter()
        .flat_map(run_command_lines)
        .filter(|line| line.starts_with("bun "))
        .chain(
            step_run_blocks(workflow, "frontend-check", workflow_name)
                .into_iter()
                .flat_map(run_command_lines)
                .filter(|line| line.starts_with("bunx ")),
        )
        .collect()
}

fn run_command_lines(run: &str) -> Vec<String> {
    run.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| line.trim_end_matches('\\').trim().to_string())
        .collect()
}

fn uses_step_exists(workflow: &Value, workflow_name: &str, job_name: &str, uses: &str) -> bool {
    job_steps(workflow, job_name, workflow_name)
        .iter()
        .any(|step| mapping_get(step, "uses").and_then(Value::as_str) == Some(uses))
}

fn step_run_index_containing(
    workflow: &Value,
    workflow_name: &str,
    job_name: &str,
    needle: &str,
) -> Option<usize> {
    job_steps(workflow, job_name, workflow_name)
        .iter()
        .position(|step| {
            mapping_get(step, "run")
                .and_then(Value::as_str)
                .is_some_and(|run| run.contains(needle))
        })
}

fn binary_workspace_members() -> Vec<String> {
    let workspace = parse_toml("../../Cargo.toml");
    let members = toml_value_at(&workspace, "workspace.members", &["workspace", "members"])
        .as_array()
        .unwrap_or_else(|| panic!("workspace.members must be a TOML array"));

    members
        .iter()
        .filter_map(|member| {
            let member_path = member
                .as_str()
                .unwrap_or_else(|| panic!("workspace member must be a string, got {member:?}"));
            let manifest = parse_toml(&format!("../../{member_path}/Cargo.toml"));
            let has_bin = toml_table(&manifest, member_path).contains_key("bin");
            if has_bin {
                Some(
                    toml_value_at(&manifest, "package.name", &["package", "name"])
                        .as_str()
                        .unwrap_or_else(|| panic!("{member_path} package.name must be a string"))
                        .to_string(),
                )
            } else {
                None
            }
        })
        .collect()
}

fn cargo_command_inventory(workflow: &Value) -> Vec<(String, String)> {
    all_step_run_blocks(workflow)
        .into_iter()
        .flat_map(|(job_name, run)| {
            run_command_lines(run)
                .into_iter()
                .filter(|line| {
                    line.starts_with("cargo test")
                        || line.starts_with("cargo clippy")
                        || line.starts_with("cargo fmt")
                })
                .map(move |line| (job_name.to_string(), line))
        })
        .collect()
}

fn assert_cargo_inventory_matches(workflow_name: &str, workflow: &Value) {
    let inventory = cargo_command_inventory(workflow);
    let allowed = [
        (
            "rust-lib-check",
            r"^cargo\s+test\s+-p\s+\$\{\{\s*matrix\.crate\s*\}\}\s*$",
        ),
        (
            "rust-lib-check",
            r"^cargo\s+clippy\s+-p\s+\$\{\{\s*matrix\.crate\s*\}\}\s+--\s+-D\s+warnings\s*$",
        ),
        (
            "rust-client-check",
            r"^cargo\s+test\s+-p\s+\$\{\{\s*matrix\.client\.name\s*\}\}\s*$",
        ),
        (
            "rust-client-check",
            r"^cargo\s+clippy\s+-p\s+\$\{\{\s*matrix\.client\.name\s*\}\}\s+--\s+-D\s+warnings\s*$",
        ),
        (
            "rust-integration",
            r"^cargo\s+test\s+--workspace(\s+--.*)?\s*$",
        ),
        (
            "rust-integration",
            r"^cargo\s+fmt\s+--all\s+--\s+--check\s*$",
        ),
        (
            "rust-integration",
            r"^cargo\s+clippy\s+--workspace\s+--\s+-D\s+warnings\s*$",
        ),
    ];

    for (job_name, line) in &inventory {
        assert!(
            allowed
                .iter()
                .any(|(allowed_job, pattern)| job_name == allowed_job
                    && regex_is_match(pattern, line)),
            "A11: {workflow_name} contains an out-of-contract cargo test/lint command in {job_name}: {line:?}; inventory: {inventory:?}"
        );
    }
    for (expected_job, expected_pattern) in allowed {
        assert!(
            inventory
                .iter()
                .any(|(job_name, line)| job_name == expected_job
                    && regex_is_match(expected_pattern, line)),
            "A11: {workflow_name} missing expected cargo test/lint command {expected_pattern:?} in {expected_job}; inventory: {inventory:?}"
        );
    }
}

fn global_bun_inventory(workflow: &Value) -> Vec<(String, String)> {
    all_step_run_blocks(workflow)
        .into_iter()
        .flat_map(|(job_name, run)| {
            run_command_lines(run)
                .into_iter()
                .filter(|line| line.starts_with("bun ") || line.starts_with("bunx "))
                .map(move |line| (job_name.to_string(), line))
        })
        .collect()
}

fn assert_frontend_bun_inventory(workflow_name: &str, workflow: &Value) {
    let frontend = frontend_command_lines(workflow, workflow_name);
    assert_eq!(
        BTreeSet::from_iter(frontend),
        BTreeSet::from([
            "bun install".to_string(),
            "bun run check".to_string(),
            "bunx tsc --noEmit".to_string(),
            "bun run test".to_string()
        ]),
        "A12: {workflow_name} frontend-check run blocks must contain exactly the preserved frontend Bun commands"
    );

    let inventory = global_bun_inventory(workflow);
    for (job_name, line) in &inventory {
        let allowed_frontend = job_name == "frontend-check"
            && matches!(
                line.as_str(),
                "bun install" | "bun run check" | "bunx tsc --noEmit" | "bun run test"
            );
        let allowed_release_build = workflow_name == "release.yml"
            && job_name == "build"
            && (line == "bun install" || line.starts_with("bunx tauri build"));
        assert!(
            allowed_frontend || allowed_release_build,
            "A12: {workflow_name} contains unexpected Bun invocation in {job_name}: {line:?}; inventory: {inventory:?}"
        );
    }
}

fn apt_install_packages(run: &str) -> BTreeSet<String> {
    let normalized = run.replace("\\\n", " ");
    let Some(install_index) = normalized.find("apt-get install") else {
        return BTreeSet::new();
    };
    let install_line = normalized[install_index..]
        .lines()
        .next()
        .unwrap_or_default();
    let tokens = install_line
        .split_whitespace()
        .map(|token| token.trim_matches('\\'))
        .collect::<Vec<_>>();
    let Some(start_index) = tokens.iter().position(|token| *token == "-y") else {
        return BTreeSet::new();
    };
    tokens[start_index + 1..]
        .iter()
        .take_while(|token| **token != "&&" && **token != ";")
        .filter(|token| !token.is_empty())
        .map(|token| token.to_string())
        .collect()
}

fn assert_apt_packages(workflow_name: &str, workflow: &Value, job_name: &str) {
    let apt_steps = step_run_blocks(workflow, job_name, workflow_name)
        .into_iter()
        .filter(|run| run.contains("apt-get install"))
        .collect::<Vec<_>>();
    assert_eq!(
        apt_steps.len(),
        1,
        "A13: {workflow_name} {job_name} must contain exactly one apt-get install step, found: {apt_steps:?}"
    );
    assert_eq!(
        apt_install_packages(apt_steps[0]),
        BTreeSet::from([
            "libwebkit2gtk-4.1-dev".to_string(),
            "libgtk-3-dev".to_string(),
            "libsoup-3.0-dev".to_string(),
            "libjavascriptcoregtk-4.1-dev".to_string()
        ]),
        "A13: {workflow_name} {job_name} apt-get install step must list exactly the preserved Linux Tauri/WebKit packages"
    );
}

#[test]
fn assertion_a01_per_lib_matrix_generator_output() {
    for (workflow_name, workflow) in workflow_pairs() {
        assert_prepare_matrix_metadata_jq(workflow_name, &workflow, "A1");
        assert_prepare_matrix_libs_filter_excludes_bins(workflow_name, &workflow, "A1");
        assert!(
            !contains_hardcoded_lib_matrix(&workflow),
            "A1: {workflow_name} must not contain a hard-coded matrix list with all current library crate names"
        );
    }
}

#[test]
fn assertion_a02_per_lib_matrix_commands() {
    for (workflow_name, workflow) in workflow_pairs() {
        let matrix = value_at(
            job(&workflow, "rust-lib-check", workflow_name),
            &format!("{workflow_name} jobs.rust-lib-check.strategy.matrix"),
            &["strategy", "matrix"],
        );
        let matrix_text = value_text(matrix);
        assert!(
            matrix_text.contains("fromJSON")
                && matrix_text.contains("needs.prepare-matrix.outputs.libs"),
            "A2: {workflow_name} rust-lib-check strategy.matrix must consume fromJSON(needs.prepare-matrix.outputs.libs), got: {matrix_text}"
        );
        assert_job_has_run_matching(
            workflow_name,
            &workflow,
            "rust-lib-check",
            "A2",
            r"(?m)(^|\s)cargo\s+test\s+-p\s+\$\{\{\s*matrix\.crate\s*\}\}",
            "cargo test -p ${{ matrix.crate }}",
        );
        assert_job_has_run_matching(
            workflow_name,
            &workflow,
            "rust-lib-check",
            "A2",
            r"(?m)(^|\s)cargo\s+clippy\s+-p\s+\$\{\{\s*matrix\.crate\s*\}\}\s+--\s+-D\s+warnings",
            "cargo clippy -p ${{ matrix.crate }} -- -D warnings",
        );
    }
}

#[test]
fn assertion_a03_per_client_matrix_generator_output() {
    for (workflow_name, workflow) in workflow_pairs() {
        let matrix = value_at(
            job(&workflow, "rust-client-check", workflow_name),
            &format!("{workflow_name} jobs.rust-client-check.strategy.matrix"),
            &["strategy", "matrix"],
        );
        let matrix_text = value_text(matrix);
        assert!(
            matrix_text.contains("fromJSON")
                && matrix_text.contains("needs.prepare-matrix.outputs.clients"),
            "A3: {workflow_name} rust-client-check strategy.matrix must consume fromJSON(needs.prepare-matrix.outputs.clients), got: {matrix_text}"
        );
        assert_job_has_run_matching(
            workflow_name,
            &workflow,
            "rust-client-check",
            "A3",
            r"(?m)(^|\s)cargo\s+test\s+-p\s+\$\{\{\s*matrix\.client\.name\s*\}\}",
            "cargo test -p ${{ matrix.client.name }}",
        );
        assert_job_has_run_matching(
            workflow_name,
            &workflow,
            "rust-client-check",
            "A3",
            r"(?m)(^|\s)cargo\s+clippy\s+-p\s+\$\{\{\s*matrix\.client\.name\s*\}\}\s+--\s+-D\s+warnings",
            "cargo clippy -p ${{ matrix.client.name }} -- -D warnings",
        );

        let prepare_runs = step_run_blocks(&workflow, "prepare-matrix", workflow_name);
        assert!(
            prepare_runs.iter().any(|run| {
                run.contains("clients")
                    && run.contains("jq")
                    && regex_is_match(
                        r#"(?s)targets?.*kind.*["']bin["']|["']bin["'].*kind.*targets?"#,
                        run,
                    )
            }),
            "A3: {workflow_name} prepare-matrix clients output must use a jq filter selecting targets with kind containing \"bin\", but found: {prepare_runs:?}"
        );

        let client_runs = step_run_blocks(&workflow, "rust-client-check", workflow_name);
        assert!(
            client_runs
                .iter()
                .all(|run| !run.contains("oulipoly-agent-runner")),
            "A3: {workflow_name} rust-client-check run blocks must use matrix client variables, not literal oulipoly-agent-runner; found: {client_runs:?}"
        );
    }
}

#[test]
fn assertion_a04_integration_workspace_commands() {
    for (workflow_name, workflow) in workflow_pairs() {
        assert_needs_include(
            workflow_name,
            &workflow,
            "rust-integration",
            &["rust-lib-check", "rust-client-check"],
            "A4",
        );
        assert_job_has_run_matching(
            workflow_name,
            &workflow,
            "rust-integration",
            "A4",
            r"(?m)(^|\s)cargo\s+test\s+--workspace(\s+--.*)?",
            "cargo test --workspace",
        );
        assert_job_has_run_matching(
            workflow_name,
            &workflow,
            "rust-integration",
            "A4",
            r"(?m)(^|\s)cargo\s+fmt\s+--all\s+--\s+--check",
            "cargo fmt --all -- --check",
        );
        assert_job_has_run_matching(
            workflow_name,
            &workflow,
            "rust-integration",
            "A4",
            r"(?m)(^|\s)cargo\s+clippy\s+--workspace\s+--\s+-D\s+warnings",
            "cargo clippy --workspace -- -D warnings",
        );
    }
}

#[test]
fn assertion_a05_ci_trigger_preserved() {
    let workflow = ci_workflow();
    let on = value_at(&workflow, "A5 ci.yml on", &["on"]);
    let on_mapping = match on {
        Value::Mapping(mapping) => mapping,
        other => panic!("A5: ci.yml on block must be a mapping, got {other:?}"),
    };
    assert_eq!(
        on_mapping.len(),
        1,
        "A5: ci.yml on block must only contain pull_request, got {on_mapping:?}"
    );
    let pull_request = mapping_get(on, "pull_request")
        .unwrap_or_else(|| panic!("A5: ci.yml on block must contain pull_request"));
    assert_eq!(
        sequence_at(
            pull_request,
            "A5 ci.yml on.pull_request.branches",
            &["branches"]
        )
        .iter()
        .map(|branch| {
            branch
                .as_str()
                .unwrap_or_else(|| panic!("A5: ci.yml pull_request branch must be a string"))
                .to_string()
        })
        .collect::<Vec<_>>(),
        vec!["main".to_string()],
        "A5: ci.yml pull_request branches must be exactly [main]"
    );
}

#[test]
fn assertion_a06_release_trigger_preserved() {
    let workflow = release_workflow();
    let on = value_at(&workflow, "A6 release.yml on", &["on"]);
    let on_mapping = match on {
        Value::Mapping(mapping) => mapping,
        other => panic!("A6: release.yml on block must be a mapping, got {other:?}"),
    };
    assert_eq!(
        on_mapping.len(),
        1,
        "A6: release.yml on block must only contain workflow_dispatch, got {on_mapping:?}"
    );
    let dispatch = mapping_get(on, "workflow_dispatch")
        .unwrap_or_else(|| panic!("A6: release.yml on block must contain workflow_dispatch"));
    assert!(
        mapping_get(
            value_at(dispatch, "A6 workflow_dispatch.inputs", &["inputs"]),
            "reason"
        )
        .is_some(),
        "A6: release.yml workflow_dispatch must preserve the optional reason input"
    );
}

#[test]
fn assertion_a07_frontend_invocations_preserved() {
    for (workflow_name, workflow) in workflow_pairs() {
        assert!(
            uses_step_exists(
                &workflow,
                workflow_name,
                "frontend-check",
                "actions/checkout@v4"
            ),
            "A7: {workflow_name} frontend-check must include actions/checkout@v4"
        );
        assert!(
            uses_step_exists(
                &workflow,
                workflow_name,
                "frontend-check",
                "oven-sh/setup-bun@v2"
            ),
            "A7: {workflow_name} frontend-check must include oven-sh/setup-bun@v2"
        );

        let fontawesome_index = step_run_index_containing(
            &workflow,
            workflow_name,
            "frontend-check",
            "@fortawesome:registry=https://npm.fontawesome.com/",
        )
        .unwrap_or_else(|| {
            panic!(
                "A7: {workflow_name} frontend-check must configure the FontAwesome registry before bun install"
            )
        });
        let fontawesome_run = string_field(
            &job_steps(&workflow, "frontend-check", workflow_name)[fontawesome_index],
            "run",
            "A7 frontend-check FontAwesome run",
        );
        assert!(
            fontawesome_run
                .contains("//npm.fontawesome.com/:_authToken=${{ secrets.FONTAWESOME_NPM_TOKEN }}")
                && fontawesome_run.contains(".npmrc"),
            "A7: {workflow_name} FontAwesome registry step must write both required .npmrc lines, got: {fontawesome_run}"
        );
        let bun_install_index =
            step_run_index_containing(&workflow, workflow_name, "frontend-check", "bun install")
                .unwrap_or_else(|| {
                    panic!("A7: {workflow_name} frontend-check must run bun install")
                });
        assert!(
            fontawesome_index < bun_install_index,
            "A7: {workflow_name} FontAwesome registry step must run before bun install"
        );

        for (command, pattern) in [
            ("bun install", r"(?m)^bun\s+install\s*$"),
            ("bun run check", r"(?m)^bun\s+run\s+check\s*$"),
            ("bunx tsc --noEmit", r"(?m)^bunx\s+tsc\s+--noEmit\s*$"),
            ("bun run test", r"(?m)^bun\s+run\s+test\s*$"),
        ] {
            assert_job_has_run_matching(
                workflow_name,
                &workflow,
                "frontend-check",
                "A7",
                pattern,
                command,
            );
        }
    }
}

#[test]
fn assertion_a08_binary_clients_have_release_path() {
    let release = release_workflow();
    let release_jobs = job_name_set(&release);
    let binary_members = binary_workspace_members();
    assert!(
        !binary_members.is_empty(),
        "A8: workspace must expose at least one binary client member for release-path coverage"
    );

    for member_name in binary_members {
        let explicit_job = format!("build-{member_name}");
        let grandfathered =
            member_name == "oulipoly-agent-runner" && release_jobs.contains("build");
        assert!(
            release_jobs.contains(&explicit_job) || grandfathered,
            "A8: binary client {member_name:?} must have release path job {explicit_job:?}, or the grandfathered legacy build job for oulipoly-agent-runner; jobs: {release_jobs:?}"
        );
    }
    assert!(
        release_jobs.contains("build"),
        "A8: release.yml must preserve the legacy build job for oulipoly-agent-runner"
    );
}

#[test]
fn assertion_a09_release_contract_cascade_preserved() {
    let release_contract = path_from_test_file("release_yml_contract.rs");
    assert!(
        release_contract.exists(),
        "A9: src-tauri/tests/release_yml_contract.rs must remain present as the cascade-protected release contract"
    );

    let own_source = read_text("workflow_yml_contract.rs");
    let forbidden_literals = [
        ["jobs.build.strategy.matrix.", "client"].concat(),
        ["jobs.build.strategy.matrix.", "target"].concat(),
        ["${{ matrix.target.", "triple }}"].concat(),
        ["${{ matrix.target.", "bundles }}"].concat(),
        ["${{ matrix.client.", "binary }}"].concat(),
        ["${{ matrix.client.", "path }}"].concat(),
    ];
    for forbidden in forbidden_literals {
        assert!(
            !own_source.contains(&forbidden),
            "A9: workflow_yml_contract.rs must not contain cascade-violating literal {forbidden:?}"
        );
    }
}

#[test]
fn assertion_a10_dependency_graph_required_edges() {
    let ci = ci_workflow();
    let ci_expected_jobs = BTreeSet::from([
        "prepare-matrix".to_string(),
        "frontend-check".to_string(),
        "rust-lib-check".to_string(),
        "rust-client-check".to_string(),
        "rust-integration".to_string(),
    ]);
    let ci_expected_edges = BTreeSet::from([
        ("prepare-matrix".to_string(), "rust-lib-check".to_string()),
        (
            "prepare-matrix".to_string(),
            "rust-client-check".to_string(),
        ),
        ("rust-lib-check".to_string(), "rust-integration".to_string()),
        (
            "rust-client-check".to_string(),
            "rust-integration".to_string(),
        ),
    ]);
    assert_eq!(
        job_name_set(&ci),
        ci_expected_jobs,
        "A10: ci.yml must contain exactly the required CI jobs and no release jobs"
    );
    assert_eq!(
        edge_set(&ci),
        ci_expected_edges,
        "A10: ci.yml dependency graph must contain exactly the required edges, with frontend-check as a leaf"
    );
    assert_acyclic("ci.yml", &ci, "A10");

    let release = release_workflow();
    let release_expected_jobs = BTreeSet::from([
        "prepare-matrix".to_string(),
        "frontend-check".to_string(),
        "rust-lib-check".to_string(),
        "rust-client-check".to_string(),
        "rust-integration".to_string(),
        "version".to_string(),
        "build".to_string(),
        "build-oulipoly-agent-cli".to_string(),
        "build-oulipoly-agent-store".to_string(),
        "build-oulipoly-agent-scratchpad".to_string(),
        "build-oulipoly-agent-messenger".to_string(),
        "release".to_string(),
    ]);
    let release_expected_edges = BTreeSet::from([
        ("prepare-matrix".to_string(), "rust-lib-check".to_string()),
        (
            "prepare-matrix".to_string(),
            "rust-client-check".to_string(),
        ),
        ("rust-lib-check".to_string(), "rust-integration".to_string()),
        (
            "rust-client-check".to_string(),
            "rust-integration".to_string(),
        ),
        ("rust-integration".to_string(), "version".to_string()),
        ("frontend-check".to_string(), "version".to_string()),
        ("version".to_string(), "build".to_string()),
        (
            "version".to_string(),
            "build-oulipoly-agent-cli".to_string(),
        ),
        (
            "version".to_string(),
            "build-oulipoly-agent-store".to_string(),
        ),
        (
            "version".to_string(),
            "build-oulipoly-agent-scratchpad".to_string(),
        ),
        (
            "version".to_string(),
            "build-oulipoly-agent-messenger".to_string(),
        ),
        ("version".to_string(), "release".to_string()),
        ("build".to_string(), "release".to_string()),
        (
            "build-oulipoly-agent-cli".to_string(),
            "release".to_string(),
        ),
        (
            "build-oulipoly-agent-store".to_string(),
            "release".to_string(),
        ),
        (
            "build-oulipoly-agent-scratchpad".to_string(),
            "release".to_string(),
        ),
        (
            "build-oulipoly-agent-messenger".to_string(),
            "release".to_string(),
        ),
    ]);
    assert_eq!(
        job_name_set(&release),
        release_expected_jobs,
        "A10: release.yml must contain exactly the required release jobs"
    );
    assert_eq!(
        edge_set(&release),
        release_expected_edges,
        "A10: release.yml dependency graph must contain exactly the required edges"
    );
    assert_eq!(
        needs_list(job(&release, "build", "release.yml")),
        vec!["version".to_string()],
        "A10: release.yml build.needs must be exactly [version] and must not consume prepare-matrix outputs"
    );
    assert_acyclic("release.yml", &release, "A10");
}

#[test]
fn assertion_a11_no_extra_test_lint_commands() {
    for (workflow_name, workflow) in workflow_pairs() {
        assert_cargo_inventory_matches(workflow_name, &workflow);
    }
}

#[test]
fn assertion_a12_bun_command_inventory_preserved() {
    for (workflow_name, workflow) in workflow_pairs() {
        assert_frontend_bun_inventory(workflow_name, &workflow);
    }
}

#[test]
fn assertion_a13_linux_apt_deps_preserved() {
    for (workflow_name, workflow) in workflow_pairs() {
        assert_apt_packages(workflow_name, &workflow, "rust-client-check");
        assert_apt_packages(workflow_name, &workflow, "rust-integration");
    }
}

#[test]
fn assertion_a14_standalone_binary_release_job_bodies() {
    let release = release_workflow();
    assert_standalone_binary_release_job(
        &release,
        "build-oulipoly-agent-cli",
        "oulipoly-agent-cli",
        "agent",
        "agent",
        "A14",
    );
    assert_standalone_binary_release_job(
        &release,
        "build-oulipoly-agent-store",
        "oulipoly-agent-store",
        "agent-store",
        "agent-store",
        "A14",
    );
    assert_standalone_binary_release_job(
        &release,
        "build-oulipoly-agent-scratchpad",
        "oulipoly-agent-scratchpad",
        "agent-scratchpad",
        "agent-scratchpad",
        "A14",
    );
    assert_standalone_binary_release_job(
        &release,
        "build-oulipoly-agent-messenger",
        "oulipoly-agent-messenger",
        "agent-messenger",
        "agent-messenger",
        "A14",
    );
}
