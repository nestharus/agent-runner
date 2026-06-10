#![cfg(unix)]

//! ## Declared roles
//!
//! `orchestration`, `accessor`, `mapper`, `formatter`, `predicate`, `validator`
//!
//! This integration-test file exercises the runtime executor return-channel contract. Test bodies
//! orchestrate named fixture, execution, and validation helpers; helpers keep fixture mapping,
//! receipt formatting, environment-lock access, filesystem predicates, and result validation in
//! separately named single-role functions.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/tests/executor_return_channel.rs
//!     role: adapter
//!     Translates:
//!       - runtime executor entry points and result surfaces
//!       - model/provider configuration fixture contract
//!       - returned artifact receipt contract
//!       - process environment and filesystem fixture contract
//!       - invocation identity and receipt serialization contract
//! ```
//!
//! The external references in this file are subordinate to those five test-adapter contracts: the
//! executor entry points under test, the minimal model/provider configuration they require, the
//! return-receipt payload they consume, the Unix process/filesystem fixture used to observe channel
//! behavior, and the invocation/serialization values needed to bind a receipt to an invocation.
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-runtime/tests/executor_return_channel.rs
//!     role: intrinsic-surface
//!     Domain: executor_return_channel_integration_contract
//!     Owns:
//!       - env_lock_serialization_for_executor_invocations
//!       - fixture_provider_scripts
//!       - model_provider_fixture_config
//!       - returned_artifact_receipts
//!       - parent_env_binding
//!       - stale_return_channel_scrubbing
//!       - resume_return_artifact_propagation
//!       - interactive_repl_return_channel_exclusion
//!       - stdout_preservation
//!       - channel_sidecar_deletion
//! ```
//!
//! This file owns one coherent integration-test domain: executor return-channel behavior across root,
//! resume, and interactive paths. All helper-level references support that domain-owned fixture and
//! validation surface.

use oulipoly_agent_messenger::{ReturnedArtifactRef, ReturnedArtifactSource, StoreAddress};
use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig, ResumeKind, ResumeStrategy};
use oulipoly_runtime::executor::cli::{
    InteractiveExecutionResult, ResumePayload, execute_interactive_with_result, execute_resume,
};
use oulipoly_runtime::executor::{self, ExecutionResult};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use uuid::Uuid;

struct FixtureScript {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

struct ParentEnvFixture {
    _dir: tempfile::TempDir,
    _script: FixtureScript,
    model: ModelConfig,
    observed_channel: PathBuf,
}

struct StaleChannelFixture {
    _dir: tempfile::TempDir,
    _script: FixtureScript,
    model: ModelConfig,
    stale: PathBuf,
    observed: PathBuf,
}

struct ResumeFixture {
    _script: FixtureScript,
    model: ModelConfig,
}

struct InteractiveFixture {
    _dir: tempfile::TempDir,
    _script: FixtureScript,
    model: ModelConfig,
    stale: PathBuf,
    observed: PathBuf,
}

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn locked_env() -> MutexGuard<'static, ()> {
    env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn fixture_script(body: &str) -> FixtureScript {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("provider.sh");
    std::fs::write(
        &path,
        format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}\n"),
    )
    .expect("write provider");
    let mut perms = std::fs::metadata(&path).expect("metadata").permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).expect("chmod provider");
    FixtureScript { _dir: dir, path }
}

fn model_for(script: &FixtureScript) -> ModelConfig {
    ModelConfig {
        name: "fixture-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![ProviderConfig {
            name: "fixture-provider".to_string(),
            command: script.path.to_string_lossy().into_owned(),
            args: Vec::new(),
            interactive_args: Some(vec!["interactive".to_string()]),
            resume: None,
            session_capture: None,
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: None,
            tool_restrictions: None,
            invocation_mode: Default::default(),
        }],
        inputs: Vec::new(),
        provider: None,
    }
}

fn receipt_json(invocation_uuid: Uuid, name: &str, version: u64) -> String {
    serde_json::to_string(&returned_artifact_ref(invocation_uuid, name, version))
        .expect("receipt json")
}

