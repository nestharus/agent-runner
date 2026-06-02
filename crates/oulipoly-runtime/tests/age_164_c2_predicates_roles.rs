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

#[derive(Clone, Copy)]
struct CliSource {
    label: &'static str,
    path: &'static str,
}

impl CliSource {
    fn full_path(self) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(self.path)
    }

    fn component_path(self) -> String {
        format!("crates/oulipoly-runtime/{}", self.path)
    }
}

const CLI_SOURCES: &[CliSource] = &[
    CliSource {
        label: "capture_result.rs",
        path: "src/executor/cli/capture_result.rs",
    },
    CliSource {
        label: "headless.rs",
        path: "src/executor/cli/headless.rs",
    },
    CliSource {
        label: "input_flags/mod.rs",
        path: "src/executor/cli/input_flags/mod.rs",
    },
    CliSource {
        label: "input_flags/validate.rs",
        path: "src/executor/cli/input_flags/validate.rs",
    },
    CliSource {
        label: "input_flags/parse.rs",
        path: "src/executor/cli/input_flags/parse.rs",
    },
    CliSource {
        label: "input_flags/format.rs",
        path: "src/executor/cli/input_flags/format.rs",
    },
    CliSource {
        label: "input_flags/messages.rs",
        path: "src/executor/cli/input_flags/messages.rs",
    },
    CliSource {
        label: "input_flags/schema_access.rs",
        path: "src/executor/cli/input_flags/schema_access.rs",
    },
    CliSource {
        label: "input_flags/predicates.rs",
        path: "src/executor/cli/input_flags/predicates.rs",
    },
    CliSource {
        label: "interactive.rs",
        path: "src/executor/cli/interactive.rs",
    },
    CliSource {
        label: "ipc/mod.rs",
        path: "src/executor/cli/ipc/mod.rs",
    },
    CliSource {
        label: "ipc/return_channel.rs",
        path: "src/executor/cli/ipc/return_channel.rs",
    },
    CliSource {
        label: "ipc/return_channel_path.rs",
        path: "src/executor/cli/ipc/return_channel_path.rs",
    },
    CliSource {
        label: "ipc/return_channel_jsonl.rs",
        path: "src/executor/cli/ipc/return_channel_jsonl.rs",
    },
    CliSource {
        label: "ipc/return_channel_parent.rs",
        path: "src/executor/cli/ipc/return_channel_parent.rs",
    },
    CliSource {
        label: "ipc/return_channel_cleanup.rs",
        path: "src/executor/cli/ipc/return_channel_cleanup.rs",
    },
    CliSource {
        label: "ipc/return_channel_warnings.rs",
        path: "src/executor/cli/ipc/return_channel_warnings.rs",
    },
    CliSource {
        label: "ipc/return_channel_predicates.rs",
        path: "src/executor/cli/ipc/return_channel_predicates.rs",
    },
    CliSource {
        label: "ipc/captured_child_marker.rs",
        path: "src/executor/cli/ipc/captured_child_marker.rs",
    },
    CliSource {
        label: "ipc/captured_child_dedupe.rs",
        path: "src/executor/cli/ipc/captured_child_dedupe.rs",
    },
    CliSource {
        label: "launch/mod.rs",
        path: "src/executor/cli/launch/mod.rs",
    },
    CliSource {
        label: "launch/command.rs",
        path: "src/executor/cli/launch/command.rs",
    },
    CliSource {
        label: "launch/command_format.rs",
        path: "src/executor/cli/launch/command_format.rs",
    },
    CliSource {
        label: "launch/command_parse.rs",
        path: "src/executor/cli/launch/command_parse.rs",
    },
    CliSource {
        label: "launch/command_validate.rs",
        path: "src/executor/cli/launch/command_validate.rs",
    },
    CliSource {
        label: "launch/prompt.rs",
        path: "src/executor/cli/launch/prompt.rs",
    },
    CliSource {
        label: "launch/prompt_file.rs",
        path: "src/executor/cli/launch/prompt_file.rs",
    },
    CliSource {
        label: "launch/prompt_format.rs",
        path: "src/executor/cli/launch/prompt_format.rs",
    },
    CliSource {
        label: "launch/prompt_path.rs",
        path: "src/executor/cli/launch/prompt_path.rs",
    },
    CliSource {
        label: "launch/prompt_predicates.rs",
        path: "src/executor/cli/launch/prompt_predicates.rs",
    },
    CliSource {
        label: "launch/capture.rs",
        path: "src/executor/cli/launch/capture.rs",
    },
    CliSource {
        label: "launch/supervisor_config.rs",
        path: "src/executor/cli/launch/supervisor_config.rs",
    },
    CliSource {
        label: "policy.rs",
        path: "src/executor/cli/policy.rs",
    },
    CliSource {
        label: "policy/messages.rs",
        path: "src/executor/cli/policy/messages.rs",
    },
    CliSource {
        label: "policy/orchestration.rs",
        path: "src/executor/cli/policy/orchestration.rs",
    },
    CliSource {
        label: "policy/predicates.rs",
        path: "src/executor/cli/policy/predicates.rs",
    },
    CliSource {
        label: "policy/validation.rs",
        path: "src/executor/cli/policy/validation.rs",
    },
    CliSource {
        label: "provider_execution.rs",
        path: "src/executor/cli/provider_execution.rs",
    },
    CliSource {
        label: "provider_lookup.rs",
        path: "src/executor/cli/provider_lookup.rs",
    },
    CliSource {
        label: "provider_identity.rs",
        path: "src/executor/cli/provider_identity.rs",
    },
    CliSource {
        label: "request.rs",
        path: "src/executor/cli/request.rs",
    },
    CliSource {
        label: "result.rs",
        path: "src/executor/cli/result.rs",
    },
    CliSource {
        label: "resume.rs",
        path: "src/executor/cli/resume.rs",
    },
    CliSource {
        label: "resume/acceptance.rs",
        path: "src/executor/cli/resume/acceptance.rs",
    },
    CliSource {
        label: "resume/args.rs",
        path: "src/executor/cli/resume/args.rs",
    },
    CliSource {
        label: "resume/messages.rs",
        path: "src/executor/cli/resume/messages.rs",
    },
    CliSource {
        label: "resume/output.rs",
        path: "src/executor/cli/resume/output.rs",
    },
    CliSource {
        label: "resume/patterns.rs",
        path: "src/executor/cli/resume/patterns.rs",
    },
    CliSource {
        label: "resume_execution.rs",
        path: "src/executor/cli/resume_execution.rs",
    },
    CliSource {
        label: "session_capture.rs",
        path: "src/executor/cli/session_capture.rs",
    },
    CliSource {
        label: "session_capture/args.rs",
        path: "src/executor/cli/session_capture/args.rs",
    },
    CliSource {
        label: "session_capture/json_path.rs",
        path: "src/executor/cli/session_capture/json_path.rs",
    },
    CliSource {
        label: "session_capture/messages.rs",
        path: "src/executor/cli/session_capture/messages.rs",
    },
    CliSource {
        label: "session_capture/parse_forced_flag.rs",
        path: "src/executor/cli/session_capture/parse_forced_flag.rs",
    },
    CliSource {
        label: "session_capture/parse_stdout_event.rs",
        path: "src/executor/cli/session_capture/parse_stdout_event.rs",
    },
    CliSource {
        label: "session_capture/paths.rs",
        path: "src/executor/cli/session_capture/paths.rs",
    },
    CliSource {
        label: "session_capture/plan.rs",
        path: "src/executor/cli/session_capture/plan.rs",
    },
    CliSource {
        label: "session_capture/start_known.rs",
        path: "src/executor/cli/session_capture/start_known.rs",
    },
    CliSource {
        label: "supervision/mod.rs",
        path: "src/executor/cli/supervision/mod.rs",
    },
    CliSource {
        label: "supervision/process.rs",
        path: "src/executor/cli/supervision/process.rs",
    },
    CliSource {
        label: "supervision/process_validate.rs",
        path: "src/executor/cli/supervision/process_validate.rs",
    },
    CliSource {
        label: "supervision/stdin.rs",
        path: "src/executor/cli/supervision/stdin.rs",
    },
    CliSource {
        label: "supervision/drain.rs",
        path: "src/executor/cli/supervision/drain.rs",
    },
    CliSource {
        label: "supervision/drain_access.rs",
        path: "src/executor/cli/supervision/drain_access.rs",
    },
    CliSource {
        label: "supervision/drain_chunks.rs",
        path: "src/executor/cli/supervision/drain_chunks.rs",
    },
    CliSource {
        label: "supervision/errors.rs",
        path: "src/executor/cli/supervision/errors.rs",
    },
    CliSource {
        label: "supervision/live_quota.rs",
        path: "src/executor/cli/supervision/live_quota.rs",
    },
    CliSource {
        label: "supervision/predicates.rs",
        path: "src/executor/cli/supervision/predicates.rs",
    },
    CliSource {
        label: "supervision/status.rs",
        path: "src/executor/cli/supervision/status.rs",
    },
    CliSource {
        label: "supervision/terminal_outcome.rs",
        path: "src/executor/cli/supervision/terminal_outcome.rs",
    },
    CliSource {
        label: "supervision/stdin_access.rs",
        path: "src/executor/cli/supervision/stdin_access.rs",
    },
    CliSource {
        label: "supervision/stdin_predicates.rs",
        path: "src/executor/cli/supervision/stdin_predicates.rs",
    },
    CliSource {
        label: "supervision/termination.rs",
        path: "src/executor/cli/supervision/termination.rs",
    },
    CliSource {
        label: "terminal_signal.rs",
        path: "src/executor/cli/terminal_signal.rs",
    },
    CliSource {
        label: "provider_specific/mod.rs",
        path: "src/executor/provider_specific/mod.rs",
    },
    CliSource {
        label: "provider_specific/policy/mod.rs",
        path: "src/executor/provider_specific/policy/mod.rs",
    },
    CliSource {
        label: "provider_specific/policy/claude.rs",
        path: "src/executor/provider_specific/policy/claude.rs",
    },
    CliSource {
        label: "provider_specific/policy/codex.rs",
        path: "src/executor/provider_specific/policy/codex.rs",
    },
    CliSource {
        label: "provider_specific/resume_acceptance.rs",
        path: "src/executor/provider_specific/resume_acceptance.rs",
    },
    CliSource {
        label: "provider_specific/session_capture/mod.rs",
        path: "src/executor/provider_specific/session_capture/mod.rs",
    },
    CliSource {
        label: "provider_specific/session_capture/telemetry_scrub.rs",
        path: "src/executor/provider_specific/session_capture/telemetry_scrub.rs",
    },
];

