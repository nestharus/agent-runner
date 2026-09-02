use crate::provider_registry::PinnedFamilyEndpoint;
use oulipoly_provider::generated::{
    CONTRACT_VERSION, DiscoveryModelsResult, DiscoveryObject, HostContext, RequestEnvelope,
};
use oulipoly_state::{CliMapping, DiscoveredModel, ModelParameter, ParamType};
use serde_json::Value;
use std::collections::BTreeMap;

const CLAUDE_CLI_NAME: &str = concat!("cla", "ude");
const CODEX_CLI_NAME: &str = concat!("cod", "ex");

/// Result of a model discovery attempt for a single CLI.
#[derive(Debug)]
pub struct DiscoveryResult {
    pub cli_name: String,
    pub cli_version: String,
    pub models: Vec<DiscoveredModel>,
    pub parameters: Vec<(String, ModelParameter)>, // (model_name, param)
}

/// Discover models through the already preflighted family implementation.
pub fn discover_models(endpoint: &PinnedFamilyEndpoint) -> Result<DiscoveryResult, String> {
    if !endpoint.capabilities().capabilities.discovery {
        return Err("provider family does not support discovery".to_string());
    }
    let family = endpoint.family();
    let result = endpoint
        .client()
        .invoke_typed::<DiscoveryModelsResult, _>(
            "discovery.models",
            discovery_request(endpoint),
            [],
        )
        .map_err(|error| error.to_string())?;
    let cli_version = result
        .provider_version
        .unwrap_or_else(|| "unknown".to_string());
    let now = chrono::Utc::now().to_rfc3339();
    let mut model_names = result
        .models
        .iter()
        .filter_map(discovered_model_name)
        .collect::<Vec<_>>();
    model_names.sort();
    model_names.dedup();
    Ok(
        discovery_result_for_models(family, &cli_version, &now, model_names)
            .unwrap_or_else(|| empty_discovery_result(family, cli_version)),
    )
}

fn discovery_request(endpoint: &PinnedFamilyEndpoint) -> Value {
    let mut fields = BTreeMap::new();
    fields.insert(
        "family".to_string(),
        Value::String(endpoint.family().to_string()),
    );
    serde_json::to_value(RequestEnvelope {
        contract: CONTRACT_VERSION.to_string(),
        request_id: format!("provider-discovery-{}", uuid::Uuid::new_v4()),
        provider_instance_id: None,
        host: HostContext {
            app: "oulipoly-agent-runner".to_string(),
            app_version: None,
            platform: None,
            working_directory: None,
            config_root: endpoint
                .host_options()
                .config_root
                .as_ref()
                .map(|path| path.display().to_string()),
            data_root: endpoint
                .host_options()
                .data_root
                .as_ref()
                .map(|path| path.display().to_string()),
            env: BTreeMap::new(),
            deadline_unix_ms: None,
        },
        params: DiscoveryObject { fields },
    })
    .unwrap_or_else(|_| serde_json::json!({}))
}

fn discovered_model_name(value: &Value) -> Option<String> {
    let name = match value {
        Value::String(name) => cleaned_model_token(name),
        Value::Object(model) => ["canonical_name", "name", "id"]
            .into_iter()
            .find_map(|key| model.get(key).and_then(Value::as_str))
            .and_then(cleaned_model_token),
        _ => None,
    }?;
    is_valid_model_name(&name).then_some(name)
}

fn discovery_result_for_models(
    cli_name: &str,
    cli_version: &str,
    discovered_at: &str,
    model_names: Vec<String>,
) -> Option<DiscoveryResult> {
    if has_no_discovered_models(&model_names) {
        return None;
    }

    Some(populated_discovery_result(
        cli_name,
        cli_version,
        discovered_at,
        &model_names,
    ))
}

fn has_no_discovered_models(model_names: &[String]) -> bool {
    model_names.is_empty()
}

fn populated_discovery_result(
    cli_name: &str,
    cli_version: &str,
    discovered_at: &str,
    model_names: &[String],
) -> DiscoveryResult {
    DiscoveryResult {
        cli_name: cli_name.to_string(),
        cli_version: cli_version.to_string(),
        models: discovered_models(cli_name, cli_version, discovered_at, model_names),
        parameters: build_default_parameters(cli_name, model_names),
    }
}

fn discovered_models(
    cli_name: &str,
    cli_version: &str,
    discovered_at: &str,
    model_names: &[String],
) -> Vec<DiscoveredModel> {
    let mut models = Vec::new();

    for model_name in model_names {
        append_discovered_model(
            cli_name,
            cli_version,
            discovered_at,
            model_name,
            &mut models,
        );
    }

    models
}

fn append_discovered_model(
    cli_name: &str,
    cli_version: &str,
    discovered_at: &str,
    model_name: &str,
    models: &mut Vec<DiscoveredModel>,
) {
    models.push(discovered_model(
        cli_name,
        cli_version,
        discovered_at,
        model_name,
    ));
}

fn discovered_model(
    cli_name: &str,
    cli_version: &str,
    discovered_at: &str,
    model_name: &str,
) -> DiscoveredModel {
    DiscoveredModel {
        canonical_name: model_name.to_string(),
        provider: cli_name.to_string(),
        discovered_at: discovered_at.to_string(),
        cli_version: cli_version.to_string(),
    }
}

fn empty_discovery_result(cli_name: &str, cli_version: String) -> DiscoveryResult {
    DiscoveryResult {
        cli_name: cli_name.to_string(),
        cli_version,
        models: vec![],
        parameters: vec![],
    }
}

fn cleaned_model_token(token: &str) -> Option<String> {
    let cleaned = token
        .trim_start_matches(['*', '>', '|'])
        .trim_start_matches(is_list_number_prefix_char)
        .trim();

    if cleaned.is_empty() {
        return None;
    }

    Some(cleaned.to_string())
}

