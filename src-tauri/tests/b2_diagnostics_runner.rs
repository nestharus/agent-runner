#![cfg(unix)]

#[path = "fixtures/b2_process_runner.rs"]
mod b2_process_runner;

use agent_runner_lib::config::{ModelConfig, PromptMode, ProviderConfig};
use agent_runner_lib::diagnostics;
use agent_runner_lib::process::StdinSpec;
use b2_process_runner::FakeProcessRunner;
use std::collections::HashMap;

fn diagnostics_model(prompt_mode: PromptMode) -> ModelConfig {
    ModelConfig {
        name: "diagnostics-fixture".to_string(),
        prompt_mode,
        providers: vec![ProviderConfig::new(
            "diagnostic-cli",
            vec!["--json".to_string()],
        )],
        inputs: Vec::new(),
    }
}

/// Risk: T6 - diagnostics may keep calling the concrete executor wrapper after
/// introducing `ProcessRunner`, making LLM classification untestable and
/// preserving the hidden direct spawn path.
/// Level: component.
/// Source: B2 contract sections 3 and 9 Q4.
/// Observable: `diagnose_error_with_runner` sends the classifier prompt through
/// the injected runner, and executor spawn errors still propagate instead of
/// silently falling back to heuristics.
#[test]
fn diagnose_error_with_runner_uses_runner_and_propagates_executor_errors() {
    let runner = FakeProcessRunner::with_responses(vec![Err("spawn denied".to_string())]);
    let model = diagnostics_model(PromptMode::Stdin);
    let models = HashMap::new();

    let err = diagnostics::diagnose_error_with_runner(
        &runner,
        "quota exceeded on provider",
        17,
        &model,
        &models,
        None,
    )
    .unwrap_err();

    assert!(err.contains("spawn denied"));
    let call = runner.single_call();
    assert_eq!(call.program, "diagnostic-cli");
    assert_eq!(call.args, ["--json"]);
    let StdinSpec::Bytes(prompt) = call.stdin else {
        panic!("diagnostics prompt should be sent through stdin");
    };
    let prompt = String::from_utf8(prompt).unwrap();
    assert!(prompt.contains("quota exceeded on provider"));
    assert!(prompt.contains("17"));
}