const E4_SPLIT_SOURCES: &[CliSource] = &[
    CliSource {
        label: "input_flags/mod.rs",
        path: "src/executor/cli/input_flags/mod.rs",
    },
    CliSource {
        label: "input_flags/validate.rs",
        path: "src/executor/cli/input_flags/validate.rs",
    },
    CliSource {
        label: "input_flags/parse.rs",
        path: "src/executor/cli/input_flags/parse.rs",
    },
    CliSource {
        label: "input_flags/format.rs",
        path: "src/executor/cli/input_flags/format.rs",
    },
    CliSource {
        label: "input_flags/messages.rs",
        path: "src/executor/cli/input_flags/messages.rs",
    },
    CliSource {
        label: "input_flags/schema_access.rs",
        path: "src/executor/cli/input_flags/schema_access.rs",
    },
    CliSource {
        label: "input_flags/predicates.rs",
        path: "src/executor/cli/input_flags/predicates.rs",
    },
    CliSource {
        label: "ipc/mod.rs",
        path: "src/executor/cli/ipc/mod.rs",
    },
    CliSource {
        label: "ipc/return_channel.rs",
        path: "src/executor/cli/ipc/return_channel.rs",
    },
    CliSource {
        label: "ipc/return_channel_path.rs",
        path: "src/executor/cli/ipc/return_channel_path.rs",
    },
    CliSource {
        label: "ipc/return_channel_jsonl.rs",
        path: "src/executor/cli/ipc/return_channel_jsonl.rs",
    },
    CliSource {
        label: "ipc/return_channel_parent.rs",
        path: "src/executor/cli/ipc/return_channel_parent.rs",
    },
    CliSource {
        label: "ipc/return_channel_cleanup.rs",
        path: "src/executor/cli/ipc/return_channel_cleanup.rs",
    },
    CliSource {
        label: "ipc/return_channel_warnings.rs",
        path: "src/executor/cli/ipc/return_channel_warnings.rs",
    },
    CliSource {
        label: "ipc/return_channel_predicates.rs",
        path: "src/executor/cli/ipc/return_channel_predicates.rs",
    },
    CliSource {
        label: "ipc/captured_child_marker.rs",
        path: "src/executor/cli/ipc/captured_child_marker.rs",
    },
    CliSource {
        label: "ipc/captured_child_dedupe.rs",
        path: "src/executor/cli/ipc/captured_child_dedupe.rs",
    },
    CliSource {
        label: "launch/mod.rs",
        path: "src/executor/cli/launch/mod.rs",
    },
    CliSource {
        label: "launch/command.rs",
        path: "src/executor/cli/launch/command.rs",
    },
    CliSource {
        label: "launch/command_format.rs",
        path: "src/executor/cli/launch/command_format.rs",
    },
    CliSource {
        label: "launch/command_parse.rs",
        path: "src/executor/cli/launch/command_parse.rs",
    },
    CliSource {
        label: "launch/command_validate.rs",
        path: "src/executor/cli/launch/command_validate.rs",
    },
    CliSource {
        label: "launch/prompt.rs",
        path: "src/executor/cli/launch/prompt.rs",
    },
    CliSource {
        label: "launch/prompt_file.rs",
        path: "src/executor/cli/launch/prompt_file.rs",
    },
    CliSource {
        label: "launch/prompt_format.rs",
        path: "src/executor/cli/launch/prompt_format.rs",
    },
    CliSource {
        label: "launch/prompt_path.rs",
        path: "src/executor/cli/launch/prompt_path.rs",
    },
    CliSource {
        label: "launch/prompt_predicates.rs",
        path: "src/executor/cli/launch/prompt_predicates.rs",
    },
    CliSource {
        label: "launch/capture.rs",
        path: "src/executor/cli/launch/capture.rs",
    },
    CliSource {
        label: "launch/supervisor_config.rs",
        path: "src/executor/cli/launch/supervisor_config.rs",
    },
    CliSource {
        label: "supervision/mod.rs",
        path: "src/executor/cli/supervision/mod.rs",
    },
    CliSource {
        label: "supervision/process.rs",
        path: "src/executor/cli/supervision/process.rs",
    },
    CliSource {
        label: "supervision/process_validate.rs",
        path: "src/executor/cli/supervision/process_validate.rs",
    },
    CliSource {
        label: "supervision/stdin.rs",
        path: "src/executor/cli/supervision/stdin.rs",
    },
    CliSource {
        label: "supervision/drain.rs",
        path: "src/executor/cli/supervision/drain.rs",
    },
    CliSource {
        label: "supervision/drain_access.rs",
        path: "src/executor/cli/supervision/drain_access.rs",
    },
    CliSource {
        label: "supervision/drain_chunks.rs",
        path: "src/executor/cli/supervision/drain_chunks.rs",
    },
    CliSource {
        label: "supervision/errors.rs",
        path: "src/executor/cli/supervision/errors.rs",
    },
    CliSource {
        label: "supervision/live_quota.rs",
        path: "src/executor/cli/supervision/live_quota.rs",
    },
    CliSource {
        label: "supervision/predicates.rs",
        path: "src/executor/cli/supervision/predicates.rs",
    },
    CliSource {
        label: "supervision/status.rs",
        path: "src/executor/cli/supervision/status.rs",
    },
    CliSource {
        label: "supervision/terminal_outcome.rs",
        path: "src/executor/cli/supervision/terminal_outcome.rs",
    },
    CliSource {
        label: "supervision/stdin_access.rs",
        path: "src/executor/cli/supervision/stdin_access.rs",
    },
    CliSource {
        label: "supervision/stdin_predicates.rs",
        path: "src/executor/cli/supervision/stdin_predicates.rs",
    },
    CliSource {
        label: "supervision/termination.rs",
        path: "src/executor/cli/supervision/termination.rs",
    },
];

