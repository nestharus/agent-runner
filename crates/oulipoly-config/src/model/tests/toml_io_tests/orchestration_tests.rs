use crate::model::tests::*;

#[test]
fn model_config_from_toml_orchestrates_parse_validate_construct() {
    let body = function_body(
        include_str!("../../toml_io/mod.rs"),
        "pub fn from_toml(\n        text: &str,\n        providers: Option<&crate::providers::ProvidersConfig>,\n    ) -> Result<Self, ModelError>",
    );

    assert_contains_in_order(
        body,
        &[
            "parse_model_toml",
            "validate_model_toml_against_providers",
            "construct_model_config_from_raw",
        ],
    );
    assert_forbidden_absent(
        body,
        &[
            "toml::from_str",
            "parse_inputs",
            "ProviderConfig {",
            "validate_interactive_args",
            "validate_resume_interactive_args",
        ],
    );
}

#[test]
fn render_validated_model_toml_orchestrates_emit_parse_validate() {
    let body = function_body(
        include_str!("../../toml_io/mod.rs"),
        "pub fn render_validated_model_toml(",
    );

    assert_contains_in_order(
        body,
        &[
            "emit_model_toml",
            "parse_model_toml",
            "validate_model_toml_against_providers",
        ],
    );
    assert_forbidden_absent(
        body,
        &[
            ".to_toml",
            "ModelConfig::from_toml",
            "validate_codex_model_arg_overlap",
            "toml::from_str",
        ],
    );
}

#[test]
fn load_models_orchestrates_read_parse_validate_construct() {
    let body = function_body(include_str!("../../toml_io/mod.rs"), "pub fn load_models(");

    assert_contains_in_order(
        body,
        &[
            "read_model_files",
            "parse_model_files",
            "validate_models_against_providers",
            "build_named_model_map",
        ],
    );
    assert_forbidden_absent(
        body,
        &[
            "fs::read_dir",
            "fs::read_to_string",
            "ModelConfig::from_toml",
            "validate_codex_model_arg_overlap",
            "models.insert",
        ],
    );
}

#[test]
fn parse_model_provider_flags_only() {
    let toml = r#"
[[providers]]
name = "claude2"
args = ["-p", "--model", "opus", "--output-format", "json"]
interactive_args = ["--model", "opus"]
"#;
    let config = ModelConfig::from_toml_with_name("claude-opus", toml, None).unwrap();
    let provider = &config.providers[0];

    assert_eq!(provider.name, "claude2");
    assert_eq!(provider.command, "");
    assert_eq!(
        provider.args,
        ["-p", "--model", "opus", "--output-format", "json"]
    );
    assert_eq!(
        provider.interactive_args.as_deref(),
        Some(&["--model".to_string(), "opus".to_string()][..])
    );
    assert!(provider.resume.is_none());
    assert!(provider.session_capture.is_none());
    assert!(provider.session_storage.is_none());
}

#[test]
fn model_roundtrip_keeps_model_provider_fields_only() {
    let original = r#"
[[providers]]
name = "claude"
args = ["-p", "--model", "sonnet"]

[[providers]]
name = "claude2"
args = ["-p", "--model", "opus"]
interactive_args = ["--model", "opus"]

[[inputs]]
name = "prompt"
type = "string"
default_input = true
"#;
    let c1 = ModelConfig::from_toml_with_name("claude-opus", original, None).unwrap();
    let rendered = c1.to_toml();
    assert!(!rendered.contains("command ="));
    assert!(!rendered.contains("prompt_mode"));
    assert!(!rendered.contains("session_storage"));

    let c2 = ModelConfig::from_toml_with_name("claude-opus", &rendered, None).unwrap();
    assert_eq!(c1.providers.len(), c2.providers.len());
    assert_eq!(
        c1.providers[1].interactive_args,
        c2.providers[1].interactive_args
    );
    assert_eq!(c1.inputs.len(), c2.inputs.len());
}
