//! ## Declared roles
//! accessor, mapper, parser, validator
//!
//! Function role map:
//! - `workspace_root`, `carried_regressions`, `target_path`, `step6c_command_log_path`: accessor
//! - `row_ids`, `step6c_command_log_section`: mapper
//! - `read_step6c_command_log`: parser
//! - `age154_*` tests: validator

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

struct CarriedRegression {
    row_id: &'static str,
    source: &'static str,
    command_or_node_id: &'static str,
    durable_target: &'static str,
}

const CARRIED_REGRESSIONS: &[CarriedRegression] = &[
    CarriedRegression {
        row_id: "AGE-154-CARRIED-01-age151-source-guard",
        source: "AGE-151",
        command_or_node_id: "cargo test -p oulipoly-agent-runner --test age151_source_guard",
        durable_target: "src-tauri/tests/age151_source_guard.rs",
    },
    CarriedRegression {
        row_id: "AGE-154-CARRIED-02-terminal-outcome-adapter-inline",
        source: "AGE-151 / AGE-153",
        command_or_node_id: "cargo test -p oulipoly-agent-runner terminal_outcome_adapter::tests::",
        durable_target: "src-tauri/src/terminal_outcome_adapter.rs",
    },
    CarriedRegression {
        row_id: "AGE-154-CARRIED-03-age153-terminal-signal-marker",
        source: "AGE-153",
        command_or_node_id: "cargo test -p oulipoly-agent-runner --test age153_terminal_signal_marker",
        durable_target: "src-tauri/tests/age153_terminal_signal_marker.rs",
    },
    CarriedRegression {
        row_id: "AGE-154-CARRIED-04-age153-result-envelope-compat",
        source: "AGE-153",
        command_or_node_id: "cargo test -p oulipoly-agent-runner --test age153_result_envelope_compat",
        durable_target: "src-tauri/tests/age153_result_envelope_compat.rs",
    },
    CarriedRegression {
        row_id: "AGE-154-CARRIED-05-age153-one-shot-terminal-signal",
        source: "AGE-153",
        command_or_node_id: "cargo test -p oulipoly-agent-runner --test age153_one_shot_terminal_signal",
        durable_target: "src-tauri/tests/age153_one_shot_terminal_signal.rs",
    },
    CarriedRegression {
        row_id: "AGE-154-CARRIED-06-age153-resume-terminal-signal",
        source: "AGE-153",
        command_or_node_id: "cargo test -p oulipoly-agent-runner --test age153_resume_terminal_signal",
        durable_target: "src-tauri/tests/age153_resume_terminal_signal.rs",
    },
    CarriedRegression {
        row_id: "AGE-154-CARRIED-07-age153-repl-terminal-signal",
        source: "AGE-153",
        command_or_node_id: "cargo test -p oulipoly-agent-runner --test age153_repl_terminal_signal",
        durable_target: "src-tauri/tests/age153_repl_terminal_signal.rs",
    },
    CarriedRegression {
        row_id: "AGE-154-CARRIED-08-age153-captured-child-supervision",
        source: "AGE-153",
        command_or_node_id: "cargo test -p oulipoly-agent-runner --test age153_captured_child_supervision",
        durable_target: "src-tauri/tests/age153_captured_child_supervision.rs",
    },
    CarriedRegression {
        row_id: "AGE-154-CARRIED-09-age153-balancer-signal-isolation",
        source: "AGE-153",
        command_or_node_id: "cargo test -p oulipoly-runtime --test age153_balancer_signal_isolation",
        durable_target: "crates/oulipoly-runtime/tests/age153_balancer_signal_isolation.rs",
    },
    CarriedRegression {
        row_id: "AGE-154-CARRIED-10-balancer-inline-regressions",
        source: "AGE-140 / AGE-153",
        command_or_node_id: "cargo test -p oulipoly-runtime balancer::tests::",
        durable_target: "crates/oulipoly-runtime/src/balancer/mod.rs",
    },
    CarriedRegression {
        row_id: "AGE-154-CARRIED-11-routing-matrix",
        source: "AGE-59 retained by AGE-140",
        command_or_node_id: "cargo test -p oulipoly-runtime --test routing_matrix",
        durable_target: "crates/oulipoly-runtime/tests/routing_matrix.rs",
    },
    CarriedRegression {
        row_id: "AGE-154-CARRIED-12-age35-routing-lifecycle",
        source: "AGE-35",
        command_or_node_id: "cargo test -p oulipoly-agent-runner --test age35_routing_lifecycle_characterization",
        durable_target: "src-tauri/tests/age35_routing_lifecycle_characterization.rs",
    },
    CarriedRegression {
        row_id: "AGE-154-CARRIED-13-age27-diagnostics-effective-provider",
        source: "AGE-27 / AGE-153",
        command_or_node_id: "cargo test -p oulipoly-agent-runner --test age27_diagnostics_effective_provider",
        durable_target: "src-tauri/tests/age27_diagnostics_effective_provider.rs",
    },
    CarriedRegression {
        row_id: "AGE-154-CARRIED-14-provider-termination-eval-observation",
        source: "AGE-91 W5 / AGE-143",
        command_or_node_id: "evals/agent-runner-provider-termination/eval.sh",
        durable_target: "evals/agent-runner-provider-termination/eval.sh",
    },
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri parent workspace root")
        .to_path_buf()
}

