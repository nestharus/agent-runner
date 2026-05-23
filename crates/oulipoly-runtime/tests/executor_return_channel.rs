#![cfg(unix)]

use oulipoly_agent_messenger::{ReturnedArtifactRef, ReturnedArtifactSource, StoreAddress};
use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig, ResumeKind, ResumeStrategy};
use oulipoly_runtime::executor;
use oulipoly_runtime::executor::cli::{
    ResumePayload, execute_interactive_with_result, execute_resume,
};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use uuid::Uuid;

struct FixtureScript {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
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
    let reference = ReturnedArtifactRef {
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
    };
    serde_json::to_string(&reference).expect("receipt json")
}

fn parent_env(invocation_uuid: Uuid) -> String {
    format!(r#"{{"source":"test","id":"{invocation_uuid}"}}"#)
}

// proposal § Test-Intent Track row: executor channel discipline with parent env
// contract § Runtime executor binding contract points 1, 3, 4, 5, 6, 8
// named risk: Runtime Executor HIGH - returned artifacts could be parsed from stdout or lost before RawResult/ExecutionResult propagation
// selected level: runtime_integration
#[test]
fn executor_with_parent_env_injects_channel_reads_receipts_deletes_sidecar_and_preserves_stdout() {
    let invocation = Uuid::new_v4();
    let receipt = receipt_json(invocation, "blob.bin", 1).replace('\'', r#"'\''"#);
    let dir = tempfile::tempdir().expect("tempdir");
    let observed_channel = dir.path().join("observed-channel.txt");
    let script = fixture_script(&format!(
        r#"test -n "${{OULIPOLY_RETURN_CHANNEL:-}}"
test -f "$OULIPOLY_RETURN_CHANNEL"
test ! -s "$OULIPOLY_RETURN_CHANNEL"
printf '%s' "$OULIPOLY_RETURN_CHANNEL" > "{observed}"
printf '%s\n' '{receipt}' >> "$OULIPOLY_RETURN_CHANNEL"
printf 'raw\000\377Z'"#,
        observed = observed_channel.display()
    ));
    let model = model_for(&script);

    let result = executor::execute_with_inputs_and_env(
        &model,
        0,
        "prompt",
        None,
        &HashMap::new(),
        Some(&parent_env(invocation)),
    )
    .expect("execute");

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout, b"raw\0\xffZ".to_vec());
    assert_eq!(result.returned_artifacts.len(), 1);
    assert_eq!(result.returned_artifacts[0].name, "blob.bin");
    let channel_path = PathBuf::from(std::fs::read_to_string(observed_channel).expect("observed"));
    assert!(
        !channel_path.exists(),
        "executor should delete return channel before returning"
    );
}

// proposal § Test-Intent Track row: executor without parent env removes stale return channel
// contract § Runtime executor binding contract point 2
// named risk: Runtime Executor HIGH - stale inherited OULIPOLY_RETURN_CHANNEL could bind root executions to another invocation
// selected level: runtime_integration
#[test]
fn executor_without_parent_env_removes_stale_channel_and_returns_empty_artifacts() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().expect("tempdir");
    let stale = dir.path().join("stale.jsonl");
    let observed = dir.path().join("observed.txt");
    let script = fixture_script(&format!(
        r#"printf '%s' "${{OULIPOLY_RETURN_CHANNEL-}}" > "{observed}"
printf 'ok'"#,
        observed = observed.display()
    ));
    let model = model_for(&script);

    unsafe {
        std::env::set_var("OULIPOLY_RETURN_CHANNEL", &stale);
    }
    let result = executor::execute(&model, 0, "prompt", None).expect("execute");
    unsafe {
        std::env::remove_var("OULIPOLY_RETURN_CHANNEL");
    }

    assert_eq!(result.exit_code, 0);
    assert_eq!(std::fs::read_to_string(observed).expect("observed"), "");
    assert!(result.returned_artifacts.is_empty());
    assert!(
        !stale.exists(),
        "no channel file should be created at stale path"
    );
}

// proposal § Test-Intent Track row: ExecutionResult populated for headless resume
// contract § Runtime executor binding contract point 5
// named risk: Runtime Executor HIGH - resume constructor could drop RawResult.returned_artifacts
// selected level: runtime_integration
#[test]
fn execute_resume_propagates_returned_artifacts_from_raw_result() {
    let invocation = Uuid::new_v4();
    let receipt = receipt_json(invocation, "resume.md", 1).replace('\'', r#"'\''"#);
    let script = fixture_script(&format!(
        r#"printf '%s\n' '{receipt}' >> "${{OULIPOLY_RETURN_CHANNEL:?missing}}"
printf 'resume stdout'"#
    ));
    let model = model_for(&script);
    let strategy = ResumeStrategy {
        kind: ResumeKind::Flag,
        flag: Some("--resume".to_string()),
        subcommand: None,
    };

    let result = execute_resume(
        &model.providers[0],
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

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout, b"resume stdout".to_vec());
    assert_eq!(result.returned_artifacts.len(), 1);
    assert_eq!(result.returned_artifacts[0].name, "resume.md");
}

// proposal § Test-Intent Track row: REPL does not bind returns
// contract § Runtime executor binding contract point 7
// named risk: Runtime Executor HIGH - interactive REPL could inherit or create a return channel despite v1 exclusion
// selected level: runtime_integration
#[test]
fn interactive_repl_removes_stale_channel_and_has_unchanged_result_shape() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().expect("tempdir");
    let stale = dir.path().join("stale.jsonl");
    let observed = dir.path().join("interactive-observed.txt");
    let script = fixture_script(&format!(
        r#"printf '%s' "${{OULIPOLY_RETURN_CHANNEL-}}" > "{observed}""#,
        observed = observed.display()
    ));
    let model = model_for(&script);

    unsafe {
        std::env::set_var("OULIPOLY_RETURN_CHANNEL", &stale);
    }
    let result = execute_interactive_with_result(&model.providers[0], None, None, None)
        .expect("interactive");
    unsafe {
        std::env::remove_var("OULIPOLY_RETURN_CHANNEL");
    }

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.terminal_reason, None);
    assert_eq!(std::fs::read_to_string(observed).expect("observed"), "");
    assert!(
        !stale.exists(),
        "interactive path must not create a stale channel file"
    );
}
