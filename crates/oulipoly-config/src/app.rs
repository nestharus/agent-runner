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
        let Some(content) = read_app_config_content(path) else {
            return Ok(Self::default());
        };

        let table = parse_app_config_table(&content)
            .map_err(|error| format_app_config_parse_error(path, error))?;

        map_app_config(&table)
    }
}

fn read_app_config_content(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn parse_app_config_table(content: &str) -> Result<toml::Table, toml::de::Error> {
    content.parse::<toml::Table>()
}

fn format_app_config_parse_error(path: &Path, error: toml::de::Error) -> String {
    format!("failed to parse app config at {}: {error}", path.display())
}

fn map_app_config(table: &toml::Table) -> Result<AppConfig, String> {
    Ok(AppConfig {
        diagnostics_model: optional_app_string(table, "diagnostics_model"),
        default_provider: optional_app_string(table, "default_provider"),
        setup: parse_setup_config(table)?,
    })
}

fn optional_app_string(table: &toml::Table, key: &str) -> Option<String> {
    table
        .get(key)
        .and_then(|value| value.as_str())
        .map(String::from)
}

fn parse_setup_config(table: &toml::Table) -> Result<SetupConfig, String> {
    let setup_table = validate_setup_table(setup_value(table))?;
    let brain_table = validate_setup_brain_table(setup_table.and_then(setup_brain_value))?;

    map_setup_config(brain_table)
}

fn setup_value(table: &toml::Table) -> Option<&toml::Value> {
    table.get("setup")
}

fn validate_setup_table(value: Option<&toml::Value>) -> Result<Option<&toml::Table>, String> {
    value
        .map(|value| {
            value
                .as_table()
                .ok_or_else(|| "setup config field `setup` must be a table".to_string())
        })
        .transpose()
}

fn setup_brain_value(table: &toml::Table) -> Option<&toml::Value> {
    table.get("brain")
}

fn validate_setup_brain_table(value: Option<&toml::Value>) -> Result<Option<&toml::Table>, String> {
    value
        .map(|value| {
            value
                .as_table()
                .ok_or_else(|| "setup config field `setup.brain` must be a table".to_string())
        })
        .transpose()
}

fn map_setup_config(brain_table: Option<&toml::Table>) -> Result<SetupConfig, String> {
    Ok(SetupConfig {
        brain: brain_table.map(parse_setup_brain_config).transpose()?,
    })
}

fn parse_setup_brain_config(table: &toml::Table) -> Result<SetupBrainConfig, String> {
    validate_setup_brain_fields(table)?;
    validate_setup_brain_artifact_string_fields(table)?;
    let artifact = map_setup_brain_artifact(table);
    validate_setup_brain_artifact(&artifact)?;
    validate_setup_brain_settings_id_field(table)?;

    Ok(map_setup_brain_config(table, artifact))
}

fn map_setup_brain_artifact(table: &toml::Table) -> ProviderImplementationRef {
    ProviderImplementationRef {
        path: setup_brain_string(table, "path"),
        crate_name: setup_brain_string(table, "crate"),
        version: setup_brain_string(table, "version"),
        binary: setup_brain_string(table, "binary"),
        script: setup_brain_string(table, "script"),
    }
}

fn validate_setup_brain_artifact(artifact: &ProviderImplementationRef) -> Result<(), String> {
    artifact.validate().map_err(|err| err.to_string())?;
    if artifact.crate_name.is_some() {
        return Err("setup brain provider reference: `crate` artifacts are not supported until setup-brain provider packaging is available".to_string());
    }

    Ok(())
}

fn map_setup_brain_config(
    table: &toml::Table,
    artifact: ProviderImplementationRef,
) -> SetupBrainConfig {
    SetupBrainConfig {
        artifact,
        settings_id: setup_brain_string(table, "settings_id"),
    }
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

fn validate_setup_brain_artifact_string_fields(table: &toml::Table) -> Result<(), String> {
    validate_setup_brain_string_field(table, "path")?;
    validate_setup_brain_string_field(table, "crate")?;
    validate_setup_brain_string_field(table, "version")?;
    validate_setup_brain_string_field(table, "binary")?;
    validate_setup_brain_string_field(table, "script")
}

fn validate_setup_brain_settings_id_field(table: &toml::Table) -> Result<(), String> {
    validate_setup_brain_string_field(table, "settings_id")
}

fn validate_setup_brain_string_field(table: &toml::Table, key: &str) -> Result<(), String> {
    let Some(value) = table.get(key) else {
        return Ok(());
    };
    value
        .as_str()
        .map(|_| ())
        .ok_or_else(|| format!("setup brain field `{key}` must be a string"))
}

fn setup_brain_string(table: &toml::Table, key: &str) -> Option<String> {
    table
        .get(key)
        .and_then(|value| value.as_str())
        .map(String::from)
}
