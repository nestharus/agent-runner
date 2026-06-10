//! ## Declared roles
//!
//! `accessor`, `mapper`, `orchestration`, `parser`, `validator`

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::super::{ModelConfig, ModelError};
use super::parse::{
    construct_model_config_from_raw, parse_model_toml, validate_model_toml_against_providers,
};
use super::raw::RawModelToml;

pub(in crate::model) fn read_model_files(dir: &Path) -> Result<Vec<(PathBuf, String)>, ModelError> {
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

pub(in crate::model) fn parse_model_files(
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

pub(in crate::model) fn validate_models_against_providers(
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
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| {
            ModelError::Other(
                "<unknown>".to_string(),
                format!("invalid model filename {}", path.display()),
            )
        })
}

pub(super) fn build_named_model_map(
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