fn returned_artifact_ref(invocation_uuid: Uuid, name: &str, version: u64) -> ReturnedArtifactRef {
    ReturnedArtifactRef {
        version_id: format!("store://return/{invocation_uuid}/{name}/{version}"),
        name: name.to_string(),
        store_address: StoreAddress {
            workflow_run_id: format!("return:{invocation_uuid}"),
            artifact_name: name.to_string(),
            version,
        },
        sha256: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        content_len: 3,
        format_hint: Some("application/octet-stream".to_string()),
        verdict_line: Some("APPROVED: ready".to_string()),
        source: ReturnedArtifactSource::InlineBytes,
        producer_invocation_uuid: invocation_uuid,
        returned_at: chrono::Utc::now(),
    }
}

fn shell_escaped_receipt(invocation_uuid: Uuid, name: &str, version: u64) -> String {
    receipt_json(invocation_uuid, name, version).replace('\'', r#"'\''"#)
}

fn parent_env(invocation_uuid: Uuid) -> String {
    format!(r#"{{"source":"test","id":"{invocation_uuid}"}}"#)
}

fn parent_env_script_body(observed: &Path, receipt: &str) -> String {
    format!(
        r#"test -n "${{OULIPOLY_RETURN_CHANNEL:-}}"
test -f "$OULIPOLY_RETURN_CHANNEL"
test ! -s "$OULIPOLY_RETURN_CHANNEL"
printf '%s' "$OULIPOLY_RETURN_CHANNEL" > "{observed}"
printf '%s\n' '{receipt}' >> "$OULIPOLY_RETURN_CHANNEL"
printf 'raw\000\377Z'"#,
        observed = observed.display()
    )
}

fn stale_channel_script_body(observed: &Path) -> String {
    format!(
        r#"printf '%s' "${{OULIPOLY_RETURN_CHANNEL-}}" > "{observed}"
printf 'ok'"#,
        observed = observed.display()
    )
}

fn resume_script_body(receipt: &str) -> String {
    format!(
        r#"printf '%s\n' '{receipt}' >> "${{OULIPOLY_RETURN_CHANNEL:?missing}}"
printf 'resume stdout'"#
    )
}

fn interactive_script_body(observed: &Path) -> String {
    format!(
        r#"printf '%s' "${{OULIPOLY_RETURN_CHANNEL-}}" > "{observed}""#,
        observed = observed.display()
    )
}

fn parent_env_fixture(invocation: Uuid) -> ParentEnvFixture {
    let receipt = shell_escaped_receipt(invocation, "blob.bin", 1);
    let dir = tempfile::tempdir().expect("tempdir");
    let observed_channel = dir.path().join("observed-channel.txt");
    let script = fixture_script(&parent_env_script_body(&observed_channel, &receipt));
    let model = model_for(&script);
    ParentEnvFixture {
        _dir: dir,
        _script: script,
        model,
        observed_channel,
    }
}

fn stale_channel_fixture() -> StaleChannelFixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let stale = dir.path().join("stale.jsonl");
    let observed = dir.path().join("observed.txt");
    let script = fixture_script(&stale_channel_script_body(&observed));
    let model = model_for(&script);
    StaleChannelFixture {
        _dir: dir,
        _script: script,
        model,
        stale,
        observed,
    }
}

fn resume_fixture(invocation: Uuid) -> ResumeFixture {
    let receipt = shell_escaped_receipt(invocation, "resume.md", 1);
    let script = fixture_script(&resume_script_body(&receipt));
    let model = model_for(&script);
    ResumeFixture {
        _script: script,
        model,
    }
}

fn resume_strategy() -> ResumeStrategy {
    ResumeStrategy {
        kind: ResumeKind::Flag,
        flag: Some("--resume".to_string()),
        subcommand: None,
    }
}

fn interactive_fixture() -> InteractiveFixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let stale = dir.path().join("stale.jsonl");
    let observed = dir.path().join("interactive-observed.txt");
    let script = fixture_script(&interactive_script_body(&observed));
    let model = model_for(&script);
    InteractiveFixture {
        _dir: dir,
        _script: script,
        model,
        stale,
        observed,
    }
}

fn path_exists(path: &Path) -> bool {
    path.exists()
}

fn assert_parent_env_channel_result(result: &ExecutionResult, observed_channel: PathBuf) {
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout, b"raw\0\xffZ".to_vec());
    assert_eq!(result.returned_artifacts.len(), 1);
    assert_eq!(result.returned_artifacts[0].name, "blob.bin");
    let channel_path = PathBuf::from(std::fs::read_to_string(observed_channel).expect("observed"));
    assert!(
        !path_exists(&channel_path),
        "executor should delete return channel before returning"
    );
}

