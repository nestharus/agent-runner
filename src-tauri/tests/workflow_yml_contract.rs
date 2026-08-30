//! Declared roles: accessor, filter, formatter, mapper, orchestration, parser, predicate, validator.

use regex::Regex;
use serde_yaml_ng::Value;
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
    require_read_text(&path, read_text_result(&path))
}

fn read_text_result(path: &Path) -> std::io::Result<String> {
    fs::read_to_string(path)
}

fn require_read_text(path: &Path, result: std::io::Result<String>) -> String {
    result.unwrap_or_else(|err| panic!("{}", format_read_text_error(path, &err)))
}

fn format_read_text_error(path: &Path, err: &std::io::Error) -> String {
    format!("failed to read {}: {err}", path.display())
}

fn parse_workflow(relative_path: &str) -> Value {
    let body = read_text(relative_path);
    require_workflow(relative_path, parse_workflow_body(&body))
}

fn parse_workflow_body(body: &str) -> Result<Value, serde_yaml_ng::Error> {
    serde_yaml_ng::from_str(body)
}

fn require_workflow(relative_path: &str, result: Result<Value, serde_yaml_ng::Error>) -> Value {
    result.unwrap_or_else(|err| panic!("{}", format_workflow_parse_error(relative_path, &err)))
}

fn format_workflow_parse_error(relative_path: &str, err: &serde_yaml_ng::Error) -> String {
    format!(
        "failed to parse {} as workflow YAML: {err}",
        path_from_test_file(relative_path).display()
    )
}

fn parse_toml(relative_path: &str) -> toml::Value {
    let body = read_text(relative_path);
    require_toml(relative_path, parse_toml_body(&body))
}

fn parse_toml_body(body: &str) -> Result<toml::Value, toml::de::Error> {
    toml::from_str(body)
}

fn require_toml(relative_path: &str, result: Result<toml::Value, toml::de::Error>) -> toml::Value {
    result.unwrap_or_else(|err| panic!("{}", format_toml_parse_error(relative_path, &err)))
}

fn format_toml_parse_error(relative_path: &str, err: &toml::de::Error) -> String {
    format!(
        "failed to parse {} as TOML: {err}",
        path_from_test_file(relative_path).display()
    )
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

fn mapping_at<'a>(root: &'a Value, label: &str, path: &[&str]) -> &'a serde_yaml_ng::Mapping {
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

fn jobs(workflow: &Value) -> &serde_yaml_ng::Mapping {
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

fn step_by_name<'a>(
    workflow: &'a Value,
    workflow_name: &str,
    job_name: &str,
    name: &str,
) -> &'a Value {
    let matching = job_steps(workflow, job_name, workflow_name)
        .iter()
        .filter(|step| mapping_get(step, "name").and_then(Value::as_str) == Some(name))
        .collect::<Vec<_>>();
    assert_eq!(
        matching.len(),
        1,
        "{workflow_name} {job_name} steps must contain exactly one step named {name:?}, found {}",
        matching.len()
    );
    matching[0]
}

fn assert_checkout_fetches_full_history(workflow: &Value, workflow_name: &str, job_name: &str) {
    let checkout = step_by_uses(workflow, workflow_name, job_name, "actions/checkout@v4");
    assert_eq!(
        value_at(
            checkout,
            &format!("{workflow_name} jobs.{job_name}.steps[checkout]"),
            &["with", "fetch-depth"],
        )
        .as_i64(),
        Some(0),
        "{workflow_name} {job_name} must fetch full history for history-dependent source guards"
    );
}

fn assert_agent_bash_install_precedes_test(
    workflow: &Value,
    workflow_name: &str,
    job_name: &str,
    test_command: &str,
    expected_condition: Option<&str>,
) {
    const INSTALL_ACTION: &str = "./.github/actions/install-agent-bash";
    let steps = job_steps(workflow, job_name, workflow_name);
    let install_index = require_step_index(
        step_index_by_uses(steps, INSTALL_ACTION),
        format_missing_agent_bash_install(workflow_name, job_name),
    );
    let test_index = require_step_index(
        step_index_by_run_text(steps, test_command),
        format_missing_test_command(workflow_name, job_name, test_command),
    );
    let install_step = &steps[install_index];
    assert_eq!(
        mapping_get(install_step, "if").and_then(Value::as_str),
        expected_condition,
        "{workflow_name} {job_name} agent-bash install condition must be {expected_condition:?}"
    );
    assert_step_order(
        install_index,
        test_index,
        format_agent_bash_install_order_error(workflow_name, job_name, test_command),
    );
}

