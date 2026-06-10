//! ## Declared roles
//!
//! `mapper`, `parser`, `validator`

use super::provider_validation;
use super::raw::{RawInput, RawModelToml, raw_provider_name, raw_provider_to_config};

use super::super::{InputDef, InputType, ModelConfig, ModelError, PromptMode};

pub(in crate::model) fn parse_model_toml(text: &str) -> Result<RawModelToml, ModelError> {
    toml::from_str(text).map_err(|err| ModelError::Toml("<unknown>".to_string(), err.to_string()))
}

pub(in crate::model) fn validate_model_toml_against_providers(
    raw: &RawModelToml,
    providers: Option<&crate::providers::ProvidersConfig>,
) -> Result<(), ModelError> {
    validate_raw_model_provider_fields(raw)?;
    let model = apply_model_construction(raw);
    validate_provider_aware_shape("<unknown>", &model, providers)
}

pub(super) fn validate_raw_model_provider_fields(raw: &RawModelToml) -> Result<(), ModelError> {
    validate_legacy_model_fields(raw)?;
    if let Some(provider) = raw.provider.as_ref() {
        provider
            .validate()
            .map_err(|source| ModelError::ProviderImplementationRef {
                model: "<unknown>".to_string(),
                source,
            })?;
    }
    validate_raw_model_providers(raw)?;
    if let Some(inputs) = raw.inputs.clone() {
        parse_inputs(inputs).map_err(|err| ModelError::Other("<unknown>".to_string(), err))?;
    }
    Ok(())
}

fn apply_model_construction(raw: &RawModelToml) -> ModelConfig {
    construct_model_config_from_raw(raw.clone())
}

pub(super) fn validate_provider_aware_shape(
    model_name: &str,
    model: &ModelConfig,
    providers: Option<&crate::providers::ProvidersConfig>,
) -> Result<(), ModelError> {
    if let Some(providers) = providers {
        provider_validation::validate_codex_model_arg_overlap(model_name, model, providers)
            .map_err(|err| ModelError::Other(model_name.to_string(), err))?;
        provider_validation::validate_proxy_claude_model_shape(model_name, model, providers)?;
    }
    Ok(())
}

pub(in crate::model) fn construct_model_config_from_raw(raw: RawModelToml) -> ModelConfig {
    let providers = raw
        .providers
        .unwrap_or_default()
        .into_iter()
        .map(raw_provider_to_config)
        .collect();
    let inputs = raw
        .inputs
        .map(parse_inputs)
        .transpose()
        .expect("validated model inputs")
        .unwrap_or_default();
    ModelConfig {
        name: String::new(),
        prompt_mode: raw
            .prompt_mode
            .as_deref()
            .map(crate::providers::parse_prompt_mode)
            .unwrap_or(PromptMode::Stdin),
        providers,
        inputs,
        provider: raw.provider,
    }
}

fn validate_legacy_model_fields(raw: &RawModelToml) -> Result<(), ModelError> {
    if raw.command.is_some()
        || raw.args.is_some()
        || raw.interactive_args.is_some()
        || raw.resume.is_some()
        || raw.session_capture.is_some()
        || raw.resume_acceptance.is_some()
        || raw.session_storage.is_some()
    {
        return Err(ModelError::Other(
            "<unknown>".to_string(),
            "Old per-provider config detected in <unknown>.toml; keep runtime provider fields in providers.toml and run `agents migrate-config` to repair existing files."
                .to_string(),
        ));
    }
    Ok(())
}

fn validate_raw_model_providers(raw: &RawModelToml) -> Result<(), ModelError> {
    let providers = raw
        .providers
        .as_ref()
        .filter(|providers| !providers.is_empty())
        .ok_or_else(|| {
            ModelError::Other(
                "<unknown>".to_string(),
                "model must declare at least one [[providers]]".to_string(),
            )
        })?;

    for provider in providers {
        let provider_name = raw_provider_name(provider);
        if provider.command.is_some()
            || provider.resume.is_some()
            || provider.prompt_mode.is_some()
            || provider.session_capture.is_some()
            || provider.resume_acceptance.is_some()
            || provider.session_storage.is_some()
        {
            return Err(ModelError::Other(
                "<unknown>".to_string(),
                format!(
                    "Old per-provider config detected in <unknown>.toml provider {provider_name}; keep runtime provider fields in providers.toml and run `agents migrate-config` to repair existing files."
                ),
            ));
        }
        if provider.system_prompt_override.is_some() || provider.tool_restrictions.is_some() {
            return Err(ModelError::Other(
                "<unknown>".to_string(),
                format!(
                    "provider {provider_name}: system_prompt_override and tool_restrictions are root-only provider settings; move them to providers.toml"
                ),
            ));
        }
        if provider.invocation_mode.is_some() {
            return Err(ModelError::InvocationModeIsRootOnly {
                model: "<unknown>".to_string(),
                provider: provider_name,
            });
        }
    }
    Ok(())
}

fn parse_input_type(raw: &RawInput) -> Result<InputType, String> {
    match raw.type_name.as_str() {
        "string" => Ok(InputType::String),
        "integer" => Ok(InputType::Integer {
            min: raw.min.map(|v| v as i64),
            max: raw.max.map(|v| v as i64),
        }),
        "number" => Ok(InputType::Number {
            min: raw.min,
            max: raw.max,
        }),
        "boolean" => Ok(InputType::Boolean),
        "enum" => {
            let options = raw
                .options
                .clone()
                .ok_or_else(|| format!("Input '{}': enum type requires 'options'", raw.name))?;
            Ok(InputType::Enum { options })
        }
        "array" => {
            let item_type = raw
                .item_type
                .clone()
                .unwrap_or_else(|| "string".to_string());
            Ok(InputType::Array {
                item_type,
                min_items: raw.min_items,
                max_items: raw.max_items,
            })
        }
        other => Err(format!("Input '{}': unknown type '{}'", raw.name, other)),
    }
}

pub(super) fn parse_inputs(raw_inputs: Vec<RawInput>) -> Result<Vec<InputDef>, String> {
    let mut inputs = Vec::new();
    let mut has_default_input = false;

    for raw in raw_inputs {
        validate_default_input_uniqueness(&raw, has_default_input)?;
        has_default_input = has_default_input || raw.default_input;
        let input_type = parse_input_type(&raw)?;
        inputs.push(input_def_from_raw(raw, input_type));
    }

    Ok(inputs)
}

fn validate_default_input_uniqueness(
    raw: &RawInput,
    has_default_input: bool,
) -> Result<(), String> {
    if raw.default_input && has_default_input {
        return Err(format!(
            "Input '{}': only one input can have default_input = true",
            raw.name
        ));
    }
    Ok(())
}

fn input_def_from_raw(raw: RawInput, input_type: InputType) -> InputDef {
    InputDef {
        name: raw.name,
        input_type,
        required: raw.required,
        default_input: raw.default_input,
        default: raw.default,
        description: raw.description,
        flag: raw.flag,
    }
}
