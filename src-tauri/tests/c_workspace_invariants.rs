use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const APP_PACKAGE: &str = "agent-runner-app";

const LIBRARY_PACKAGES: &[&str] = &[
    "agent-runner-config",
    "agent-runner-state",
    "agent-runner-executor",
    "agent-runner-balancer",
    "agent-runner-quota",
    "agent-runner-diagnostics",
    "agent-runner-runtime",
    "agent-runner-session",
    "agent-runner-schema-probe",
    "agent-runner-discovery",
];

const WORKSPACE_PACKAGES: &[&str] = &[
    "agent-runner-config",
    "agent-runner-state",
    "agent-runner-executor",
    "agent-runner-balancer",
    "agent-runner-quota",
    "agent-runner-diagnostics",
    "agent-runner-runtime",
    "agent-runner-session",
    "agent-runner-schema-probe",
    "agent-runner-discovery",
    APP_PACKAGE,
];

#[test]
fn workspace_metadata_matches_contract_boundaries() {
    let metadata = cargo_metadata();
    let package_by_id = package_names_by_id(&metadata);
    let member_names = workspace_member_names(&metadata, &package_by_id);
    let expected = package_set(WORKSPACE_PACKAGES);

    assert_eq!(
        member_names, expected,
        "workspace member package names must match contract section 2.2"
    );

    for package in ["agent-runner-state", "agent-runner-config", APP_PACKAGE] {
        assert!(
            member_names.contains(package),
            "metadata is missing required package {package}"
        );
    }

    let dependency_graph = workspace_dependency_graph(&metadata, &member_names);

    assert_not_in_dependency_tree(
        &dependency_graph,
        "agent-runner-state",
        "agent-runner-balancer",
    );
    assert_not_in_dependency_tree(
        &dependency_graph,
        "agent-runner-config",
        "agent-runner-state",
    );

    let app_deps = dependency_graph
        .get(APP_PACKAGE)
        .unwrap_or_else(|| panic!("metadata is missing dependency data for {APP_PACKAGE}"));
    assert!(
        app_deps.contains("tauri"),
        "{APP_PACKAGE} must depend on tauri"
    );
    assert!(
        app_deps.contains("tauri-build"),
        "{APP_PACKAGE} must keep the Tauri build dependency"
    );
    for library_package in LIBRARY_PACKAGES {
        assert!(
            app_deps.contains(*library_package),
            "{APP_PACKAGE} must depend on extracted library package {library_package}"
        );
    }
}

#[test]
#[ignore = "slow workspace build invariant; run with `cargo test --ignored`"]
fn workspace_build_succeeds() {
    let output = Command::new("cargo")
        .arg("build")
        .arg("--workspace")
        .current_dir(workspace_root())
        .output()
        .expect("failed to invoke `cargo build --workspace`");

    assert!(
        output.status.success(),
        "`cargo build --workspace` failed\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn cargo_metadata() -> Value {
    let output = Command::new("cargo")
        .arg("metadata")
        .arg("--format-version")
        .arg("1")
        .current_dir(workspace_root())
        .output()
        .expect("failed to invoke `cargo metadata --format-version 1`");

    assert!(
        output.status.success(),
        "`cargo metadata --format-version 1` failed\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "`cargo metadata --format-version 1` did not emit valid JSON: {err}\nstdout:\n{}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn package_names_by_id(metadata: &Value) -> BTreeMap<String, String> {
    metadata["packages"]
        .as_array()
        .expect("cargo metadata `packages` must be an array")
        .iter()
        .map(|package| {
            let id = package["id"]
                .as_str()
                .expect("cargo metadata package id must be a string")
                .to_string();
            let name = package["name"]
                .as_str()
                .expect("cargo metadata package name must be a string")
                .to_string();
            (id, name)
        })
        .collect()
}

fn workspace_member_names(
    metadata: &Value,
    package_by_id: &BTreeMap<String, String>,
) -> BTreeSet<String> {
    metadata["workspace_members"]
        .as_array()
        .expect("cargo metadata `workspace_members` must be an array")
        .iter()
        .map(|member_id| {
            let member_id = member_id
                .as_str()
                .expect("workspace member id must be a string");
            package_by_id
                .get(member_id)
                .unwrap_or_else(|| panic!("workspace member id {member_id} not found in packages"))
                .clone()
        })
        .collect()
}

fn workspace_dependency_graph(
    metadata: &Value,
    workspace_packages: &BTreeSet<String>,
) -> BTreeMap<String, BTreeSet<String>> {
    metadata["packages"]
        .as_array()
        .expect("cargo metadata `packages` must be an array")
        .iter()
        .filter_map(|package| {
            let name = package["name"]
                .as_str()
                .expect("cargo metadata package name must be a string");
            if !workspace_packages.contains(name) {
                return None;
            }

            let dependencies = package["dependencies"]
                .as_array()
                .expect("cargo metadata package dependencies must be an array")
                .iter()
                .filter_map(|dependency| {
                    let dependency_name = dependency["name"]
                        .as_str()
                        .expect("cargo metadata dependency name must be a string");
                    let dependency_alias = dependency["rename"].as_str();

                    if workspace_packages.contains(dependency_name) {
                        Some(dependency_name.to_string())
                    } else if dependency_alias
                        .is_some_and(|alias| workspace_packages.contains(alias))
                    {
                        Some(dependency_alias.unwrap().to_string())
                    } else if dependency_name == "tauri" || dependency_name == "tauri-build" {
                        Some(dependency_name.to_string())
                    } else {
                        None
                    }
                })
                .collect();

            Some((name.to_string(), dependencies))
        })
        .collect()
}

fn assert_not_in_dependency_tree(
    dependency_graph: &BTreeMap<String, BTreeSet<String>>,
    root: &str,
    forbidden: &str,
) {
    assert!(
        dependency_graph.contains_key(root),
        "metadata is missing required package {root}"
    );
    assert!(
        dependency_graph.contains_key(forbidden),
        "metadata is missing required package {forbidden}"
    );
    assert!(
        !depends_on(dependency_graph, root, forbidden),
        "{root} must not have {forbidden} anywhere in its dependency tree"
    );
}

fn depends_on(
    dependency_graph: &BTreeMap<String, BTreeSet<String>>,
    root: &str,
    target: &str,
) -> bool {
    let mut visited = BTreeSet::new();
    let mut stack = dependency_graph
        .get(root)
        .into_iter()
        .flat_map(|dependencies| dependencies.iter().map(String::as_str))
        .collect::<Vec<_>>();

    while let Some(package) = stack.pop() {
        if package == target {
            return true;
        }
        if !visited.insert(package) {
            continue;
        }
        if let Some(dependencies) = dependency_graph.get(package) {
            stack.extend(dependencies.iter().map(String::as_str));
        }
    }

    false
}

fn package_set(packages: &[&str]) -> BTreeSet<String> {
    packages
        .iter()
        .map(|package| (*package).to_string())
        .collect()
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri manifest must have a repository root parent")
        .to_path_buf()
}