fn step_index_by_uses(steps: &[Value], uses: &str) -> Option<usize> {
    steps
        .iter()
        .position(|step| mapping_get(step, "uses").and_then(Value::as_str) == Some(uses))
}

fn step_index_by_run_text(steps: &[Value], text: &str) -> Option<usize> {
    steps.iter().position(|step| {
        mapping_get(step, "run")
            .and_then(Value::as_str)
            .is_some_and(|run| run.contains(text))
    })
}

fn require_step_index(index: Option<usize>, failure: String) -> usize {
    index.unwrap_or_else(|| panic!("{failure}"))
}

fn assert_step_order(before: usize, after: usize, failure: String) {
    assert!(before < after, "{failure}");
}

fn format_missing_agent_bash_install(workflow_name: &str, job_name: &str) -> String {
    format!("{workflow_name} {job_name} must install pinned agent-bash")
}

fn format_missing_test_command(workflow_name: &str, job_name: &str, test_command: &str) -> String {
    format!("{workflow_name} {job_name} must run {test_command}")
}

fn format_agent_bash_install_order_error(
    workflow_name: &str,
    job_name: &str,
    test_command: &str,
) -> String {
    format!("{workflow_name} {job_name} must install agent-bash before {test_command}")
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
    for line in job_text.lines() {
        assert!(
            !line.contains("--target-dir"),
            "{assertion}: {job_name} must not override Cargo's target directory with --target-dir, found line: {line:?}"
        );
    }
    assert!(
        job_text.contains(&format!(
            "cp src-tauri/target/${{{{ matrix.target }}}}/release/{binary_name} artifacts/{artifact_prefix}-${{{{ matrix.target }}}}"
        )),
        "{assertion}: {job_name} must copy the non-Windows binary from src-tauri/target with target suffix"
    );
    assert!(
        job_text.contains(&format!(
            "Copy-Item src-tauri/target/${{{{ matrix.target }}}}/release/{binary_name}.exe artifacts/{artifact_prefix}-${{{{ matrix.target }}}}.exe"
        )),
        "{assertion}: {job_name} must copy the Windows .exe from src-tauri/target with target suffix"
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
    serde_yaml_ng::to_string(value)
        .unwrap_or_else(|err| panic!("failed to render YAML value: {err}"))
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
            let has_release_bin = toml_table(&manifest, member_path)
                .get("bin")
                .and_then(toml::Value::as_array)
                .is_some_and(|bins| {
                    bins.iter().any(|bin| {
                        bin.as_table()
                            .and_then(|bin| bin.get("required-features"))
                            .and_then(toml::Value::as_array)
                            .is_none_or(Vec::is_empty)
                    })
                });
            if has_release_bin {
                let package_name = toml_value_at(&manifest, "package.name", &["package", "name"])
                    .as_str()
                    .unwrap_or_else(|| panic!("{member_path} package.name must be a string"));
                (package_name != "oulipoly-agent-cli").then(|| package_name.to_string())
            } else {
                None
            }
        })
        .collect()
}

