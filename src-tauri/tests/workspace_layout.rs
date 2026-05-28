use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;

const WORKSPACE_MEMBERS: &[&str] = &[
    "src-tauri",
    "crates/oulipoly-core",
    "crates/oulipoly-config",
    "crates/oulipoly-state",
    "crates/oulipoly-runtime",
    "crates/oulipoly-setup",
    "crates/oulipoly-provider",
    "crates/oulipoly-agent-store",
    "crates/oulipoly-agent-scratchpad",
    "crates/oulipoly-agent-messenger",
];

const LIB_CRATES: &[&str] = &[
    "oulipoly-core",
    "oulipoly-config",
    "oulipoly-state",
    "oulipoly-runtime",
    "oulipoly-provider",
    "oulipoly-setup",
];

const EXPECTED_EDGES: &[(&str, &str)] = &[
    ("src-tauri", "oulipoly-runtime"),
    ("src-tauri", "oulipoly-setup"),
    ("src-tauri", "oulipoly-state"),
    ("src-tauri", "oulipoly-agent-messenger"),
    ("src-tauri", "oulipoly-config"),
    ("src-tauri", "oulipoly-core"),
    ("src-tauri", "oulipoly-provider"),
    ("oulipoly-runtime", "oulipoly-state"),
    ("oulipoly-runtime", "oulipoly-agent-messenger"),
    ("oulipoly-runtime", "oulipoly-config"),
    ("oulipoly-runtime", "oulipoly-core"),
    ("oulipoly-runtime", "oulipoly-provider"),
    ("oulipoly-setup", "oulipoly-state"),
    ("oulipoly-state", "oulipoly-config"),
    ("oulipoly-state", "oulipoly-agent-messenger"),
    ("oulipoly-state", "oulipoly-core"),
    ("oulipoly-agent-scratchpad", "oulipoly-agent-store"),
    ("oulipoly-agent-messenger", "oulipoly-agent-store"),
    ("oulipoly-agent-messenger", "oulipoly-agent-scratchpad"),
];

const GRAPH_NODES: &[&str] = &[
    "src-tauri",
    "oulipoly-core",
    "oulipoly-config",
    "oulipoly-state",
    "oulipoly-runtime",
    "oulipoly-setup",
    "oulipoly-provider",
    "oulipoly-agent-store",
    "oulipoly-agent-scratchpad",
    "oulipoly-agent-messenger",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri manifest should have a parent repo root")
        .to_path_buf()
}

fn expected_members() -> BTreeSet<String> {
    WORKSPACE_MEMBERS
        .iter()
        .map(|member| member.to_string())
        .collect()
}

fn expected_edges() -> BTreeSet<(String, String)> {
    EXPECTED_EDGES
        .iter()
        .map(|(dependent, dependency)| (dependent.to_string(), dependency.to_string()))
        .collect()
}

fn graph_nodes() -> BTreeSet<String> {
    GRAPH_NODES.iter().map(|node| node.to_string()).collect()
}

