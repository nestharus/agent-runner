use agent_runner_config::{AppConfig, load_app_config_from_path};
use std::fs;

/// Risk: D-T2 (load_app_config parses default_model)
/// Source: D-agent-binary contract §7
/// Level: unit
/// Fixture source: inline tempfile config.toml
#[test]
fn load_app_config_parses_default_model() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    fs::write(
        &config_path,
        r#"
diagnostics_model = "diag-fixture"
default_model = "fixture-cli"
"#,
    )
    .unwrap();

    let config: AppConfig = load_app_config_from_path(&config_path).unwrap();

    assert_eq!(config.diagnostics_model.as_deref(), Some("diag-fixture"));
    assert_eq!(config.default_model.as_deref(), Some("fixture-cli"));
}

/// Risk: D-T2 (load_app_config treats absent default_model as None)
/// Source: D-agent-binary contract §7
/// Level: unit
/// Fixture source: inline tempfile config.toml
#[test]
fn load_app_config_missing_default_model_is_none() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    fs::write(&config_path, r#"diagnostics_model = "diag-fixture""#).unwrap();

    let config: AppConfig = load_app_config_from_path(&config_path).unwrap();

    assert_eq!(config.diagnostics_model.as_deref(), Some("diag-fixture"));
    assert_eq!(config.default_model, None);
}
