use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct AppConfig {
    pub diagnostics_model: Option<String>,
    pub default_provider: Option<String>,
}

impl AppConfig {
    pub fn load(_path: &Path) -> Result<Self, String> {
        Ok(Self::default())
    }
}
