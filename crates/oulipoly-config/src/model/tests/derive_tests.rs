use super::*;

#[test]
fn derive_provider_name_simple() {
    assert_eq!(derive_provider_name("claude", &[]), "claude");
}

#[test]
fn derive_provider_name_env_wrapper() {
    let args = vec![
        "-u".to_string(),
        "CLAUDECODE".to_string(),
        "claude2".to_string(),
        "-p".to_string(),
        "--model".to_string(),
        "opus".to_string(),
    ];
    assert_eq!(derive_provider_name("env", &args), "claude2");
}

#[test]
fn derive_provider_name_prefixed_command_string() {
    assert_eq!(
        derive_provider_name("env -u CODEX_ENV codex", &["exec".to_string()]),
        "codex"
    );
}

#[test]
fn derive_provider_name_env_assignment() {
    let args = vec!["FOO=bar".to_string(), "claude3".to_string()];
    assert_eq!(derive_provider_name("env", &args), "claude3");
}

#[test]
fn provider_config_auto_derives_name() {
    let p = ProviderConfig::new(
        "env",
        vec![
            "-u".to_string(),
            "CLAUDECODE".to_string(),
            "claude2".to_string(),
            "-p".to_string(),
        ],
    );
    assert_eq!(p.name, "claude2");
}

#[test]
fn provider_config_constructors_default_invocation_mode_to_headless() {
    let direct = ProviderConfig::new("claude", vec!["-p".to_string()]);
    let model = ProviderConfig::model_provider("claude", vec!["--model".to_string()]);

    assert_eq!(direct.invocation_mode, InvocationMode::Headless);
    assert_eq!(model.invocation_mode, InvocationMode::Headless);
}
