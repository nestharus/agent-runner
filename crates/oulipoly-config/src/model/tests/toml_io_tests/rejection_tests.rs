use crate::model::tests::*;

#[test]
fn rejects_no_providers() {
    let toml = r#"
[[inputs]]
name = "prompt"
type = "string"
"#;
    assert!(ModelConfig::from_toml_with_name("test", toml, None).is_err());
}

#[test]
fn rejects_duplicate_default_input() {
    let toml = r#"
[[providers]]
name = "test"

[[inputs]]
name = "a"
type = "string"
default_input = true

[[inputs]]
name = "b"
type = "string"
default_input = true
"#;
    let result = ModelConfig::from_toml_with_name("test", toml, None);
    assert!(result.unwrap_err().contains("only one input"));
}

#[test]
fn rejects_enum_without_options() {
    let toml = r#"
[[providers]]
name = "test"

[[inputs]]
name = "format"
type = "enum"
"#;
    let result = ModelConfig::from_toml_with_name("test", toml, None);
    assert!(result.unwrap_err().contains("requires 'options'"));
}

#[test]
fn config_load_rejects_old_per_provider_blocks_in_model_toml() {
    let toml = r#"
[[providers]]
name = "claude2"
command = "env"
args = ["-u", "CLAUDECODE", "claude2", "-p", "--model", "opus"]
prompt_mode = "stdin"

[providers.resume]
kind = "flag"
flag = "--resume"
"#;
    let err = ModelConfig::from_toml_with_name("claude-opus", toml, None).unwrap_err();
    assert!(err.contains("Old per-provider config detected in claude-opus.toml"));
    assert!(err.contains("agents migrate-config"));
}

#[test]
fn model_toml_rejects_age28_provider_fields() {
    let toml = r#"
[[providers]]
name = "claude"
args = ["--model", "opus"]
system_prompt_override = "root-only policy must not live in model TOML"

[providers.tool_restrictions]
kind = "claude"

[providers.tool_restrictions.claude]
disallowed_tools = ["Task"]
"#;

    let err = ModelConfig::from_toml_with_name("claude-opus", toml, None).unwrap_err();

    assert!(err.contains("model claude-opus provider claude"), "{err}");
    assert!(err.contains("system_prompt_override"), "{err}");
    assert!(err.contains("root-only"), "{err}");
    assert!(err.contains("providers.toml"), "{err}");
}

#[test]
fn old_top_level_model_config_is_rejected() {
    let toml = r#"
command = "claude"
args = ["-p", "--model", "opus"]
prompt_mode = "stdin"
"#;
    let err = ModelConfig::from_toml_with_name("claude-opus", toml, None).unwrap_err();
    assert!(err.contains("agents migrate-config"));
}
