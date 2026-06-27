//! ## Declared roles
//!
//! - accessor
//! - filter
//! - formatter
//! - mapper
//! - orchestration
//! - parser
//! - predicate
//! - validator
//!
//! Role set: { accessor, filter, formatter, mapper, orchestration, parser, predicate, validator }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-config/src/model/loader.rs
//!     role: intrinsic-surface
//!     Domain: model-loading-pipeline
//!     Owns:
//!       - model TOML parse, validation, construction, read, and load orchestration
//!       - provider-aware validation against ProvidersConfig and ProviderEntry root configuration
//!       - Codex and Claude provider-shape migration guards subordinate to model loading
//!       - filesystem Path/PathBuf and HashMap carriers used by model loading
//! ```
//!
use crate::claude_tool_filter::{ClaudeToolFilterShape, validate_proxy_claude_filter_shape};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::codex_overlap::validate_codex_model_arg_overlap;
use super::config::{ModelConfig, PromptMode, parse_inputs};
use super::error::ModelError;
use super::provider_config::{InvocationMode, ProviderConfig};
use super::provider_name::derive_provider_name;
use super::raw_toml::{RawModelToml, RawProvider};

pub(super) fn parse_model_toml(text: &str) -> Result<RawModelToml, ModelError> {
    toml::from_str(text).map_err(|err| ModelError::Toml("<unknown>".to_string(), err.to_string()))
}

pub(super) fn validate_model_toml_against_providers(
    raw: &RawModelToml,
    providers: Option<&crate::providers::ProvidersConfig>,
) -> Result<(), ModelError> {
    validate_raw_model_provider_fields(raw)?;
    let model = apply_model_construction(raw);
    validate_provider_aware_shape("<unknown>", &model, providers)
}

