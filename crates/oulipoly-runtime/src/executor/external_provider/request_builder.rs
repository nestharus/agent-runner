//! Role: mapper.

use super::context::ExternalProviderDispatchContext;
use crate::executor::cli::{resolve_input_flags, shell_split};
use oulipoly_config::PromptMode;
use oulipoly_provider::generated::{
    BytePayload, CONTRACT_VERSION, HostContext, JsonObject, LaunchParams, LaunchRequest,
    PolicyEvaluateParams, PolicyEvaluateRequest, ProviderModelRequest,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub(crate) struct LaunchCandidate {
    pub(crate) argv: Vec<String>,
    pub(crate) env: BTreeMap<String, String>,
    pub(crate) stdin: Option<String>,
    pub(crate) prompt: String,
    pub(crate) prompt_mode: PromptMode,
    pub(crate) working_directory: String,
}

pub(crate) fn build_launch_candidate(
    context: &ExternalProviderDispatchContext,
) -> Result<LaunchCandidate, String> {
    let mut env = std::env::vars().collect::<BTreeMap<_, _>>();
    if let Some(parent) = &context.parent_invocation_env {
        env.insert("OULIPOLY_PARENT_INVOCATION".to_string(), parent.clone());
    }
    let input_args = resolve_input_flags(&context.model, &context.extra_inputs)?;

    Ok(LaunchCandidate {
        argv: provider_argv(context, &input_args),
        env,
        stdin: launch_stdin(context),
        prompt: context.prompt.clone(),
        prompt_mode: context.prompt_mode,
        working_directory: working_directory(context),
    })
}

pub(crate) fn build_policy_request(
    context: &ExternalProviderDispatchContext,
    candidate: &LaunchCandidate,
) -> Result<Value, serde_json::Error> {
    let provider_args = model_provider_args(context);
    serde_json::to_value(PolicyEvaluateRequest {
        contract: CONTRACT_VERSION.to_string(),
        request_id: request_id("policy"),
        provider_instance_id: Some(context.provider.name.clone()),
        host: host_context(context, &candidate.working_directory),
        params: PolicyEvaluateParams {
            settings_id: context.settings_id.clone(),
            mode: mode(context),
            model: provider_model_request(context, &candidate.prompt, &provider_args),
            launch: policy_launch_object(context, candidate, &provider_args),
        },
    })
}

pub(crate) fn build_launch_request(
    context: &ExternalProviderDispatchContext,
    candidate: &LaunchCandidate,
) -> Result<Value, serde_json::Error> {
    let env = if candidate.env.is_empty() {
        None
    } else {
        Some(candidate.env.clone())
    };
    let stdin = candidate.stdin.as_ref().map(|stdin| BytePayload {
        encoding: "utf8".to_string(),
        data: stdin.clone(),
    });
    serde_json::to_value(LaunchRequest {
        contract: CONTRACT_VERSION.to_string(),
        request_id: request_id("launch"),
        provider_instance_id: Some(context.provider.name.clone()),
        host: host_context(context, &candidate.working_directory),
        params: LaunchParams {
            settings_id: context.settings_id.clone(),
            mode: mode(context),
            model: provider_model_request(
                context,
                &candidate.prompt,
                &model_provider_args(context),
            ),
            argv: candidate.argv.clone(),
            working_directory: candidate.working_directory.clone(),
            env,
            stdin,
            session: launch_session(context),
        },
    })
}

fn provider_argv(context: &ExternalProviderDispatchContext, input_args: &[String]) -> Vec<String> {
    let mut argv = shell_split(&context.provider.command);
    if argv.is_empty() {
        argv.push(context.provider.command.clone());
    }
    argv.extend(context.provider.args.clone());
    argv.extend(input_args.iter().cloned());
    if matches!(context.prompt_mode, PromptMode::Arg) {
        argv.push(context.prompt.clone());
    }
    argv
}

fn launch_stdin(context: &ExternalProviderDispatchContext) -> Option<String> {
    match context.prompt_mode {
        PromptMode::Stdin => Some(context.prompt.clone()),
        PromptMode::Arg => None,
    }
}

fn host_context(
    _context: &ExternalProviderDispatchContext,
    working_directory: &str,
) -> HostContext {
    HostContext {
        app: "oulipoly-agent-runner".to_string(),
        app_version: None,
        platform: Some(std::env::consts::OS.to_string()),
        working_directory: Some(working_directory.to_string()),
        config_root: None,
        data_root: None,
        env: BTreeMap::new(),
        deadline_unix_ms: None,
    }
}

fn provider_model_request(
    context: &ExternalProviderDispatchContext,
    prompt: &str,
    provider_args: &[String],
) -> ProviderModelRequest {
    ProviderModelRequest {
        name: context.model.name.clone(),
        provider_args: provider_args.to_vec(),
        inputs: json!({
            "prompt": prompt,
            "named": context.extra_inputs.clone(),
        }),
    }
}

fn policy_launch_object(
    context: &ExternalProviderDispatchContext,
    candidate: &LaunchCandidate,
    model_provider_args: &[String],
) -> JsonObject {
    let mut object = JsonObject::new();
    object.insert(
        "command".to_string(),
        Value::String(context.provider.command.clone()),
    );
    object.insert(
        "args".to_string(),
        json!(base_provider_args(context, model_provider_args)),
    );
    object.insert("prompt_mode".to_string(), Value::String(mode(context)));
    object.insert(
        "invocation_mode".to_string(),
        json!(context.provider.invocation_mode),
    );
    if let Some(system_prompt) = &context.provider.system_prompt_override {
        object.insert(
            "system_prompt_override".to_string(),
            Value::String(system_prompt.clone()),
        );
    }
    if let Some(tool_restrictions) = &context.provider.tool_restrictions {
        object.insert("tool_restrictions".to_string(), json!(tool_restrictions));
    }
    object.insert(
        "working_directory".to_string(),
        Value::String(candidate.working_directory.clone()),
    );
    object.insert("argv".to_string(), json!(candidate.argv));
    object.insert("env".to_string(), json!(candidate.env));
    if let Some(stdin) = &candidate.stdin {
        object.insert("stdin".to_string(), Value::String(stdin.clone()));
    }
    object.insert(
        "prompt".to_string(),
        Value::String(candidate.prompt.clone()),
    );
    object
}

fn model_provider_args(context: &ExternalProviderDispatchContext) -> Vec<String> {
    context
        .model
        .providers
        .get(context.provider_index)
        .map(|provider| provider.args.clone())
        .unwrap_or_default()
}

fn base_provider_args(
    context: &ExternalProviderDispatchContext,
    model_provider_args: &[String],
) -> Vec<String> {
    let effective_args = context.provider.args.as_slice();
    if effective_args.ends_with(model_provider_args) {
        effective_args[..effective_args.len() - model_provider_args.len()].to_vec()
    } else {
        effective_args.to_vec()
    }
}

fn launch_session(context: &ExternalProviderDispatchContext) -> Option<JsonObject> {
    let mut session = JsonObject::new();
    if let Some(session_id) = &context.start_known_provider_session_id {
        session.insert(
            "known_provider_session_id".to_string(),
            Value::String(session_id.clone()),
        );
    }
    if session.is_empty() {
        None
    } else {
        Some(session)
    }
}

fn mode(context: &ExternalProviderDispatchContext) -> String {
    format!("{:?}", context.prompt_mode).to_lowercase()
}

fn working_directory(context: &ExternalProviderDispatchContext) -> String {
    context
        .working_dir
        .clone()
        .or_else(current_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .display()
        .to_string()
}

fn current_dir() -> Option<PathBuf> {
    std::env::current_dir().ok()
}

fn request_id(label: &str) -> String {
    format!("external-provider-{label}-{}", uuid::Uuid::new_v4())
}