fn metadata(root: &Path, args: &[&str]) -> Value {
    let output = Command::new(env!("CARGO"))
        .args(args)
        .current_dir(root)
        .output()
        .expect("failed to execute cargo metadata");

    assert!(
        output.status.success(),
        "cargo metadata failed with status {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    serde_json::from_slice(&output.stdout).expect("cargo metadata stdout should be JSON")
}

fn package_name(package: &Value) -> &str {
    package
        .get("name")
        .and_then(Value::as_str)
        .expect("metadata package should have a string name")
}

fn package_id(package: &Value) -> &str {
    package
        .get("id")
        .and_then(Value::as_str)
        .expect("metadata package should have a string id")
}

fn edge_label(package_name: &str) -> String {
    if package_name == "oulipoly-agent-runner" {
        "src-tauri".to_string()
    } else {
        package_name.to_string()
    }
}

fn workspace_edge_set(root: &Path, include_dev: bool) -> BTreeSet<(String, String)> {
    let metadata = metadata(root, &["metadata", "--format-version", "1"]);
    let workspace_ids: BTreeSet<&str> = metadata
        .get("workspace_members")
        .and_then(Value::as_array)
        .expect("metadata should include workspace_members")
        .iter()
        .map(|id| id.as_str().expect("workspace member id should be a string"))
        .collect();

    let packages = metadata
        .get("packages")
        .and_then(Value::as_array)
        .expect("metadata should include packages");

    let workspace_names: BTreeSet<String> = packages
        .iter()
        .filter(|package| workspace_ids.contains(package_id(package)))
        .map(|package| package_name(package).to_string())
        .collect();

    let mut edges = BTreeSet::new();

    for package in packages
        .iter()
        .filter(|package| workspace_ids.contains(package_id(package)))
    {
        let dependent = edge_label(package_name(package));
        let dependencies = package
            .get("dependencies")
            .and_then(Value::as_array)
            .expect("metadata package should include dependencies");

        for dependency in dependencies.iter().filter(|dependency| {
            dependency.get("path").is_some()
                && (include_dev || dependency.get("kind").and_then(Value::as_str) != Some("dev"))
        }) {
            let dependency_name = dependency
                .get("name")
                .and_then(Value::as_str)
                .expect("metadata dependency should have a string name");

            if workspace_names.contains(dependency_name) {
                edges.insert((dependent.clone(), dependency_name.to_string()));
            }
        }
    }

    edges
}

#[test]
fn workspace_members_exact_set() {
    let manifest_path = repo_root().join("Cargo.toml");
    let manifest = std::fs::read_to_string(&manifest_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", manifest_path.display()));
    let parsed: toml::Value = toml::from_str(&manifest)
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", manifest_path.display()));
    let members: BTreeSet<String> = parsed
        .get("workspace")
        .and_then(|workspace| workspace.get("members"))
        .and_then(toml::Value::as_array)
        .expect("root Cargo.toml should define [workspace] members")
        .iter()
        .map(|member| {
            member
                .as_str()
                .expect("workspace member should be a string")
                .to_string()
        })
        .collect();

    assert_eq!(members, expected_members());
}

#[test]
fn workspace_default_members_exact_set() {
    let manifest_path = repo_root().join("Cargo.toml");
    let manifest = std::fs::read_to_string(&manifest_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", manifest_path.display()));
    let parsed: toml::Value = toml::from_str(&manifest)
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", manifest_path.display()));
    let default_members: BTreeSet<String> = parsed
        .get("workspace")
        .and_then(|workspace| workspace.get("default-members"))
        .and_then(toml::Value::as_array)
        .expect("root Cargo.toml should define [workspace] default-members")
        .iter()
        .map(|member| {
            member
                .as_str()
                .expect("workspace default-member should be a string")
                .to_string()
        })
        .collect();

    assert_eq!(default_members, expected_members());
}

#[test]
fn lib_crates_resolve_via_metadata() {
    let root = repo_root();
    let metadata = metadata(&root, &["metadata", "--format-version", "1", "--no-deps"]);
    let packages = metadata
        .get("packages")
        .and_then(Value::as_array)
        .expect("cargo metadata should include packages");

    for crate_name in LIB_CRATES {
        let package = packages
            .iter()
            .find(|package| package_name(package) == *crate_name)
            .unwrap_or_else(|| panic!("missing workspace package in metadata: {crate_name}"));
        let manifest_path = package
            .get("manifest_path")
            .and_then(Value::as_str)
            .expect("metadata package should include a manifest_path");
        let expected_suffix = format!("/crates/{crate_name}/Cargo.toml");

        assert_eq!(package_name(package), *crate_name);
        assert!(
            manifest_path.ends_with(&expected_suffix),
            "{crate_name} manifest_path should end with {expected_suffix}, got {manifest_path}"
        );
    }
}

#[test]
fn binary_target_resolves() {
    let binary_path = Path::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));

    assert!(
        !binary_path.as_os_str().is_empty(),
        "CARGO_BIN_EXE_oulipoly-agent-runner should be non-empty"
    );
    assert!(
        binary_path
            .components()
            .any(|component| component.as_os_str() == "target"),
        "binary path should resolve under target/: {}",
        binary_path.display()
    );
    assert!(
        binary_path.exists(),
        "binary target should exist at test runtime: {}",
        binary_path.display()
    );
}

#[test]
fn workspace_excludes_removed_agent_cli_and_keeps_artifact_tools() {
    let manifest_path = repo_root().join("Cargo.toml");
    let manifest = std::fs::read_to_string(&manifest_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", manifest_path.display()));
    let parsed: toml::Value = toml::from_str(&manifest)
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", manifest_path.display()));
    let workspace = parsed
        .get("workspace")
        .expect("root Cargo.toml should define [workspace]");

    for key in ["members", "default-members"] {
        let entries: BTreeSet<String> = workspace
            .get(key)
            .and_then(toml::Value::as_array)
            .unwrap_or_else(|| panic!("workspace should define {key}"))
            .iter()
            .map(|entry| {
                entry
                    .as_str()
                    .unwrap_or_else(|| panic!("workspace {key} entry should be a string"))
                    .to_string()
            })
            .collect();

        assert!(
            !entries.contains("crates/oulipoly-agent-cli"),
            "workspace {key} should not include removed crates/oulipoly-agent-cli: {entries:?}"
        );

        for expected in [
            "crates/oulipoly-agent-store",
            "crates/oulipoly-agent-scratchpad",
            "crates/oulipoly-agent-messenger",
        ] {
            assert!(
                entries.contains(expected),
                "workspace {key} should include {expected}: {entries:?}"
            );
        }
    }
}

#[test]
fn agent_binary_targets_have_expected_names() {
    let root = repo_root();
    let metadata = metadata(&root, &["metadata", "--format-version", "1", "--no-deps"]);
    let packages = metadata
        .get("packages")
        .and_then(Value::as_array)
        .expect("cargo metadata should include packages");

    assert!(
        packages
            .iter()
            .all(|package| self::package_name(package) != "oulipoly-agent-cli"),
        "metadata should not include removed oulipoly-agent-cli package"
    );

    for package in packages {
        let targets = package
            .get("targets")
            .and_then(Value::as_array)
            .expect("metadata package should include targets");
        assert!(
            targets.iter().all(|target| {
                let name = target.get("name").and_then(Value::as_str);
                let kinds = target.get("kind").and_then(Value::as_array);
                !(name == Some("agent")
                    && kinds.is_some_and(|kinds| kinds.iter().any(|kind| kind == "bin")))
            }),
            "metadata should not include removed agent binary target in package {}: {targets:?}",
            self::package_name(package)
        );
    }

    for (package_name, expected_binary) in [
        ("oulipoly-agent-store", "agent-store"),
        ("oulipoly-agent-scratchpad", "agent-scratchpad"),
        ("oulipoly-agent-messenger", "agent-messenger"),
    ] {
        let package = packages
            .iter()
            .find(|package| self::package_name(package) == package_name)
            .unwrap_or_else(|| panic!("metadata should include {package_name}"));
        let targets = package
            .get("targets")
            .and_then(Value::as_array)
            .expect("metadata package should include targets");

        assert!(
            targets.iter().any(|target| {
                let name = target.get("name").and_then(Value::as_str);
                let kinds = target.get("kind").and_then(Value::as_array);
                name == Some(expected_binary)
                    && kinds.is_some_and(|kinds| kinds.iter().any(|kind| kind == "bin"))
            }),
            "{package_name} should expose a binary target named {expected_binary}: {targets:?}"
        );
    }
}

#[test]
fn no_binary_name_collision() {
    let root = repo_root();
    let metadata = metadata(&root, &["metadata", "--format-version", "1", "--no-deps"]);
    let packages = metadata
        .get("packages")
        .and_then(Value::as_array)
        .expect("cargo metadata should include packages");
    let mut owners_by_binary_name: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for package in packages {
        let package_name = package_name(package).to_string();
        let targets = package
            .get("targets")
            .and_then(Value::as_array)
            .expect("metadata package should include targets");

        for target in targets {
            let kinds = target
                .get("kind")
                .and_then(Value::as_array)
                .expect("metadata target should include kind");
            if kinds.iter().any(|kind| kind == "bin") {
                let name = target
                    .get("name")
                    .and_then(Value::as_str)
                    .expect("metadata binary target should include name");
                owners_by_binary_name
                    .entry(name.to_string())
                    .or_default()
                    .push(package_name.clone());
            }
        }
    }

    let collisions: BTreeMap<_, _> = owners_by_binary_name
        .into_iter()
        .filter(|(_, owners)| owners.len() > 1)
        .collect();

    assert!(
        collisions.is_empty(),
        "workspace binary names should be unique: {collisions:?}"
    );
}

#[test]
fn dep_graph_exact_match() {
    let edges = workspace_edge_set(&repo_root(), true);

    assert_eq!(edges, expected_edges());
}

#[test]
fn dep_graph_acyclic() {
    let edges = workspace_edge_set(&repo_root(), false);
    let mut nodes = graph_nodes();

    for (dependent, dependency) in &edges {
        nodes.insert(dependent.clone());
        nodes.insert(dependency.clone());
    }

    let mut outgoing: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut indegree: BTreeMap<String, usize> =
        nodes.iter().map(|node| (node.clone(), 0)).collect();

    for (dependent, dependency) in &edges {
        outgoing
            .entry(dependent.clone())
            .or_default()
            .insert(dependency.clone());
        *indegree.entry(dependency.clone()).or_insert(0) += 1;
    }

    let mut ready: VecDeque<String> = indegree
        .iter()
        .filter_map(|(node, count)| (*count == 0).then(|| node.clone()))
        .collect();
    let mut visited = 0;

    while let Some(node) = ready.pop_front() {
        visited += 1;
        if let Some(dependencies) = outgoing.get(&node) {
            for dependency in dependencies {
                let count = indegree
                    .get_mut(dependency)
                    .expect("dependency should have an indegree entry");
                *count -= 1;
                if *count == 0 {
                    ready.push_back(dependency.clone());
                }
            }
        }
    }

    assert_eq!(
        visited,
        nodes.len(),
        "workspace dependency graph contains a cycle: {edges:?}"
    );
}
