#![cfg(unix)]

use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig};
use oulipoly_runtime::executor;
use oulipoly_runtime::executor::cli::{self, EffectiveExecuteRequest};
use oulipoly_runtime::services::{
    ExecutorServiceOutput, ExecutorServicePort, ExecutorServiceRequest,
};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

struct FixtureScript {
    _dir: tempfile::TempDir,
    path: PathBuf,
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
            name: "model-provider".to_string(),
            command: script.path.to_string_lossy().into_owned(),
            args: Vec::new(),
            interactive_args: None,
            resume: None,
            session_capture: None,
            resume_acceptance: None,
            session_storage: None,
        }],
        inputs: Vec::new(),
    }
}

fn effective_provider(script: &FixtureScript) -> ProviderConfig {
    ProviderConfig {
        name: "effective-provider".to_string(),
        command: script.path.to_string_lossy().into_owned(),
        args: Vec::new(),
        interactive_args: None,
        resume: None,
        session_capture: None,
        resume_acceptance: None,
        session_storage: None,
    }
}

fn assert_execution_equivalent(
    service: &executor::ExecutionResult,
    direct: &executor::ExecutionResult,
) {
    assert_eq!(service.exit_code, direct.exit_code);
    assert_eq!(service.provider_index, direct.provider_index);
    assert_eq!(service.stdout, direct.stdout);
    assert_eq!(service.stderr, direct.stderr);
    assert_eq!(service.terminal_reason, direct.terminal_reason);
    assert_eq!(
        service.returned_artifacts.len(),
        direct.returned_artifacts.len()
    );
    assert_eq!(
        service.captured_child_invocations.len(),
        direct.captured_child_invocations.len()
    );
}

// Risk: R-A3 / proposal T10 - executor adapter routing must delegate to the
// same already-effective execution helper without dropping provider index,
// prompt, working directory, parent env, stdout/stderr, or result shape.
// Level: unit/component.
// Source: AGE-34 contract "Expected observable signals"; assumption A3.
#[test]
fn runtime_executor_service_effective_request_matches_direct_executor_call() {
    let model_script = fixture_script("printf 'wrong-model-provider\\n'");
    let working_dir = tempfile::tempdir().expect("working dir");
    let env_dump = tempfile::NamedTempFile::new().expect("env dump");
    let effective_script = fixture_script(&format!(
        r#"test "$PWD" = "{cwd}"
printf '%s' "${{OULIPOLY_PARENT_INVOCATION-}}" > "{env_dump}"
printf 'effective:%s\n' "$1"
printf 'stderr:%s\n' "$1" >&2"#,
        cwd = working_dir.path().display(),
        env_dump = env_dump.path().display()
    ));
    let model = model_for(&model_script);
    let provider = effective_provider(&effective_script);
    let parent_env = r#"{"source":"age34-test","id":"11111111-1111-1111-1111-111111111111"}"#;
    let extra_inputs: HashMap<String, Vec<String>> = HashMap::new();

    let direct = cli::execute_effective(EffectiveExecuteRequest {
        model: &model,
        provider: &provider,
        provider_index: 9,
        prompt_mode: PromptMode::Arg,
        prompt: "prompt-value",
        working_dir: Some(working_dir.path()),
        extra_inputs: &extra_inputs,
        parent_invocation_env: Some(parent_env),
    })
    .expect("direct execute");

    let service: &dyn ExecutorServicePort = &executor::RuntimeExecutorService;
    let ExecutorServiceOutput { result } = service
        .execute(ExecutorServiceRequest::Effective {
            model: model.clone(),
            provider: provider.clone(),
            provider_index: 9,
            prompt_mode: PromptMode::Arg,
            prompt: "prompt-value".to_string(),
            working_dir: Some(working_dir.path().to_path_buf()),
            extra_inputs,
            parent_invocation_env: Some(parent_env.to_string()),
        })
        .expect("service execute");

    assert_execution_equivalent(&result, &direct);
    assert_eq!(
        std::fs::read_to_string(env_dump.path()).unwrap(),
        parent_env
    );
}

// Risk: R-A3 / proposal T10 - raw facade-style executor routing must preserve
// provider-index selection and input/default handling by delegating to cli::execute.
// Level: unit/component.
// Source: AGE-34 contract "Schemas" for facade-style raw execute.
#[test]
fn runtime_executor_service_raw_request_matches_direct_executor_call() {
    let script = fixture_script("printf 'provider:%s\\n' \"$1\"");
    let model = model_for(&script);
    let extra_inputs: HashMap<String, Vec<String>> = HashMap::new();

    let direct = cli::execute(&model, 0, "raw-prompt", None, &extra_inputs, None)
        .expect("direct raw execute");

    let service: &dyn ExecutorServicePort = &executor::RuntimeExecutorService;
    let ExecutorServiceOutput { result } = service
        .execute(ExecutorServiceRequest::Facade {
            model: model.clone(),
            provider_index: 0,
            prompt: "raw-prompt".to_string(),
            working_dir: None,
            extra_inputs,
            parent_invocation_env: None,
        })
        .expect("service raw execute");

    assert_execution_equivalent(&result, &direct);
}