const E5_SPLIT_SOURCES: &[CliSource] = &[
    CliSource {
        label: "policy/messages.rs",
        path: "src/executor/cli/policy/messages.rs",
    },
    CliSource {
        label: "policy/orchestration.rs",
        path: "src/executor/cli/policy/orchestration.rs",
    },
    CliSource {
        label: "policy/predicates.rs",
        path: "src/executor/cli/policy/predicates.rs",
    },
    CliSource {
        label: "policy/validation.rs",
        path: "src/executor/cli/policy/validation.rs",
    },
    CliSource {
        label: "resume/acceptance.rs",
        path: "src/executor/cli/resume/acceptance.rs",
    },
    CliSource {
        label: "resume/args.rs",
        path: "src/executor/cli/resume/args.rs",
    },
    CliSource {
        label: "resume/messages.rs",
        path: "src/executor/cli/resume/messages.rs",
    },
    CliSource {
        label: "resume/output.rs",
        path: "src/executor/cli/resume/output.rs",
    },
    CliSource {
        label: "resume/patterns.rs",
        path: "src/executor/cli/resume/patterns.rs",
    },
    CliSource {
        label: "session_capture/args.rs",
        path: "src/executor/cli/session_capture/args.rs",
    },
    CliSource {
        label: "session_capture/json_path.rs",
        path: "src/executor/cli/session_capture/json_path.rs",
    },
    CliSource {
        label: "session_capture/messages.rs",
        path: "src/executor/cli/session_capture/messages.rs",
    },
    CliSource {
        label: "session_capture/parse_forced_flag.rs",
        path: "src/executor/cli/session_capture/parse_forced_flag.rs",
    },
    CliSource {
        label: "session_capture/parse_stdout_event.rs",
        path: "src/executor/cli/session_capture/parse_stdout_event.rs",
    },
    CliSource {
        label: "session_capture/paths.rs",
        path: "src/executor/cli/session_capture/paths.rs",
    },
    CliSource {
        label: "session_capture/plan.rs",
        path: "src/executor/cli/session_capture/plan.rs",
    },
    CliSource {
        label: "session_capture/start_known.rs",
        path: "src/executor/cli/session_capture/start_known.rs",
    },
    CliSource {
        label: "provider_specific/mod.rs",
        path: "src/executor/provider_specific/mod.rs",
    },
    CliSource {
        label: "provider_specific/policy/mod.rs",
        path: "src/executor/provider_specific/policy/mod.rs",
    },
    CliSource {
        label: "provider_specific/policy/claude.rs",
        path: "src/executor/provider_specific/policy/claude.rs",
    },
    CliSource {
        label: "provider_specific/policy/codex.rs",
        path: "src/executor/provider_specific/policy/codex.rs",
    },
    CliSource {
        label: "provider_specific/resume_acceptance.rs",
        path: "src/executor/provider_specific/resume_acceptance.rs",
    },
    CliSource {
        label: "provider_specific/session_capture/mod.rs",
        path: "src/executor/provider_specific/session_capture/mod.rs",
    },
    CliSource {
        label: "provider_specific/session_capture/telemetry_scrub.rs",
        path: "src/executor/provider_specific/session_capture/telemetry_scrub.rs",
    },
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

fn cli_source(source: CliSource) -> String {
    std::fs::read_to_string(source.full_path())
        .unwrap_or_else(|err| panic!("{} source readable: {err}", source.label))
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
    if source_declares_no_roles(source) {
        assert_functionless_inventory(label, source);
        return;
    }
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

fn source_declares_no_roles(source: &str) -> bool {
    let roles = role_tokens(declared_roles_line(source));
    roles.len() == 1 && roles[0] == "none"
}

fn assert_functionless_inventory(label: &str, source: &str) {
    let has_function_definition = source.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("fn ") || trimmed.starts_with("pub") && trimmed.contains(" fn ")
    });
    assert!(
        source.contains("Functionless module inventory; no A1 function-role claim."),
        "{label} functionless inventory must say it has no A1 function-role claim"
    );
    assert!(
        !has_function_definition,
        "{label} declares Roles: none but contains a function definition"
    );
}

