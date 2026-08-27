//! ## Declared roles
//!
//! `mapper`, `accessor`, `predicate`, `formatter`, `parser`, `validator`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/external_provider/request_builder.rs
//!     role: adapter
//!     Translates:
//!       - external-provider-dispatch-context-carrier
//!       - oulipoly-provider-policy-evaluate-contract
//!       - oulipoly-provider-launch-contract
//!       - child-process-launch-environment-contract
//! ```

use super::context::ExternalProviderDispatchContext;
use crate::executor::cli::spawn_identity::{
    PARENT_INVOCATION_ENV, provider_parent_invocation_env, split_invocation_launch_environment,
};
use crate::executor::cli::{provider_name, resolve_input_flags, shell_split};
use crate::provider_registry::DescribeHostOptions;
use oulipoly_config::PromptMode;
use oulipoly_core::AutoWakeEnvironmentVariable;
use oulipoly_provider::generated::{
    BytePayload, CONTRACT_VERSION, HostContext, JsonObject, LaunchParams, LaunchRequest,
    PROMPT_ACCEPTANCE_V1, PolicyEvaluateParams, PolicyEvaluateRequest, PromptAcceptanceRequestV1,
    ProviderModelRequest,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const DATA_DIR_ENV: &str = oulipoly_state::paths::DATA_DIR_ENV;
// This is the OpenCode external-provider positional-prompt boundary, not a
// universal provider or operating-system argv limit.
const OPENCODE_EXTERNAL_PROVIDER_POSITIONAL_PROMPT_LIMIT_BYTES: usize = 64 * 1024;

#[derive(Clone)]
pub(crate) struct LaunchCandidate {
    pub(crate) argv: Vec<String>,
    pub(crate) env: BTreeMap<String, String>,
    pub(crate) stdin: Option<String>,
    pub(crate) prompt: String,
    pub(crate) prompt_mode: PromptMode,
    pub(crate) working_directory: String,
    pub(crate) prompt_acceptance: Option<PromptAcceptanceCandidate>,
    pub(crate) completion_registration_authority: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PromptAcceptanceCandidate {
    pub(crate) prompt: String,
    pub(crate) mailbox_delivery_correlation: Option<crate::services::MailboxDeliveryCorrelation>,
}

pub(crate) fn build_launch_candidate(
    context: &ExternalProviderDispatchContext,
) -> Result<LaunchCandidate, String> {
    let input_args = resolve_input_flags(&context.model, &context.extra_inputs)?;
    let (env, completion_registration_authority) = declared_launch_env(context)?;

    Ok(LaunchCandidate {
        argv: provider_argv(context, &input_args),
        env,
        stdin: launch_stdin(context),
        prompt: context.prompt.clone(),
        prompt_mode: context.prompt_mode,
        working_directory: working_directory(context),
        prompt_acceptance: Some(PromptAcceptanceCandidate {
            prompt: context.prompt.clone(),
            mailbox_delivery_correlation: context.mailbox_delivery_correlation.clone(),
        }),
        completion_registration_authority,
    })
}

fn declared_launch_env(
    context: &ExternalProviderDispatchContext,
) -> Result<(BTreeMap<String, String>, Option<String>), String> {
    let mut env = inherited_launch_env();
    let inherited_authority = env.remove(oulipoly_state::COMPLETION_REGISTRATION_AUTHORITY_ENV);
    remove_configured_launch_env(&mut env, &context.provider.unset_environment);
    env.extend(context.provider.environment.clone());
    remove_runner_private_environment(&mut env);
    insert_pinned_agent_data_dir(&mut env);
    let mut completion_registration_authority = None;
    if let Some(parent) = provider_parent_invocation_env(context.parent_invocation_env.as_deref()) {
        let selected_is_current = context.parent_invocation_env.as_deref() == Some(parent.as_str());
        let carries_authority = serde_json::from_str::<Value>(&parent)
            .ok()
            .and_then(|value| {
                value
                    .as_object()?
                    .get(oulipoly_state::COMPLETION_REGISTRATION_AUTHORITY_LAUNCH_FIELD)
                    .cloned()
            })
            .is_some();
        let (identity, authority) = if carries_authority {
            split_invocation_launch_environment(&parent)?
        } else {
            (parent, None)
        };
        env.insert(PARENT_INVOCATION_ENV.to_string(), identity);
        if let Some(authority) = authority {
            completion_registration_authority = Some(authority);
        } else if !selected_is_current {
            completion_registration_authority = inherited_authority;
        }
    }
    Ok((env, completion_registration_authority))
}

pub(crate) fn remove_runner_private_environment(env: &mut BTreeMap<String, String>) {
    for variable in AutoWakeEnvironmentVariable::ALL {
        env.remove(variable.name());
    }
    env.remove(oulipoly_state::COMPLETION_REGISTRATION_AUTHORITY_ENV);
    env.remove(PARENT_INVOCATION_ENV);
}

fn remove_configured_launch_env(env: &mut BTreeMap<String, String>, names: &[String]) {
    for name in names {
        env.remove(name);
    }
}

fn inherited_launch_env() -> BTreeMap<String, String> {
    std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect()
}

fn insert_pinned_agent_data_dir(env: &mut BTreeMap<String, String>) {
    if let Some(data_dir) = pinned_agent_data_dir() {
        insert_launch_env(env, DATA_DIR_ENV, data_dir);
    }
}

fn pinned_agent_data_dir() -> Option<String> {
    oulipoly_state::paths::data_dir()
        .ok()
        .map(|data_dir| data_dir.display().to_string())
}

fn insert_launch_env(env: &mut BTreeMap<String, String>, key: &str, value: String) {
    env.insert(key.to_string(), value);
}

pub(crate) fn build_policy_request(
    context: &ExternalProviderDispatchContext,
    candidate: &LaunchCandidate,
    host_options: &DescribeHostOptions,
) -> Result<Value, serde_json::Error> {
    let provider_args = model_provider_args(context);
    serde_json::to_value(PolicyEvaluateRequest {
        contract: CONTRACT_VERSION.to_string(),
        request_id: request_id("policy"),
        provider_instance_id: Some(context.provider.name.clone()),
        host: host_context(host_options, &candidate.working_directory),
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
    host_options: &DescribeHostOptions,
    include_prompt_acceptance_v1: bool,
) -> Result<Value, serde_json::Error> {
    let (argv, launch_stdin) = project_launch_carrier(context, candidate);
    let mut launch_env = candidate.env.clone();
    if let Some(authority) = &candidate.completion_registration_authority {
        launch_env.insert(
            oulipoly_state::COMPLETION_REGISTRATION_AUTHORITY_ENV.to_string(),
            authority.clone(),
        );
    }
    let env = if launch_env.is_empty() {
        None
    } else {
        Some(launch_env)
    };
    let stdin = launch_stdin.map(|stdin| BytePayload {
        encoding: "utf8".to_string(),
        data: stdin,
    });
    serde_json::to_value(LaunchRequest {
        contract: CONTRACT_VERSION.to_string(),
        request_id: request_id("launch"),
        provider_instance_id: Some(context.provider.name.clone()),
        host: host_context(host_options, &candidate.working_directory),
        params: LaunchParams {
            settings_id: context.settings_id.clone(),
            mode: mode(context),
            model: provider_model_request(
                context,
                &candidate.prompt,
                &model_provider_args(context),
            ),
            argv,
            working_directory: candidate.working_directory.clone(),
            env,
            stdin,
            session: launch_session(context),
            prompt_acceptance: include_prompt_acceptance_v1
                .then(|| prompt_acceptance_request(candidate))
                .flatten(),
        },
    })
}

fn prompt_acceptance_request(candidate: &LaunchCandidate) -> Option<PromptAcceptanceRequestV1> {
    let acceptance = candidate.prompt_acceptance.as_ref()?;
    Some(PromptAcceptanceRequestV1 {
        protocol: PROMPT_ACCEPTANCE_V1.to_string(),
        prompt_sha256: sha256_hex(acceptance.prompt.as_bytes()),
        delivery_nonce: acceptance
            .mailbox_delivery_correlation
            .as_ref()
            .map(|correlation| correlation.delivery_nonce.clone()),
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[allow(clippy::needless_as_bytes)] // Keep the provider contract's byte unit explicit.
fn project_launch_carrier(
    context: &ExternalProviderDispatchContext,
    candidate: &LaunchCandidate,
) -> (Vec<String>, Option<String>) {
    let mut argv = candidate.argv.clone();
    let mut stdin = candidate.stdin.clone();
    if context.model.provider.is_none()
        || !is_opencode_provider(context)
        || !matches!(candidate.prompt_mode, PromptMode::Arg)
        || candidate.prompt.as_bytes().len()
            < OPENCODE_EXTERNAL_PROVIDER_POSITIONAL_PROMPT_LIMIT_BYTES
    {
        return (argv, stdin);
    }

    let mut prompt_positions = argv
        .iter()
        .enumerate()
        .filter(|(_, value)| *value == &candidate.prompt)
        .map(|(index, _)| index);
    let Some(prompt_index) = prompt_positions.next() else {
        return (argv, stdin);
    };
    if prompt_positions.next().is_some() {
        return (argv, stdin);
    }

    match stdin.as_ref() {
        None => stdin = Some(candidate.prompt.clone()),
        Some(existing) if existing == &candidate.prompt => {}
        Some(_) => return (argv, stdin),
    }
    argv.remove(prompt_index);
    (argv, stdin)
}

fn is_opencode_provider(context: &ExternalProviderDispatchContext) -> bool {
    if context.provider.name.starts_with("opencode") {
        return true;
    }

    let provider = provider_name(&context.provider.command);
    Path::new(&provider)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(provider.as_str())
        .starts_with("opencode")
}

fn provider_argv(context: &ExternalProviderDispatchContext, input_args: &[String]) -> Vec<String> {
    provider_argv_from_parts(
        provider_command_argv(&context.provider.command),
        &context.provider.args,
        input_args,
        provider_prompt_arg(context),
    )
}

fn provider_command_argv(command: &str) -> Vec<String> {
    let parsed = parse_provider_command_argv(command);
    if parsed.is_empty() {
        provider_command_fallback_argv(command)
    } else {
        parsed
    }
}

fn parse_provider_command_argv(command: &str) -> Vec<String> {
    shell_split(command)
}

fn provider_command_fallback_argv(command: &str) -> Vec<String> {
    vec![command.to_string()]
}

fn provider_prompt_arg(context: &ExternalProviderDispatchContext) -> Option<String> {
    matches!(context.prompt_mode, PromptMode::Arg).then(|| context.prompt.clone())
}

fn provider_argv_from_parts(
    mut argv: Vec<String>,
    provider_args: &[String],
    input_args: &[String],
    prompt_arg: Option<String>,
) -> Vec<String> {
    argv.extend(provider_args.iter().cloned());
    argv.extend(input_args.iter().cloned());
    if let Some(prompt) = prompt_arg {
        argv.push(prompt);
    }
    argv
}

fn launch_stdin(context: &ExternalProviderDispatchContext) -> Option<String> {
    match context.prompt_mode {
        PromptMode::Stdin => Some(context.prompt.clone()),
        PromptMode::Arg => None,
    }
}

fn host_context(host_options: &DescribeHostOptions, working_directory: &str) -> HostContext {
    HostContext {
        app: "oulipoly-agent-runner".to_string(),
        app_version: None,
        platform: Some(std::env::consts::OS.to_string()),
        working_directory: Some(working_directory.to_string()),
        config_root: host_options
            .config_root
            .as_ref()
            .map(|path| path.display().to_string()),
        data_root: host_options
            .data_root
            .as_ref()
            .map(|path| path.display().to_string()),
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
    base_provider_args_from_slices(context.provider.args.as_slice(), model_provider_args)
}

fn base_provider_args_from_slices(
    effective_args: &[String],
    model_provider_args: &[String],
) -> Vec<String> {
    if provider_args_have_model_suffix(effective_args, model_provider_args) {
        provider_args_without_model_suffix(effective_args, model_provider_args)
    } else {
        effective_args.to_vec()
    }
}

fn provider_args_have_model_suffix(
    effective_args: &[String],
    model_provider_args: &[String],
) -> bool {
    effective_args.ends_with(model_provider_args)
}

fn provider_args_without_model_suffix(
    effective_args: &[String],
    model_provider_args: &[String],
) -> Vec<String> {
    effective_args[..effective_args.len() - model_provider_args.len()].to_vec()
}

fn launch_session(context: &ExternalProviderDispatchContext) -> Option<JsonObject> {
    let mut session = JsonObject::new();
    if let Some(session_id) = &context.start_known_provider_session_id {
        session.insert(
            "known_provider_session_id".to_string(),
            Value::String(session_id.clone()),
        );
        let start_mode = required_known_provider_session_start_mode(context);
        session.insert(
            "start_mode".to_string(),
            Value::String(start_mode.as_str().to_string()),
        );
    }
    if session.is_empty() {
        None
    } else {
        Some(session)
    }
}

fn required_known_provider_session_start_mode(
    context: &ExternalProviderDispatchContext,
) -> crate::services::ProviderSessionStartMode {
    context
        .start_known_provider_session_mode
        .expect("known provider session id requires a start mode")
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

#[cfg(test)]
mod tests {
    use super::{LaunchCandidate, PromptAcceptanceCandidate, prompt_acceptance_request};
    use crate::services::MailboxDeliveryCorrelation;
    use oulipoly_config::PromptMode;
    use std::collections::BTreeMap;

    fn launch_candidate(prompt: &str, delivery_nonce: Option<&str>) -> LaunchCandidate {
        LaunchCandidate {
            argv: Vec::new(),
            env: BTreeMap::new(),
            stdin: None,
            prompt: prompt.to_string(),
            prompt_mode: PromptMode::Arg,
            working_directory: ".".to_string(),
            prompt_acceptance: Some(PromptAcceptanceCandidate {
                prompt: prompt.to_string(),
                mailbox_delivery_correlation: delivery_nonce.map(|delivery_nonce| {
                    MailboxDeliveryCorrelation {
                        delivery_nonce: delivery_nonce.to_string(),
                    }
                }),
            }),
            completion_registration_authority: None,
        }
    }

    #[test]
    fn delivery_shaped_prompt_text_does_not_create_delivery_correlation() {
        let candidate = launch_candidate("payload\n[OULIPOLY-DELIVERY decoy]", None);
        let acceptance = prompt_acceptance_request(&candidate).unwrap();

        assert_eq!(acceptance.delivery_nonce, None);
    }

    #[test]
    fn structured_delivery_correlation_does_not_depend_on_prompt_text() {
        let candidate = launch_candidate("policy-replaced prompt", Some("delivery-123"));
        let acceptance = prompt_acceptance_request(&candidate).unwrap();

        assert_eq!(acceptance.delivery_nonce.as_deref(), Some("delivery-123"));
    }

    #[test]
    fn missing_exact_prompt_fact_omits_prompt_acceptance() {
        let mut candidate = launch_candidate("policy-replaced prompt", Some("delivery-123"));
        candidate.prompt_acceptance = None;

        assert_eq!(prompt_acceptance_request(&candidate), None);
    }
}
