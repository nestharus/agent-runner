//! ## Declared roles
//!
//! `formatter`, `orchestration`, `parser`, `validator`

mod files;
mod format;
mod parse;
mod provider_validation;
mod raw;

use std::collections::HashMap;
use std::path::Path;

use super::{ModelConfig, ModelError};

#[cfg(test)]
pub(in crate::model) use files::{
    parse_model_files, read_model_files, validate_models_against_providers,
};
#[cfg(test)]
pub(in crate::model) use format::emit_model_toml;
#[cfg(test)]
pub(in crate::model) use parse::{
    construct_model_config_from_raw, parse_model_toml, validate_model_toml_against_providers,
};
#[cfg(test)]
pub(in crate::model) use provider_validation::{
    CodexArgPart, codex_arg_overlap, split_codex_arg_parts, validate_codex_model_arg_overlap,
};

impl ModelConfig {
    pub fn to_toml(&self) -> String {
        format::emit_model_toml(self)
    }

    pub fn from_toml(
        text: &str,
        providers: Option<&crate::providers::ProvidersConfig>,
    ) -> Result<Self, ModelError> {
        let raw = parse::parse_model_toml(text)?;
        parse::validate_model_toml_against_providers(&raw, providers)?;
        Ok(parse::construct_model_config_from_raw(raw))
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

pub fn render_validated_model_toml(
    model: &ModelConfig,
    providers: Option<&crate::providers::ProvidersConfig>,
) -> Result<String, ModelError> {
    let rendered = format::emit_model_toml(model);
    let raw = parse::parse_model_toml(&rendered)?;
    parse::validate_model_toml_against_providers(&raw, providers)?;
    Ok(rendered)
}

pub fn load_models(
    models_dir: &Path,
    providers: Option<&crate::providers::ProvidersConfig>,
) -> Result<HashMap<String, ModelConfig>, ModelError> {
    let files = files::read_model_files(models_dir)?;
    let raws = files::parse_model_files(files)?;
    files::validate_models_against_providers(&raws, providers)?;
    files::build_named_model_map(raws)
}
