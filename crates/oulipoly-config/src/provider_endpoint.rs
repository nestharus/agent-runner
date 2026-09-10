use std::path::Path;

/// Explicit account-to-family provider implementation authority.
///
/// The executable is either absolute or relative to the directory containing
/// `providers.toml`. Native commands, models, and `PATH` are not candidates.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderEndpointConfig {
    pub family: String,
    pub executable: String,
}

impl ProviderEndpointConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.family.trim().is_empty() {
            return Err("provider implementation family must not be empty".to_string());
        }
        if self.executable.trim().is_empty() || Path::new(&self.executable).as_os_str().is_empty() {
            return Err("provider implementation executable must not be empty".to_string());
        }
        Ok(())
    }
}
