use oulipoly_config::{ProviderImplementationFlavor, app::AppConfig};
use std::path::PathBuf;
use tempfile::TempDir;

#[test]
fn missing_setup_brain_parses_as_no_configured_setup_brain() {
    let (_tmp, path) = write_app_config(
        r#"
diagnostics_model = "neutral-diagnostics"
default_provider = "fixture-default"
"#,
    );

    let config = AppConfig::load(&path).expect("app config should load");

    assert!(
        config.setup.brain.is_none(),
        "missing [setup.brain] must keep setup brain absent so the no-config fallback is the only eligible legacy path"
    );
}

#[test]
fn valid_setup_brain_artifact_and_settings_id_parse() {
    let (_tmp, path) = write_app_config(
        r#"
[setup.brain]
binary = "fixture-setup-brain"
settings_id = "brain-default"
"#,
    );

    let config = AppConfig::load(&path).expect("app config should load");
    let brain = config
        .setup
        .brain
        .as_ref()
        .expect("setup brain should be configured");

    assert_eq!(
        brain.artifact.binary.as_deref(),
        Some("fixture-setup-brain")
    );
    assert_eq!(
        brain.artifact.flavor(),
        Ok(ProviderImplementationFlavor::Binary)
    );
    assert_eq!(brain.settings_id.as_deref(), Some("brain-default"));
}

#[test]
fn invalid_setup_brain_artifact_uses_provider_ref_validation() {
    let (_tmp, path) = write_app_config(
        r#"
[setup.brain]
binary = "fixture-setup-brain"
script = "fixtures/setup-brain.sh"
settings_id = "brain-default"
"#,
    );

    let error =
        AppConfig::load(&path).expect_err("multiple setup brain artifact flavors must fail");
    let message = error.to_string();

    assert!(
        message.contains("multiple flavors")
            && message.contains("binary")
            && message.contains("script"),
        "invalid setup brain artifact must reuse ProviderImplementationRef validation text, got: {message}"
    );
}

#[test]
fn setup_brain_crate_artifact_fails_until_packaging_is_available() {
    let (_tmp, path) = write_app_config(
        r#"
[setup.brain]
crate = "fixture-setup-brain"
version = "0.1.0"
"#,
    );

    let error = AppConfig::load(&path).expect_err("crate setup brain artifact must fail in S8");
    assert!(
        error
            .to_string()
            .contains("`crate` artifacts are not supported"),
        "{error}"
    );
}

#[test]
fn setup_brain_unknown_field_fails_instead_of_being_ignored() {
    let (_tmp, path) = write_app_config(
        r#"
[setup.brain]
binary = "fixture-setup-brain"
unexpected = "ignored would be unsafe"
"#,
    );

    let error = AppConfig::load(&path).expect_err("unknown setup brain field must fail");
    assert!(
        error.to_string().contains("unknown setup brain field"),
        "{error}"
    );
}

#[test]
fn setup_brain_non_string_field_fails_instead_of_being_dropped() {
    let (_tmp, path) = write_app_config(
        r#"
[setup.brain]
binary = "fixture-setup-brain"
settings_id = 42
"#,
    );

    let error = AppConfig::load(&path).expect_err("non-string setup brain field must fail");
    assert!(
        error
            .to_string()
            .contains("setup brain field `settings_id` must be a string"),
        "{error}"
    );
}

#[test]
fn setup_field_wrong_shape_fails_instead_of_being_absent() {
    let (_tmp, path) = write_app_config(
        r#"
setup = "fixture-not-a-table"
"#,
    );

    let error = AppConfig::load(&path).expect_err("non-table setup config must fail");
    assert!(
        error
            .to_string()
            .contains("setup config field `setup` must be a table"),
        "{error}"
    );
}

#[test]
fn setup_brain_wrong_shape_fails_instead_of_being_absent() {
    let (_tmp, path) = write_app_config(
        r#"
[setup]
brain = "fixture-not-a-table"
"#,
    );

    let error = AppConfig::load(&path).expect_err("non-table setup brain config must fail");
    assert!(
        error
            .to_string()
            .contains("setup config field `setup.brain` must be a table"),
        "{error}"
    );
}

#[test]
fn malformed_app_config_fails_instead_of_being_absent() {
    let (_tmp, path) = write_app_config(
        r#"
[setup.brain
binary = "fixture-setup-brain"
"#,
    );

    let error = AppConfig::load(&path).expect_err("malformed app config must fail");
    assert!(
        error.to_string().contains("failed to parse app config"),
        "{error}"
    );
}

fn write_app_config(content: &str) -> (TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("app.toml");
    std::fs::write(&path, content).expect("write app config");
    (tmp, path)
}