fn workspace_members_with_inherited_version() -> Vec<String> {
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
            let inherits_version = toml_table(&manifest, member_path)
                .get("package")
                .and_then(toml::Value::as_table)
                .and_then(|package| package.get("version"))
                .and_then(toml::Value::as_table)
                .and_then(|version| version.get("workspace"))
                .and_then(toml::Value::as_bool)
                .unwrap_or(false);

            inherits_version.then(|| format!("{member_path}/Cargo.toml"))
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
    let mut allowed = vec![
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
    if workflow_name == "ci.yml" {
        for job in ["rust-state-windows", "rust-state-macos"] {
            allowed.push((
                job,
                r"^cargo\s+test\s+-p\s+oulipoly-state\s+read_only_snapshot\s*$",
            ));
            allowed.push((
                job,
                r"^cargo\s+test\s+-p\s+oulipoly-state\s+pid_identity::tests\s*$",
            ));
        }
        allowed.extend([
            (
                "rust-native-wake",
                r"^cargo\s+test\s+-p\s+oulipoly-agent-runner\s+--lib\s+wake_coordinator::admission::tests::native_memory_observer_matches_host_and_drives_default_admission\s+--\s+--exact\s+--nocapture\s*$",
            ),
            (
                "rust-native-wake",
                r"^cargo\s+test\s+-p\s+oulipoly-agent-runner\s+--lib\s+wake_coordinator::admission::tests::unavailable_memory_observation_returns_visible_error_without_stranding_queue\s+--\s+--exact\s*$",
            ),
            (
                "rust-native-wake",
                r"^cargo\s+test\s+-p\s+oulipoly-agent-runner\s+--test\s+age309_native_wake_domain\s+native_count_five_startup_sweep_reaches_one_detached_provider_turn\s+--\s+--exact\s+--nocapture\s*$",
            ),
        ]);
    }

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

fn assert_ci_linux_install_step_condition(job_name: &str) {
    let workflow = ci_workflow();
    assert_eq!(
        string_at(
            job(&workflow, job_name, "ci.yml"),
            &format!("ci.yml jobs.{job_name}.runs-on"),
            &["runs-on"],
        ),
        "ubuntu-latest",
        "T3: ci.yml {job_name} must stay on the characterized Ubuntu runner"
    );

    let step = step_by_name(&workflow, "ci.yml", job_name, "Install system deps (Linux)");
    assert_eq!(
        string_field(
            step,
            "if",
            &format!("T3 ci.yml {job_name} Install system deps (Linux).if"),
        ),
        "runner.os == 'Linux'",
        "T3: ci.yml {job_name} Linux install step must use the tightened Linux-only condition"
    );
    assert!(
        string_field(
            step,
            "run",
            &format!("T3 ci.yml {job_name} Install system deps (Linux).run"),
        )
        .contains("apt-get install"),
        "T3: ci.yml {job_name} Linux install step must keep the apt install run block"
    );
    assert_apt_packages("ci.yml", &workflow, job_name);
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
fn history_dependent_source_guards_have_full_history_in_recurring_workflows() {
    for (workflow_name, workflow) in workflow_pairs() {
        for job_name in ["rust-lib-check", "rust-client-check", "rust-integration"] {
            assert_checkout_fetches_full_history(&workflow, workflow_name, job_name);
        }
    }

    let coverage = parse_workflow("../../.github/workflows/coverage.yml");
    assert_checkout_fetches_full_history(&coverage, "coverage.yml", "rust-coverage");
}

#[test]
fn agent_bash_integration_dependency_is_pinned_in_test_workflows() {
    assert_agent_bash_action_is_pinned();
    assert_agent_bash_test_workflow_ordering();
}

fn assert_agent_bash_action_is_pinned() {
    const AGENT_BASH_REV: &str = "2a435c4909d184bbac28873fcfb8afb72861ec2c";
    let action = read_text("../../.github/actions/install-agent-bash/action.yml");
    let revision_argument = Regex::new(&format!(
        r"(?m)^\s*cargo\s+install\b[^\n]*\s--rev\s+{}(?:\s|$)",
        regex::escape(AGENT_BASH_REV)
    ))
    .expect("agent-bash revision regex must compile");
    assert!(
        revision_argument.is_match(&action),
        "install-agent-bash must pass the pinned revision to cargo install --rev"
    );
    let github_env_export = Regex::new(
        r#"(?m)^\s*echo\s+["']AGENT_BASH_BIN=[^\n"']+["']\s*>>\s*["']\$GITHUB_ENV["']\s*$"#,
    )
    .expect("agent-bash GITHUB_ENV regex must compile");
    assert!(
        github_env_export.is_match(&action),
        "install-agent-bash must export AGENT_BASH_BIN through GITHUB_ENV"
    );
}

fn assert_agent_bash_test_workflow_ordering() {
    for (workflow_name, workflow) in [
        ("ci.yml", ci_workflow()),
        ("release.yml", release_workflow()),
    ] {
        assert_agent_bash_install_precedes_test(
            &workflow,
            workflow_name,
            "rust-client-check",
            "cargo test -p ${{ matrix.client.name }}",
            Some("matrix.client.name == 'oulipoly-agent-runner'"),
        );
        assert_agent_bash_install_precedes_test(
            &workflow,
            workflow_name,
            "rust-integration",
            "cargo test --workspace",
            None,
        );
    }
    let coverage = parse_workflow("../../.github/workflows/coverage.yml");
    assert_agent_bash_install_precedes_test(
        &coverage,
        "coverage.yml",
        "rust-coverage",
        "cargo llvm-cov --workspace --no-report",
        None,
    );
}

#[test]
fn assertion_a05_ci_trigger_is_manual_dispatch() {
    let workflow = ci_workflow();
    let on = value_at(&workflow, "A5 ci.yml on", &["on"]);
    let on_mapping = match on {
        Value::Mapping(mapping) => mapping,
        other => panic!("A5: ci.yml on block must be a mapping, got {other:?}"),
    };
    assert_eq!(
        on_mapping.len(),
        1,
        "A5: ci.yml on block must only contain workflow_dispatch, got {on_mapping:?}"
    );
    assert!(
        mapping_get(on, "workflow_dispatch").is_some(),
        "A5: ci.yml on block must contain workflow_dispatch (manual-only trigger)"
    );
    assert!(
        mapping_get(on, "pull_request").is_none(),
        "A5: ci.yml on block must not contain the pull_request auto-trigger"
    );
    assert!(
        mapping_get(on, "push").is_none(),
        "A5: ci.yml on block must not contain the push auto-trigger"
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
    assert!(
        !release_jobs.contains("build-oulipoly-agent-cli"),
        "A8: release.yml must not keep the removed oulipoly-agent-cli release job"
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
        "rust-state-windows".to_string(),
        "rust-state-macos".to_string(),
        "rust-native-wake".to_string(),
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
        (
            "rust-state-windows".to_string(),
            "rust-integration".to_string(),
        ),
        (
            "rust-state-macos".to_string(),
            "rust-integration".to_string(),
        ),
        (
            "rust-native-wake".to_string(),
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
    let native_wake_os = sequence_at(
        job(&ci, "rust-native-wake", "ci.yml"),
        "ci.yml jobs.rust-native-wake.strategy.matrix.os",
        &["strategy", "matrix", "os"],
    )
    .iter()
    .map(|value| {
        value
            .as_str()
            .unwrap_or_else(|| panic!("native wake OS must be a string, got {value:?}"))
            .to_string()
    })
    .collect::<BTreeSet<_>>();
    assert_eq!(
        native_wake_os,
        BTreeSet::from(["macos-latest".to_string(), "windows-latest".to_string()]),
        "A10: native wake evidence must execute on both shipped non-Linux operating systems"
    );
    assert_eq!(
        value_at(
            job(&ci, "rust-native-wake", "ci.yml"),
            "ci.yml jobs.rust-native-wake.strategy.fail-fast",
            &["strategy", "fail-fast"],
        )
        .as_bool(),
        Some(false),
        "A10: one native wake leg must not cancel evidence collection for the other"
    );

    let release = release_workflow();
    let release_expected_jobs = BTreeSet::from([
        "prepare-matrix".to_string(),
        "frontend-check".to_string(),
        "rust-lib-check".to_string(),
        "rust-client-check".to_string(),
        "rust-integration".to_string(),
        "version".to_string(),
        "build".to_string(),
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
fn assertion_t3_ci_linux_install_steps_characterize_current_condition() {
    assert_ci_linux_install_step_condition("rust-client-check");
    assert_ci_linux_install_step_condition("rust-integration");
}

#[test]
fn assertion_a14_standalone_binary_release_job_bodies() {
    let release = release_workflow();
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

#[test]
fn assertion_a15_release_version_resolver_uses_cargo_metadata() {
    let release = release_workflow();
    let resolve = step_by_name(&release, "release.yml", "version", "Resolve version");
    let run = string_field(
        resolve,
        "run",
        "A15 release.yml jobs.version.steps[Resolve version].run",
    );

    assert!(
        regex_is_match(
            r"\bcargo\s+metadata\s+--format-version\s+1\s+--no-deps\b",
            run
        ),
        "A15: release.yml Resolve version must invoke cargo metadata --format-version 1 --no-deps, got: {run}"
    );
    assert!(
        run.contains("jq")
            && run.contains(r#"select(.name == "oulipoly-agent-runner")"#)
            && run.contains(".version"),
        "A15: release.yml Resolve version must use jq to select package oulipoly-agent-runner and read .version, got: {run}"
    );
    assert!(
        run.contains(r"^[0-9]+\.[0-9]+\.[0-9]+$"),
        "A15: release.yml Resolve version must validate semver shape ^[0-9]+\\.[0-9]+\\.[0-9]+$, got: {run}"
    );
    assert!(
        run.contains("exit 1") || run.contains("return 1"),
        "A15: release.yml Resolve version must fail when semver validation fails, got: {run}"
    );
    assert!(
        run.contains("git rev-parse \"v${cargo_version}\"")
            || run.contains("git rev-parse \"v${resolved_version}\"")
            || run.contains("git rev-parse \"${tag}\""),
        "A15: release.yml Resolve version must preserve duplicate-tag detection against v-prefixed tag names, got: {run}"
    );
    assert!(
        run.contains(r#"git tag --list "v${major}.${minor}.*""#)
            && run.contains("highest_patch")
            && run.contains("next_patch=$((highest_patch + 1))"),
        "A15: release.yml Resolve version must preserve highest patch bumping for duplicate vX.Y.Z tags, got: {run}"
    );
    assert!(
        run.contains(r#"echo "version=$version" >> "$GITHUB_OUTPUT""#)
            && run.contains(r#"echo "tag=v$version" >> "$GITHUB_OUTPUT""#),
        "A15: release.yml Resolve version must emit version= and tag=v outputs, got: {run}"
    );
    for manifest_path in workspace_members_with_inherited_version() {
        let pattern = format!(r"(?m)\bgrep\s+[^|\n]*\b{}\b", regex::escape(&manifest_path));
        assert!(
            !regex_is_match(&pattern, run),
            "A15: release.yml Resolve version must not grep inheriting manifest {manifest_path:?}, got: {run}"
        );
    }
    assert!(
        !run.contains("version.workspace"),
        "A15: release.yml Resolve version must not contain the inherited version.workspace literal, got: {run}"
    );
}

#[test]
fn assertion_a16_workspace_version_inheritance_contract() {
    let workspace = parse_toml("../../Cargo.toml");
    let workspace_version = toml_value_at(
        &workspace,
        "root Cargo.toml workspace.package.version",
        &["workspace", "package", "version"],
    )
    .as_str()
    .unwrap_or_else(|| panic!("A16: root Cargo.toml [workspace.package].version must be a string"));
    assert!(
        regex_is_match(r"^[0-9]+\.[0-9]+\.[0-9]+$", workspace_version),
        "A16: root Cargo.toml [workspace.package].version must be semver X.Y.Z, got {workspace_version:?}"
    );

    let tauri_manifest = parse_toml("../../src-tauri/Cargo.toml");
    let package_name = toml_value_at(
        &tauri_manifest,
        "src-tauri package.name",
        &["package", "name"],
    )
    .as_str()
    .unwrap_or_else(|| panic!("A16: src-tauri/Cargo.toml package.name must be a string"));
    assert_eq!(
        package_name, "oulipoly-agent-runner",
        "A16: src-tauri/Cargo.toml must remain the oulipoly-agent-runner package"
    );
    let version_workspace = toml_value_at(
        &tauri_manifest,
        "src-tauri package.version.workspace",
        &["package", "version", "workspace"],
    )
    .as_bool()
    .unwrap_or_else(|| {
        panic!("A16: src-tauri/Cargo.toml must declare package.version.workspace as a boolean")
    });
    assert!(
        version_workspace,
        "A16: src-tauri/Cargo.toml must declare version.workspace = true"
    );
}

#[test]
fn assertion_a17_agents_release_process_documented() {
    let agents_path = path_from_test_file("../../AGENTS.md");
    assert!(
        agents_path.exists(),
        "A17: AGENTS.md must exist at {}",
        agents_path.display()
    );
    let body = fs::read_to_string(&agents_path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", agents_path.display()));
    assert!(
        body.lines().any(|line| line == "## Release Process"),
        "A17: AGENTS.md must contain a top-level ## Release Process section"
    );

    let heading_offsets = body
        .lines()
        .scan(0, |offset, line| {
            let line_start = *offset;
            *offset += line.len() + 1;
            Some((line_start, line))
        })
        .collect::<Vec<_>>();
    let release_heading = heading_offsets
        .iter()
        .find_map(|(line_start, line)| (*line == "## Release Process").then_some(*line_start))
        .unwrap_or_else(|| {
            panic!("A17: AGENTS.md must contain a top-level ## Release Process section heading")
        });
    let release_section_end = body[release_heading..]
        .lines()
        .scan(release_heading, |offset, line| {
            let line_start = *offset;
            *offset += line.len() + 1;
            Some((line_start, line))
        })
        .skip(1)
        .find_map(|(line_start, line)| line.starts_with("## ").then_some(line_start))
        .unwrap_or(body.len());
    let release_section = &body[release_heading..release_section_end];

    for needle in [
        "workflow_dispatch",
        "vX.Y.Z",
        "[workspace.package].version",
        "oulipoly-agent-runner",
        "agent-store",
        "agent-scratchpad",
        "agent-messenger",
    ] {
        assert!(
            release_section.contains(needle),
            "A17: AGENTS.md Release Process section must contain {needle:?}"
        );
    }
}