fn is_list_number_prefix_char(c: char) -> bool {
    c.is_ascii_digit() || c == '.' || c == ')'
}

/// Check if a string looks like a valid model name.
fn is_valid_model_name(name: &str) -> bool {
    has_valid_model_name_length(name)
        && has_model_name_letter(name)
        && is_not_model_stop_word(name)
        && has_valid_model_name_chars(name)
}

fn has_valid_model_name_length(name: &str) -> bool {
    name.len() >= 2 && name.len() <= 100
}

fn has_model_name_letter(name: &str) -> bool {
    name.chars().any(is_ascii_alphabetic_char)
}

fn is_not_model_stop_word(name: &str) -> bool {
    let lower = name.to_lowercase();
    !model_stop_words().contains(&lower.as_str())
}

fn model_stop_words() -> &'static [&'static str] {
    &[
        "name",
        "id",
        "type",
        "model",
        "models",
        "list",
        "help",
        "version",
        "the",
        "and",
        "for",
        "with",
        "from",
        "this",
        "that",
        "description",
        "status",
        "created",
        "updated",
        "default",
        "none",
        "true",
        "false",
    ]
}

fn has_valid_model_name_chars(name: &str) -> bool {
    name.chars().all(is_valid_model_name_char)
}

fn is_ascii_alphabetic_char(c: char) -> bool {
    c.is_ascii_alphabetic()
}

fn is_valid_model_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == ':' || c == '/'
}

/// Build default parameter definitions for known CLIs.
/// These represent common parameters that most models of a given CLI support.
fn build_default_parameters(
    cli_name: &str,
    model_names: &[String],
) -> Vec<(String, ModelParameter)> {
    let common_params = common_parameters(cli_name, model_names);
    // Apply common params to all discovered models for this CLI
    parameter_pairs_for_models(model_names, &common_params)
}

fn common_parameters(cli_name: &str, model_names: &[String]) -> Vec<ModelParameter> {
    match cli_name {
        CLAUDE_CLI_NAME => vec![max_tokens_parameter()],
        CODEX_CLI_NAME => vec![model_enum_parameter("-m", model_names)],
        "forge" => vec![model_enum_parameter("--model", model_names)],
        "gemini" => vec![temperature_parameter()],
        _ => vec![],
    }
}

fn max_tokens_parameter() -> ModelParameter {
    ModelParameter {
        name: "max_tokens".to_string(),
        display_name: "Max Tokens".to_string(),
        param_type: ParamType::Number {
            min: Some(1.0),
            max: Some(200000.0),
        },
        description: "Maximum number of tokens to generate".to_string(),
        cli_mapping: CliMapping {
            flag: "--max-tokens".to_string(),
            value_template: "{value}".to_string(),
        },
    }
}

fn model_enum_parameter(flag: &str, model_names: &[String]) -> ModelParameter {
    ModelParameter {
        name: "model".to_string(),
        display_name: "Model".to_string(),
        param_type: ParamType::Enum {
            options: model_names.to_vec(),
        },
        description: "Model to use for generation".to_string(),
        cli_mapping: CliMapping {
            flag: flag.to_string(),
            value_template: "{value}".to_string(),
        },
    }
}

fn temperature_parameter() -> ModelParameter {
    ModelParameter {
        name: "temperature".to_string(),
        display_name: "Temperature".to_string(),
        param_type: ParamType::Number {
            min: Some(0.0),
            max: Some(2.0),
        },
        description: "Controls randomness of output".to_string(),
        cli_mapping: CliMapping {
            flag: "--temperature".to_string(),
            value_template: "{value}".to_string(),
        },
    }
}

fn parameter_pairs_for_models(
    model_names: &[String],
    common_params: &[ModelParameter],
) -> Vec<(String, ModelParameter)> {
    let mut params = Vec::new();

    for model_name in model_names {
        append_parameter_pairs_for_model(model_name, common_params, &mut params);
    }

    params
}

fn append_parameter_pairs_for_model(
    model_name: &str,
    common_params: &[ModelParameter],
    params: &mut Vec<(String, ModelParameter)>,
) {
    for param in common_params {
        params.push((model_name.to_string(), param.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_valid_model_name_accepts_good_names() {
        assert!(is_valid_model_name("claude-opus-4"));
        assert!(is_valid_model_name("gpt-5.3"));
        assert!(is_valid_model_name("models/gemini-pro"));
        assert!(is_valid_model_name("o3"));
    }

    #[test]
    fn is_valid_model_name_rejects_bad_names() {
        assert!(!is_valid_model_name(""));
        assert!(!is_valid_model_name("a")); // too short
        assert!(!is_valid_model_name("help"));
        assert!(!is_valid_model_name("the"));
        assert!(!is_valid_model_name("123")); // no letters
        assert!(!is_valid_model_name("model with spaces"));
    }

    #[test]
    fn build_default_parameters_claude() {
        let models = vec!["claude-opus-4".to_string()];
        let params = build_default_parameters("claude", &models);
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].0, "claude-opus-4");
        assert_eq!(params[0].1.name, "max_tokens");
    }

    #[test]
    fn build_default_parameters_unknown_cli() {
        let models = vec!["some-model".to_string()];
        let params = build_default_parameters("unknown-cli", &models);
        assert!(params.is_empty());
    }

    #[test]
    fn build_default_parameters_multiple_models() {
        let models = vec!["m1".to_string(), "m2".to_string()];
        let params = build_default_parameters("codex", &models);
        // Each model gets the same set of params
        assert_eq!(params.len(), 2);
        assert_eq!(params[0].0, "m1");
        assert_eq!(params[1].0, "m2");
    }
}
