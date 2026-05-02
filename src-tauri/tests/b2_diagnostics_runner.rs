#![cfg(unix)]

mod fixtures;

use agent_runner_lib::config::PromptMode;
use agent_runner_lib::diagnostics::{
    ErrorCategory, classify_exhaustion, diagnose_error_with_runner,
};
use fixtures::b2_process_runner::*;
use std::collections::HashMap;

/// Risk: T6 (diagnostics LLM path invokes injected executor runner)
/// Source: proposal §8 T6; contract §3 diagnostics::diagnose_error_with_runner
/// Level: unit
/// Fixture source: src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn diagnostics_with_runner_parses_successful_model_output() {
    let runner = FakeProcessRunner::new();
    runner.push_stdout(b"rate_limit\nThe provider returned HTTP 429");
    let diagnostics_model = model_with_provider(
        "diagnostics-model",
        PromptMode::Arg,
        provider("diagnostics-cli", &[]),
    );
    let models = HashMap::from([(diagnostics_model.name.clone(), diagnostics_model.clone())]);

    let diagnosis = diagnose_error_with_runner(
        &runner,
        "original stderr",
        1,
        &diagnostics_model,
        &models,
        None,
    )
    .unwrap();

    assert_eq!(diagnosis.category, ErrorCategory::RateLimit);
    assert!(diagnosis.summary.contains("429"));
    let call = runner.only_call();
    assert_eq!(call.program, "diagnostics-cli");
    assert!(call.args.iter().any(|arg| arg.contains("original stderr")));
}

/// Risk: T6 (empty diagnostics model output falls back to heuristic)
/// Source: proposal §8 T6; contract §3 diagnostics heuristic path
/// Level: unit
/// Fixture source: src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn diagnostics_with_runner_empty_model_output_uses_heuristic_fallback() {
    let runner = FakeProcessRunner::new();
    runner.push_stdout(b"");
    let diagnostics_model = model_with_provider(
        "diagnostics-model",
        PromptMode::Arg,
        provider("diagnostics-cli", &[]),
    );
    let models = HashMap::new();

    let diagnosis = diagnose_error_with_runner(
        &runner,
        "Error: Unauthorized - token expired",
        1,
        &diagnostics_model,
        &models,
        None,
    )
    .unwrap();

    assert_eq!(diagnosis.category, ErrorCategory::AuthExpired);
    assert_eq!(runner.calls().len(), 1);
}

/// Risk: T6 (heuristic exhaustion classifier remains pure)
/// Source: proposal §8 T6; contract §3 diagnostics pure functions
/// Level: unit
/// Fixture source: src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn diagnostics_classify_exhaustion_remains_independent_of_runner() {
    assert!(classify_exhaustion(
        "quota exceeded: billing hard limit reached for this organization"
    ));
    assert!(!classify_exhaustion("syntax error: unexpected token"));
}
