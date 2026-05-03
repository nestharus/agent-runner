#![cfg(unix)]

#[path = "../../../src-tauri/tests/fixtures/b2_process_runner.rs"]
mod b2_process_runner;

mod fixtures {
    pub mod d_agent_fixtures;
}

use agent_runner_agent_bin::{run, AgentRunOptions};
use b2_process_runner::FakeProcessRunner;
use fixtures::d_agent_fixtures::AgentFixture;
use std::process::ExitCode;
use std::sync::Arc;

/// Risk: D-T1 (agent binary launches REPL with default model)
/// Source: D-agent-binary contract §7
/// Level: component
/// Fixture source: tests/fixtures/d_agent_fixtures.rs
#[test]
fn agent_default_model_dispatches_run_repl_with_services() {
    let fixture = AgentFixture::new();
    fixture.write_config(r#"default_model = "fixture-model""#);
    fixture.write_interactive_model("fixture-model", "fixture-provider", "fixture-cli");
    let runner = Arc::new(FakeProcessRunner::new());
    runner.push_interactive_response(Ok(0));
    let mut stderr = Vec::new();

    let exit = run(
        AgentRunOptions {
            argv: vec!["agent".to_string()],
            config_path: fixture.config_path(),
            models_dir_override: Some(fixture.models_dir().to_path_buf()),
        },
        fixture.services(runner.clone()),
        &mut stderr,
    );

    assert_eq!(exit, ExitCode::SUCCESS);
    assert!(runner.calls().is_empty());
    let call = runner.single_interactive_call();
    assert_eq!(call.program, "fixture-cli");
    assert_eq!(call.args, ["launch"]);
    assert_eq!(call.env.len(), 1, "agent should not add non-REPL env");
    assert!(call.env.contains_key("OULIPOLY_PARENT_INVOCATION"));
    assert!(
        String::from_utf8(stderr)
            .unwrap()
            .contains("OULIPOLY_INVOCATION="),
        "agent should preserve REPL invocation emission"
    );
}

/// Risk: D-T3 (missing default_model fails clearly with no spawn)
/// Source: D-agent-binary contract §7
/// Level: unit
/// Fixture source: tests/fixtures/d_agent_fixtures.rs
#[test]
fn agent_missing_default_model_returns_clear_error_without_spawn() {
    let fixture = AgentFixture::new();
    fixture.write_config(r#"diagnostics_model = "fixture-diagnostics""#);
    let runner = Arc::new(FakeProcessRunner::new());
    let mut stderr = Vec::new();

    let exit = run(
        AgentRunOptions {
            argv: vec!["agent".to_string()],
            config_path: fixture.config_path(),
            models_dir_override: Some(fixture.models_dir().to_path_buf()),
        },
        fixture.services(runner.clone()),
        &mut stderr,
    );

    assert_eq!(exit, ExitCode::from(2));
    assert!(runner.calls().is_empty());
    assert!(runner.interactive_calls().is_empty());
    let stderr = String::from_utf8(stderr).unwrap();
    assert!(stderr.contains("default_model"), "{stderr}");
}

/// Risk: D-T4 (missing config.toml fails clearly with no spawn)
/// Source: D-agent-binary contract §7
/// Level: unit
/// Fixture source: tests/fixtures/d_agent_fixtures.rs
#[test]
fn agent_missing_config_file_returns_clear_error_without_spawn() {
    let fixture = AgentFixture::new();
    let runner = Arc::new(FakeProcessRunner::new());
    let mut stderr = Vec::new();

    let exit = run(
        AgentRunOptions {
            argv: vec!["agent".to_string()],
            config_path: fixture.config_path(),
            models_dir_override: Some(fixture.models_dir().to_path_buf()),
        },
        fixture.services(runner.clone()),
        &mut stderr,
    );

    assert_eq!(exit, ExitCode::from(2));
    assert!(runner.calls().is_empty());
    assert!(runner.interactive_calls().is_empty());
    let stderr = String::from_utf8(stderr).unwrap();
    assert!(stderr.contains("config.toml"), "{stderr}");
    assert!(stderr.contains("default_model"), "{stderr}");
}