fn assert_source_path_inventory_includes(path: &str) {
    assert!(
        CLI_SOURCES.iter().any(|source| source.path == path),
        "C2 CLI source inventory must include {path}"
    );
}

#[test]
fn cli_root_and_submodules_have_declared_role_headers_before_imports() {
    let root = cli_root_source();
    assert_declared_role_header_before_imports("cli.rs", &root);
    for source_path in CLI_SOURCES {
        let source = cli_source(*source_path);
        assert_declared_role_header_before_imports(source_path.label, &source);
    }
}

#[test]
fn cli_root_and_submodule_declared_roles_use_known_vocabulary() {
    let root = cli_root_source();
    assert_declared_roles_use_known_vocabulary("cli.rs", &root);
    for source_path in CLI_SOURCES {
        let source = cli_source(*source_path);
        assert_declared_roles_use_known_vocabulary(source_path.label, &source);
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

fn adapter_component_lines(source: &str) -> Vec<&str> {
    source
        .lines()
        .filter(|line| {
            line.trim_start()
                .starts_with("//!   - component: crates/oulipoly-runtime/")
        })
        .collect()
}

#[test]
fn cli_submodules_have_adapter_declaration_translates_blocks() {
    for source_path in CLI_SOURCES {
        check_source_adapter_declarations(*source_path);
    }
}

fn check_source_adapter_declarations(source_path: CliSource) {
    let source = cli_source(source_path);
    if source_declares_no_roles(&source) {
        assert_functionless_inventory(source_path.label, &source);
        return;
    }
    assert!(
        source.contains("//! ## Adapter declarations"),
        "{} missing adapter declarations header",
        source_path.label
    );
    assert!(
        source.contains("//!     Translates:"),
        "{} missing Translates block",
        source_path.label
    );
    assert!(
        !translates_entries(&source).is_empty(),
        "{} Translates block must name at least one contract",
        source_path.label
    );
    let expected_component = format!("//!   - component: {}", source_path.component_path());
    let component_lines = adapter_component_lines(&source);
    assert!(
        !component_lines.is_empty(),
        "{} missing adapter component declaration",
        source_path.label
    );
    assert!(
        component_lines
            .iter()
            .all(|component| *component == expected_component),
        "{} adapter component declaration must match nested source path {}; got {component_lines:?}",
        source_path.label,
        source_path.component_path()
    );
}

#[test]
fn phase6_e4_source_inventory_names_every_split_leaf() {
    for source_path in E4_SPLIT_SOURCES {
        assert_source_path_inventory_includes(source_path.path);
    }

    for old_flat_source in [
        "src/executor/cli/input_flags.rs",
        "src/executor/cli/ipc.rs",
        "src/executor/cli/launch.rs",
        "src/executor/cli/supervision.rs",
    ] {
        assert!(
            !CLI_SOURCES
                .iter()
                .any(|source| source.path == old_flat_source),
            "C2 E4 inventory must use nested split leaves, not old flat carrier {old_flat_source}"
        );
    }
}

#[test]
fn phase6_e5_source_inventory_names_every_split_leaf_and_island() {
    for source_path in E5_SPLIT_SOURCES {
        assert_source_path_inventory_includes(source_path.path);
    }
}

#[test]
fn executor_coverage_spec_lists_required_cli_leaves() {
    let spec = std::fs::read_to_string(coverage_spec_path()).expect("executor spec readable");

    for source_path in [
        CliSource {
            label: "headless.rs",
            path: "src/executor/cli/headless.rs",
        },
        CliSource {
            label: "interactive.rs",
            path: "src/executor/cli/interactive.rs",
        },
        CliSource {
            label: "provider_execution.rs",
            path: "src/executor/cli/provider_execution.rs",
        },
        CliSource {
            label: "provider_lookup.rs",
            path: "src/executor/cli/provider_lookup.rs",
        },
        CliSource {
            label: "request.rs",
            path: "src/executor/cli/request.rs",
        },
        CliSource {
            label: "result.rs",
            path: "src/executor/cli/result.rs",
        },
        CliSource {
            label: "resume_execution.rs",
            path: "src/executor/cli/resume_execution.rs",
        },
    ]
    .into_iter()
    .chain(E4_SPLIT_SOURCES.iter().copied())
    .chain(E5_SPLIT_SOURCES.iter().copied())
    {
        let expected = format!("- `{}`", source_path.component_path());
        assert!(
            spec.contains(&expected),
            "planning/coverage/spec-executor.md must list {} for executor audit discovery",
            source_path.component_path()
        );
    }
}

#[test]
fn phase6_e4_split_sources_remain_provider_token_neutral() {
    for source_path in E4_SPLIT_SOURCES {
        let source = cli_source(*source_path).to_lowercase();
        for forbidden in ["claude", "codex"] {
            assert!(
                !source.contains(forbidden),
                "{} must remain provider-token neutral for E4 split: {forbidden}",
                source_path.label
            );
        }
    }
}
