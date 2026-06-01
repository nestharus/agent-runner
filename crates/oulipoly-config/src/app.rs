use std::path::Path;

use crate::ProviderImplementationRef;

#[derive(Debug, Clone, Default)]
pub struct AppConfig {
    pub diagnostics_model: Option<String>,
    pub default_provider: Option<String>,
    pub setup: SetupConfig,
}

#[derive(Debug, Clone, Default)]
pub struct SetupConfig {
    pub brain: Option<SetupBrainConfig>,
}

#[derive(Debug, Clone)]
pub struct SetupBrainConfig {
    pub artifact: ProviderImplementationRef,
    pub settings_id: Option<String>,
}

impl AppConfig {
    pub fn load(path: &Path) -> Result<Self, String> {
        let Ok(content) = std::fs::read_to_string(path) else {
            return Ok(Self::default());
        };

        let table = content.parse::<toml::Table>().map_err(|error| {
            format!("failed to parse app config at {}: {error}", path.display())
        })?;

        Ok(Self {
            diagnostics_model: table
                .get("diagnostics_model")
                .and_then(|value| value.as_str())
                .map(String::from),
            default_provider: table
                .get("default_provider")
                .and_then(|value| value.as_str())
                .map(String::from),
            setup: parse_setup_config(&table)?,
        })
    }
}

fn parse_setup_config(table: &toml::Table) -> Result<SetupConfig, String> {
    let Some(setup_value) = table.get("setup") else {
        return Ok(SetupConfig::default());
    };
    let Some(setup_table) = setup_value.as_table() else {
        return Err("setup config field `setup` must be a table".to_string());
    };

    let brain = match setup_table.get("brain") {
        Some(value) => {
            let table = value
                .as_table()
                .ok_or_else(|| "setup config field `setup.brain` must be a table".to_string())?;
            Some(parse_setup_brain_config(table)?)
        }
        None => None,
    };

    Ok(SetupConfig { brain })
}

fn parse_setup_brain_config(table: &toml::Table) -> Result<SetupBrainConfig, String> {
    validate_setup_brain_fields(table)?;
    let artifact = ProviderImplementationRef {
        path: string_field(table, "path")?,
        crate_name: string_field(table, "crate")?,
        version: string_field(table, "version")?,
        binary: string_field(table, "binary")?,
        script: string_field(table, "script")?,
    };

    artifact.validate().map_err(|err| err.to_string())?;
    if artifact.crate_name.is_some() {
        return Err("setup brain provider reference: `crate` artifacts are not supported until setup-brain provider packaging is available".to_string());
    }

    Ok(SetupBrainConfig {
        artifact,
        settings_id: string_field(table, "settings_id")?,
    })
}

fn validate_setup_brain_fields(table: &toml::Table) -> Result<(), String> {
    for key in table.keys() {
        if !matches!(
            key.as_str(),
            "path" | "crate" | "version" | "binary" | "script" | "settings_id"
        ) {
            return Err(format!("unknown setup brain field `{key}`"));
        }
    }
    Ok(())
}

fn string_field(table: &toml::Table, key: &str) -> Result<Option<String>, String> {
    let Some(value) = table.get(key) else {
        return Ok(None);
    };
    value
        .as_str()
        .map(|value| Some(value.to_string()))
        .ok_or_else(|| format!("setup brain field `{key}` must be a string"))
}
