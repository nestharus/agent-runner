//! Default agent-runner data path resolution.

use std::path::PathBuf;

pub const APP_DATA_DIR_NAME: &str = "oulipoly-agent-runner";
pub const DATA_DIR_ENV: &str = "OULIPOLY_DATA_DIR";

pub fn data_dir() -> Result<PathBuf, String> {
    std::env::var_os(DATA_DIR_ENV)
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(default_data_dir)
}

fn default_data_dir() -> Result<PathBuf, String> {
    let data_dir =
        dirs::data_dir().ok_or_else(|| "Could not determine data directory".to_string())?;
    Ok(data_dir.join(APP_DATA_DIR_NAME))
}
