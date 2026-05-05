use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct AppConfig {
    pub diagnostics_model: Option<String>,
    pub default_provider: Option<String>,
}

impl AppConfig {
    pub fn load(path: &Path) -> Result<Self, String> {
        let Ok(content) = std::fs::read_to_string(path) else {
            return Ok(Self::default());
        };

        let Ok(table) = content.parse::<toml::Table>() else {
            return Ok(Self::default());
        };

        Ok(Self {
            diagnostics_model: table
                .get("diagnostics_model")
                .and_then(|value| value.as_str())
                .map(String::from),
            default_provider: table
                .get("default_provider")
                .and_then(|value| value.as_str())
                .map(String::from),
        })
    }
}