fn carried_regressions() -> &'static [CarriedRegression] {
    CARRIED_REGRESSIONS
}

fn target_path(entry: &CarriedRegression) -> PathBuf {
    workspace_root().join(entry.durable_target)
}

fn step6c_command_log_path() -> Option<PathBuf> {
    let relative_path = Path::new("planning")
        .join("age-154-compat-disposition")
        .join(".scratch")
        .join("phase6")
        .join("age-140-carried-regression-command-log.md");

    let mut cursor = Path::new(env!("CARGO_MANIFEST_DIR"));
    loop {
        let candidate = cursor.join(&relative_path);
        if candidate.exists() {
            return Some(candidate);
        }
        cursor = cursor.parent()?;
    }
}

fn read_step6c_command_log() -> Option<String> {
    let path = step6c_command_log_path()?;
    Some(fs::read_to_string(&path).unwrap_or_else(|err| {
        panic!(
            "failed to read Step 6c command log {}: {err}",
            path.display()
        )
    }))
}

fn step6c_command_log_section<'a>(command_log: &'a str, entry: &CarriedRegression) -> &'a str {
    let header = format!("## {}", entry.row_id);
    let start = command_log
        .find(&header)
        .unwrap_or_else(|| panic!("Step 6c command log missing row header {}", entry.row_id));
    let section = &command_log[start..];
    let end = section.find("\n## ").unwrap_or(section.len());
    &section[..end]
}

fn row_ids() -> BTreeSet<&'static str> {
    carried_regressions()
        .iter()
        .map(|entry| entry.row_id)
        .collect()
}

#[test]
fn age154_age140_carried_regression_mapping_documents_every_required_rerun() {
    assert_eq!(
        carried_regressions().len(),
        14,
        "proposal item 6 structured carried-regression mapping must stay complete"
    );
    assert_eq!(
        row_ids().len(),
        carried_regressions().len(),
        "Step 6b output-index row IDs must be unique"
    );
    for entry in carried_regressions() {
        assert!(
            entry.row_id.starts_with("AGE-154-CARRIED-"),
            "unexpected row id {} from {}",
            entry.row_id,
            entry.source
        );
        assert!(
            !entry.command_or_node_id.is_empty(),
            "{} must name the Step 6c rerun command",
            entry.row_id
        );
    }
}

#[test]
fn age154_age140_carried_regression_targets_exist_for_step6c_consumption() {
    for entry in carried_regressions() {
        let path = target_path(entry);
        assert!(
            path.exists(),
            "{} target must exist for Step 6c rerun/documented mapping: {}",
            entry.row_id,
            path.display()
        );
    }
}

#[test]
fn age154_age140_carried_regression_step6c_evidence_records_per_row_exit_code_zero() {
    // The Step 6c command log lives under `planning/age-154-compat-disposition/.scratch/phase6/`,
    // which is host-only scratch state and is not present on CI runners. Skip when absent so the
    // test remains green wherever the planning directory is unavailable; it still asserts evidence
    // contents on hosts where the file exists.
    let Some(command_log) = read_step6c_command_log() else {
        eprintln!(
            "skipping age154_age140_carried_regression_step6c_evidence_records_per_row_exit_code_zero: \
             planning/age-154-compat-disposition Step 6c command log not present on this host"
        );
        return;
    };

    for entry in carried_regressions() {
        let section = step6c_command_log_section(&command_log, entry);
        assert!(
            section.contains(entry.row_id),
            "Step 6c evidence section must preserve row id {}",
            entry.row_id
        );
        assert!(
            section.contains(&format!("Command: {}", entry.command_or_node_id)),
            "{} Step 6c evidence must preserve command_or_node_id `{}`",
            entry.row_id,
            entry.command_or_node_id
        );
        assert!(
            section.contains("Exit code: 0"),
            "{} Step 6c durable evidence must record the contract-named observable: command_or_node_id returned exit code 0 against HEAD",
            entry.row_id
        );
    }
}