fn assert_stale_channel_result(result: &ExecutionResult, observed: PathBuf, stale: &Path) {
    assert_eq!(result.exit_code, 0);
    assert_eq!(std::fs::read_to_string(observed).expect("observed"), "");
    assert!(result.returned_artifacts.is_empty());
    assert!(
        !path_exists(stale),
        "no channel file should be created at stale path"
    );
}

fn assert_resume_result(result: &ExecutionResult) {
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout, b"resume stdout".to_vec());
    assert_eq!(result.returned_artifacts.len(), 1);
    assert_eq!(result.returned_artifacts[0].name, "resume.md");
}

fn assert_interactive_result(result: &InteractiveExecutionResult, observed: PathBuf, stale: &Path) {
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.terminal_reason, None);
    assert_eq!(std::fs::read_to_string(observed).expect("observed"), "");
    assert!(
        !path_exists(stale),
        "interactive path must not create a stale channel file"
    );
}

// proposal § Test-Intent Track row: executor channel discipline with parent env
// contract § Runtime executor binding contract points 1, 3, 4, 5, 6, 8
// named risk: Runtime Executor HIGH - returned artifacts could be parsed from stdout or lost before RawResult/ExecutionResult propagation
// selected level: runtime_integration
#[test]
fn executor_with_parent_env_injects_channel_reads_receipts_deletes_sidecar_and_preserves_stdout() {
    let _guard = locked_env();
    let invocation = Uuid::new_v4();
    let fixture = parent_env_fixture(invocation);

    let result = executor::execute_with_inputs_and_env(
        &fixture.model,
        0,
        "prompt",
        None,
        &HashMap::new(),
        Some(&parent_env(invocation)),
    )
    .expect("execute");

    assert_parent_env_channel_result(&result, fixture.observed_channel);
}

// proposal § Test-Intent Track row: executor without parent env removes stale return channel
// contract § Runtime executor binding contract point 2
// named risk: Runtime Executor HIGH - stale inherited OULIPOLY_RETURN_CHANNEL could bind root executions to another invocation
// selected level: runtime_integration
#[test]
fn executor_without_parent_env_removes_stale_channel_and_returns_empty_artifacts() {
    let _guard = locked_env();
    let fixture = stale_channel_fixture();

    unsafe {
        std::env::set_var("OULIPOLY_RETURN_CHANNEL", &fixture.stale);
    }
    let result = executor::execute(&fixture.model, 0, "prompt", None).expect("execute");
    unsafe {
        std::env::remove_var("OULIPOLY_RETURN_CHANNEL");
    }

    assert_stale_channel_result(&result, fixture.observed, &fixture.stale);
}

// proposal § Test-Intent Track row: ExecutionResult populated for headless resume
// contract § Runtime executor binding contract point 5
// named risk: Runtime Executor HIGH - resume constructor could drop RawResult.returned_artifacts
// selected level: runtime_integration
#[test]
fn execute_resume_propagates_returned_artifacts_from_raw_result() {
    let _guard = locked_env();
    let invocation = Uuid::new_v4();
    let fixture = resume_fixture(invocation);
    let strategy = resume_strategy();

    let result = execute_resume(
        &fixture.model.providers[0],
        0,
        PromptMode::Arg,
        "prompt",
        None,
        Some(&parent_env(invocation)),
        ResumePayload {
            session_id: "5169694d-de0f-40d1-890c-6e28e55bab27",
            strategy: &strategy,
        },
    )
    .expect("resume execute");

    assert_resume_result(&result);
}

// proposal § Test-Intent Track row: REPL does not bind returns
// contract § Runtime executor binding contract point 7
// named risk: Runtime Executor HIGH - interactive REPL could inherit or create a return channel despite v1 exclusion
// selected level: runtime_integration
#[test]
fn interactive_repl_removes_stale_channel_and_has_unchanged_result_shape() {
    let _guard = locked_env();
    let fixture = interactive_fixture();

    unsafe {
        std::env::set_var("OULIPOLY_RETURN_CHANNEL", &fixture.stale);
    }
    let result = execute_interactive_with_result(&fixture.model.providers[0], None, None, None)
        .expect("interactive");
    unsafe {
        std::env::remove_var("OULIPOLY_RETURN_CHANNEL");
    }

    assert_interactive_result(&result, fixture.observed, &fixture.stale);
}