fn validate_raw_model_provider_fields(raw: &RawModelToml) -> Result<(), ModelError> {
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

fn validate_provider_aware_shape(
    model_name: &str,
    model: &ModelConfig,
    providers: Option<&crate::providers::ProvidersConfig>,
) -> Result<(), ModelError> {
    if let Some(providers) = providers {
        validate_codex_model_arg_overlap(model_name, model, providers)
            .map_err(|err| ModelError::Other(model_name.to_string(), err))?;
        validate_proxy_claude_model_shape(model_name, model, providers)?;
    }
    Ok(())
}

pub(super) fn construct_model_config_from_raw(raw: RawModelToml) -> ModelConfig {
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

pub(super) fn emit_model_toml(model: &ModelConfig) -> String {
    model.to_toml()
}

pub(super) fn read_model_files(dir: &Path) -> Result<Vec<(PathBuf, String)>, ModelError> {
    list_model_toml_paths(dir)?
        .into_iter()
        .map(|path| {
            let text = read_file_contents(&path)?;
            Ok(pair_path_and_text(path, text))
        })
        .collect()
}

fn read_model_dir_paths(dir: &Path) -> Result<Vec<PathBuf>, ModelError> {
    let mut paths = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(paths),
        Err(err) => return Err(ModelError::Io(err)),
    };
    for entry in entries {
        let entry = entry?;
        paths.push(entry.path());
    }
    Ok(paths)
}

fn select_toml_paths(mut paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths.retain(|path| path.extension().and_then(|ext| ext.to_str()) == Some("toml"));
    paths.sort();
    paths
}

fn list_model_toml_paths(dir: &Path) -> Result<Vec<PathBuf>, ModelError> {
    Ok(select_toml_paths(read_model_dir_paths(dir)?))
}

fn read_file_contents(path: &Path) -> Result<String, ModelError> {
    std::fs::read_to_string(path).map_err(ModelError::Io)
}

fn pair_path_and_text(path: PathBuf, text: String) -> (PathBuf, String) {
    (path, text)
}

pub(super) fn parse_model_files(
    files: Vec<(PathBuf, String)>,
) -> Result<Vec<(PathBuf, RawModelToml)>, ModelError> {
    files
        .into_iter()
        .map(|(path, text)| {
            let raw = parse_model_toml(&text).map_err(|err| match model_name_from_path(&path) {
                Ok(name) => err.with_model_name(&name),
                Err(_) => err,
            })?;
            Ok((path, raw))
        })
        .collect()
}

pub(super) fn validate_models_against_providers(
    raws: &[(PathBuf, RawModelToml)],
    providers: Option<&crate::providers::ProvidersConfig>,
) -> Result<(), ModelError> {
    for (path, raw) in raws {
        let name = model_name_from_path(path)?;
        validate_model_toml_against_providers(raw, providers)
            .map_err(|err| err.with_model_name(&name))?;
    }
    Ok(())
}

fn model_name_from_path(path: &Path) -> Result<String, ModelError> {
    let stem = extract_model_file_stem(path).ok_or_else(|| format_invalid_model_filename(path))?;
    let stem = validate_model_file_stem(stem).map_err(|()| format_invalid_model_filename(path))?;
    Ok(map_model_file_stem_to_name(stem))
}

fn extract_model_file_stem(path: &Path) -> Option<&str> {
    path.file_stem().and_then(|stem| stem.to_str())
}

fn validate_model_file_stem(stem: &str) -> Result<&str, ()> {
    if stem.is_empty() {
        return Err(());
    }
    Ok(stem)
}

fn format_invalid_model_filename(path: &Path) -> ModelError {
    ModelError::Other(
        "<unknown>".to_string(),
        format!("invalid model filename {}", path.display()),
    )
}

fn map_model_file_stem_to_name(stem: &str) -> String {
    stem.to_string()
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

fn raw_provider_name(provider: &RawProvider) -> String {
    provider.name.clone().unwrap_or_else(|| {
        provider
            .command
            .as_deref()
            .map(|command| derive_provider_name(command, provider.args.as_deref().unwrap_or(&[])))
            .unwrap_or_else(|| "<unknown>".to_string())
    })
}

fn raw_provider_to_config(raw: RawProvider) -> ProviderConfig {
    let args = raw.args.unwrap_or_default();
    let name = raw.name.unwrap_or_else(|| {
        raw.command
            .as_deref()
            .map(|command| derive_provider_name(command, &args))
            .unwrap_or_default()
    });
    ProviderConfig {
        name,
        command: String::new(),
        args,
        interactive_args: raw.interactive_args,
        resume: None,
        session_capture: None,
        resume_acceptance: None,
        session_storage: None,
        system_prompt_override: None,
        tool_restrictions: None,
        invocation_mode: InvocationMode::Headless,
    }
}

fn validate_proxy_claude_model_shape(
    model_name: &str,
    model: &ModelConfig,
    providers: &crate::providers::ProvidersConfig,
) -> Result<(), ModelError> {
    for model_provider in &model.providers {
        if !is_claude_provider(model_provider, providers) {
            continue;
        }
        let Some(effective) = resolve_effective_provider_for_model(model_provider, providers)
        else {
            continue;
        };
        let argv = build_provider_argv_tokens(&effective);
        let shape =
            ClaudeToolFilterShape::detect_in_argv(&argv).unwrap_or(ClaudeToolFilterShape::NoFilter);
        validate_proxy_claude_filter_shape(effective.invocation_mode, shape).map_err(|err| {
            ModelError::Other(
                model_name.to_string(),
                format!("provider {}: {err}", model_provider.name),
            )
        })?;
    }
    Ok(())
}

fn is_claude_provider(
    model_provider: &ProviderConfig,
    providers: &crate::providers::ProvidersConfig,
) -> bool {
    let Some(root) = providers.get(&model_provider.name) else {
        return false;
    };
    let root_family = derive_provider_name(
        root.command.as_deref().unwrap_or(&model_provider.name),
        &root.args,
    );
    root_family
        .rsplit('/')
        .next()
        .unwrap_or(&root_family)
        .starts_with("claude")
        || model_provider.name.starts_with("claude")
}

fn resolve_effective_provider_for_model(
    model_provider: &ProviderConfig,
    providers: &crate::providers::ProvidersConfig,
) -> Option<ProviderConfig> {
    providers
        .effective_provider(model_provider)
        .ok()
        .map(|(effective, _)| effective)
}

fn build_provider_argv_tokens(provider: &ProviderConfig) -> Vec<String> {
    let mut argv = crate::providers::shell_split(&provider.command);
    argv.extend(provider.args.iter().cloned());
    if let Some(interactive_args) = provider.interactive_args.as_ref() {
        argv.extend(interactive_args.iter().cloned());
    }
    argv
}

pub fn render_validated_model_toml(
    model: &ModelConfig,
    providers: Option<&crate::providers::ProvidersConfig>,
) -> Result<String, ModelError> {
    let rendered = emit_model_toml(model);
    let raw = parse_model_toml(&rendered)?;
    validate_model_toml_against_providers(&raw, providers)?;
    Ok(rendered)
}

impl ModelConfig {
    pub fn from_toml(
        text: &str,
        providers: Option<&crate::providers::ProvidersConfig>,
    ) -> Result<Self, ModelError> {
        let raw = parse_model_toml(text)?;
        validate_model_toml_against_providers(&raw, providers)?;
        Ok(construct_model_config_from_raw(raw))
    }

    pub fn from_toml_with_name(
        name: &str,
        text: &str,
        providers: Option<&crate::providers::ProvidersConfig>,
    ) -> Result<Self, ModelError> {
        let model = Self::from_toml(text, providers).map_err(|err| err.with_model_name(name))?;
        Ok(apply_name_to_model(model, name))
    }
}

fn apply_name_to_model(mut model: ModelConfig, name: &str) -> ModelConfig {
    if model.name.is_empty() {
        model.name = name.to_string();
    }
    model
}

pub fn load_models(
    models_dir: &Path,
    providers: Option<&crate::providers::ProvidersConfig>,
) -> Result<HashMap<String, ModelConfig>, ModelError> {
    let files = read_model_files(models_dir)?;
    let raws = parse_model_files(files)?;
    validate_models_against_providers(&raws, providers)?;
    build_named_model_map(raws)
}

fn build_named_model_map(
    raws: Vec<(PathBuf, RawModelToml)>,
) -> Result<HashMap<String, ModelConfig>, ModelError> {
    raws.into_iter()
        .map(|(path, raw)| {
            let name = model_name_from_path(&path)?;
            let mut model = construct_model_config_from_raw(raw);
            model.name = name.clone();
            Ok((name, model))
        })
        .collect()
}
