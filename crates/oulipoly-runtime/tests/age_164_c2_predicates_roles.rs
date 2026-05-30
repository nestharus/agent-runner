//! AGE-164 cluster 2 — declared-role / predicate cluster.
//!
//! This residual file preserves the source-boundary slot for the original
//! declared-role and component-set guards. Predicate behavior that reaches
//! runtime/config/IPC contracts lives in the contract-specific C1, C3, C5,
//! and C6 test files so each adapter declaration remains bounded.
//!
//! ## Declared roles
//!
//! Roles: accessor, mapper, parser, validator.
//!
//! - accessor: source-file reads for the split cli modules.
//! - mapper: expected submodule path construction.
//! - parser: declared-role and adapter-declaration block extraction.
//! - validator: structural assertions for declared-role and adapter carriers.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/tests/age_164_c2_predicates_roles.rs
//!     role: adapter
//!     Translates:
//!       - rust-source-boundary-test-contract
//!       - declared-role-header-contract
//!       - executor-cli-component-set-contract
//! ```

use std::path::PathBuf;

const CLI_MODULES: &[&str] = &[
    "capture_result",
    "headless",
    "input_flags",
    "ipc",
    "launch",
    "policy",
    "provider_execution",
    "provider_lookup",
    "provider_identity",
    "request",
    "result",
    "resume",
    "resume_execution",
    "session_capture",
    "supervision",
    "terminal_signal",
];

const ROLE_VOCABULARY: &[&str] = &[
    "orchestration",
    "mapper",
    "parser",
    "validator",
    "formatter",
    "filter",
    "accessor",
    "predicate",
];

fn cli_module_path(module: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("executor")
        .join("cli")
        .join(format!("{module}.rs"))
}

fn cli_root_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("executor")
        .join("cli.rs")
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates dir")
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

fn coverage_spec_path() -> PathBuf {
    workspace_root()
        .join("planning")
        .join("coverage")
        .join("spec-executor.md")
}

fn cli_root_source() -> String {
    std::fs::read_to_string(cli_root_path()).expect("cli root source readable")
}

fn cli_module_source(module: &str) -> String {
    std::fs::read_to_string(cli_module_path(module)).expect("cli module source readable")
}

fn declared_roles_line(source: &str) -> &str {
    source
        .lines()
        .find(|line| line.trim_start().starts_with("//! Roles:"))
        .expect("declared roles line")
}

fn role_tokens(line: &str) -> Vec<&str> {
    line.trim_start()
        .trim_start_matches("//! Roles:")
        .trim()
        .trim_end_matches('.')
        .split(',')
        .map(str::trim)
        .filter(|role| !role.is_empty())
        .collect()
}

fn declared_role_header_positions(source: &str) -> Option<(usize, Option<usize>)> {
    source
        .find("//! ## Declared roles")
        .map(|roles_header| (roles_header, source.find("\nuse ")))
}

fn assert_declared_role_header_before_imports(label: &str, source: &str) {
    let (roles_header, first_use) = declared_role_header_positions(source)
        .unwrap_or_else(|| panic!("{label} missing declared roles header"));
    assert!(
        first_use.is_none_or(|use_pos| roles_header < use_pos),
        "{label} declared roles header must precede imports"
    );
}

fn assert_declared_roles_use_known_vocabulary(label: &str, source: &str) {
    let roles = role_tokens(declared_roles_line(source));
    assert!(
        !roles.is_empty(),
        "{label} declared roles must not be empty"
    );
    for role in roles {
        assert!(
            ROLE_VOCABULARY.contains(&role),
            "{label} declares unknown role {role}"
        );
    }
}

#[test]
fn cli_root_and_submodules_have_declared_role_headers_before_imports() {
    let root = cli_root_source();
    assert_declared_role_header_before_imports("cli.rs", &root);
    for module in CLI_MODULES {
        let source = cli_module_source(module);
        assert_declared_role_header_before_imports(module, &source);
    }
}

#[test]
fn cli_root_and_submodule_declared_roles_use_known_vocabulary() {
    let root = cli_root_source();
    assert_declared_roles_use_known_vocabulary("cli.rs", &root);
    for module in CLI_MODULES {
        let source = cli_module_source(module);
        assert_declared_roles_use_known_vocabulary(module, &source);
    }
}

fn translates_entries(source: &str) -> Vec<&str> {
    let mut entries = Vec::new();
    let mut in_translates_block = false;
    for line in source.lines() {
        let trimmed = line.trim_start();
        if trimmed == "//!     Translates:" {
            in_translates_block = true;
            continue;
        }
        if in_translates_block {
            if trimmed.starts_with("//!       - ") {
                entries.push(trimmed);
            } else {
                break;
            }
        }
    }
    entries
}

#[test]
fn cli_submodules_have_adapter_declaration_translates_blocks() {
    for module in CLI_MODULES {
        let source = cli_module_source(module);
        assert!(
            source.contains("//! ## Adapter declarations"),
            "{module} missing adapter declarations header"
        );
        assert!(
            source.contains("//!     Translates:"),
            "{module} missing Translates block"
        );
        assert!(
            !translates_entries(&source).is_empty(),
            "{module} Translates block must name at least one contract"
        );
        assert!(
            source.contains(&format!(
                "//!   - component: crates/oulipoly-runtime/src/executor/cli/{module}.rs"
            )),
            "{module} adapter declaration must name the module path"
        );
    }
}

#[test]
fn executor_coverage_spec_lists_required_cli_leaves() {
    let spec = std::fs::read_to_string(coverage_spec_path()).expect("executor spec readable");

    for module in [
        "headless",
        "provider_execution",
        "provider_lookup",
        "request",
        "result",
        "resume_execution",
    ] {
        let expected = format!("- `crates/oulipoly-runtime/src/executor/cli/{module}.rs`");
        assert!(
            spec.contains(&expected),
            "planning/coverage/spec-executor.md must list {module}.rs for executor audit discovery"
        );
    }
}
