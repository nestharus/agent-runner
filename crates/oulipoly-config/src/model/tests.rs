use crate::providers::{ProviderEntry, ProvidersConfig};

use super::*;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn function_body<'a>(source: &'a str, needle: &str) -> &'a str {
    let start = source.find(needle).expect("function signature exists");
    let brace_start = source[start..].find('{').expect("function body starts") + start;
    let mut depth = 0usize;
    for (offset, ch) in source[brace_start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[brace_start + 1..brace_start + offset];
                }
            }
            _ => {}
        }
    }
    panic!("function body ends");
}

fn assert_contains_in_order(body: &str, expected: &[&str]) {
    let mut cursor = 0usize;
    for token in expected {
        let found = body[cursor..]
            .find(token)
            .unwrap_or_else(|| panic!("missing orchestration call {token} in {body}"));
        cursor += found + token.len();
    }
}

fn assert_forbidden_absent(body: &str, forbidden: &[&str]) {
    for token in forbidden {
        assert!(
            !body.contains(token),
            "orchestration shell must not contain inline logic token {token}: {body}"
        );
    }
}

fn write_providers_toml(root: &Path, body: &str) {
    fs::write(root.join("providers.toml"), body).unwrap();
}

fn write_model_toml(models_dir: &Path, name: &str, body: &str) {
    fs::create_dir_all(models_dir).unwrap();
    fs::write(models_dir.join(format!("{name}.toml")), body).unwrap();
}

fn load_temp_models(
    providers_toml: &str,
    model_name: &str,
    model_toml: &str,
) -> Result<HashMap<String, ModelConfig>, String> {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let models_dir = root.join("models");
    write_providers_toml(root, providers_toml);
    write_model_toml(&models_dir, model_name, model_toml);

    let providers = ProvidersConfig::load(&root.join("providers.toml")).unwrap();
    // AGE-40 Step 6c adds the `load_models(&Path, Option<&ProvidersConfig>)` signature.
    Ok(load_models(&models_dir, Some(&providers))?)
}

fn test_model(provider_name: &str, args: &[&str]) -> ModelConfig {
    ModelConfig {
        name: "gpt-high".into(),
        prompt_mode: PromptMode::Stdin,
        providers: vec![ProviderConfig::model_provider(
            provider_name,
            args.iter().map(|arg| (*arg).to_string()).collect(),
        )],
        inputs: vec![],
        provider: None,
    }
}

fn test_providers(provider_name: &str, args: &[&str]) -> ProvidersConfig {
    let mut entries = HashMap::new();
    entries.insert(
        provider_name.to_string(),
        ProviderEntry {
            args: args.iter().map(|arg| (*arg).to_string()).collect(),
            ..ProviderEntry::default()
        },
    );
    ProvidersConfig { entries }
}

fn codex_providers(args: &[&str], interactive_args: Option<&[&str]>) -> ProvidersConfig {
    let mut entries = HashMap::new();
    entries.insert(
        "codex".to_string(),
        ProviderEntry {
            args: args.iter().map(|arg| (*arg).to_string()).collect(),
            interactive_args: interactive_args
                .map(|args| args.iter().map(|arg| (*arg).to_string()).collect()),
            ..ProviderEntry::default()
        },
    );
    ProvidersConfig { entries }
}

fn codex_providers_with_typed_policy(config_pairs: &[&str]) -> ProvidersConfig {
    let mut entries = HashMap::new();
    entries.insert(
        "codex".to_string(),
        ProviderEntry {
            command: Some("codex".to_string()),
            args: vec!["exec".to_string()],
            tool_restrictions: Some(ToolRestrictions {
                kind: ToolRestrictionKind::Codex,
                claude: ClaudeRestrictions::default(),
                codex: CodexRestrictions {
                    config_pairs: config_pairs
                        .iter()
                        .map(|pair| (*pair).to_string())
                        .collect(),
                    disabled_features: Vec::new(),
                },
            }),
            ..ProviderEntry::default()
        },
    );
    ProvidersConfig { entries }
}

#[test]
fn model_config_from_toml_orchestrates_parse_validate_construct() {
    let body = function_body(
        include_str!("loader.rs"),
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
        include_str!("loader.rs"),
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
    let body = function_body(include_str!("loader.rs"), "pub fn load_models(");

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

#[test]
fn load_models_rejects_codex_dangerously_flag_overlap() {
    let err = load_temp_models(
        r#"
[codex]
command = "codex"
args = ["exec", "--dangerously-bypass-approvals-and-sandbox"]
"#,
        "gpt-high",
        r#"
[[providers]]
name = "codex"
args = ["--dangerously-bypass-approvals-and-sandbox", "-m", "gpt-5.5"]
"#,
    )
    .unwrap_err();

    assert!(err.contains("--dangerously-bypass-approvals-and-sandbox"));
}

#[test]
fn load_models_accepts_canonical_codex_split() {
    let models = load_temp_models(
        r#"
[codex]
command = "codex"
args = ["exec", "--dangerously-bypass-approvals-and-sandbox"]
"#,
        "gpt-high",
        r#"
[[providers]]
name = "codex"
args = ["-m", "gpt-5.5"]
"#,
    )
    .unwrap();

    assert_eq!(
        models["gpt-high"].providers[0].args,
        vec!["-m".to_string(), "gpt-5.5".to_string()]
    );
}

#[test]
fn render_validated_model_toml_accepts_clean_codex_model_with_providers() {
    let providers = codex_providers(&["exec", "-c", "sandbox=workspace-write"], None);
    let model = test_model("codex", &["-m", "gpt-5.5"]);

    let rendered = super::render_validated_model_toml(&model, Some(&providers)).unwrap();
    let reparsed = ModelConfig::from_toml_with_name("gpt-high", &rendered, None).unwrap();

    assert_eq!(
        reparsed.providers[0].args,
        vec!["-m".to_string(), "gpt-5.5".to_string()]
    );
}

#[test]
fn render_validated_model_toml_rejects_duplicate_codex_args() {
    let providers = codex_providers(&["exec", "-c", "sandbox=workspace-write"], None);
    let model = test_model("codex", &["exec", "-m", "gpt-5.5"]);

    let err = super::render_validated_model_toml(&model, Some(&providers)).unwrap_err();

    assert!(err.contains("duplicates root [codex].args"), "{err}");
}

#[test]
fn render_validated_model_toml_rejects_duplicate_codex_interactive_args() {
    let providers = codex_providers(
        &["exec"],
        Some(&["exec", "--dangerously-bypass-approvals-and-sandbox"]),
    );
    let mut model = test_model("codex", &["-m", "gpt-5.5"]);
    model.providers[0].interactive_args = Some(vec![
        "--dangerously-bypass-approvals-and-sandbox".to_string(),
    ]);

    let err = super::render_validated_model_toml(&model, Some(&providers)).unwrap_err();

    assert!(
        err.contains("duplicates root [codex].interactive_args"),
        "{err}"
    );
}

#[test]
fn render_validated_model_toml_without_providers_bypasses_overlap_check() {
    let model = test_model("codex", &["exec", "-m", "gpt-5.5"]);

    let rendered = super::render_validated_model_toml(&model, None).unwrap();
    let reparsed = ModelConfig::from_toml_with_name("gpt-high", &rendered, None).unwrap();

    assert_eq!(reparsed.providers[0].args[0], "exec");
}

#[test]
fn load_models_accepts_age29_migrated_c_split() {
    load_temp_models(
        r#"
[codex]
command = "codex"
args = ["exec", "-c", "sandbox=workspace-write"]
"#,
        "gpt-high",
        r#"
[[providers]]
name = "codex"
args = ["-c", "model_reasoning_effort=high"]
"#,
    )
    .unwrap();
}

#[test]
fn load_models_rejects_duplicate_c_pair() {
    let err = load_temp_models(
        r#"
[codex]
command = "codex"
args = ["exec", "-c", "sandbox=workspace-write"]
"#,
        "gpt-high",
        r#"
[[providers]]
name = "codex"
args = ["-c", "sandbox=workspace-write"]
"#,
    )
    .unwrap_err();

    assert!(err.contains("(-c, sandbox=workspace-write)"));
    assert!(err.contains("[codex].args"));
}

#[test]
fn codex_overlap_guard_blocks_typed_policy_vs_model_arg_collision() {
    let providers = codex_providers_with_typed_policy(&["model_reasoning_effort=high"]);
    let model = test_model(
        "codex",
        &["-m", "gpt-5.5", "-c", "model_reasoning_effort=high"],
    );

    let err = validate_codex_model_arg_overlap("gpt-high", &model, &providers).unwrap_err();

    assert!(err.contains("gpt-high"), "{err}");
    assert!(err.contains("codex"), "{err}");
    assert!(err.contains("typed-policy"), "{err}");
    assert!(err.contains("model_reasoning_effort=high"), "{err}");
}

#[test]
fn codex_overlap_guard_blocks_typed_policy_key_override() {
    let providers = codex_providers_with_typed_policy(&["sandbox=read-only"]);
    let model = test_model("codex", &["-m", "gpt-5.5", "-c", "sandbox=workspace-write"]);

    let err = validate_codex_model_arg_overlap("gpt-high", &model, &providers).unwrap_err();

    assert!(err.contains("typed-policy"), "{err}");
    assert!(err.contains("sandbox=read-only"), "{err}");
}

#[test]
fn codex_overlap_guard_does_not_regress_model_reasoning_effort() {
    let empty_policy = codex_providers_with_typed_policy(&[]);
    let model = test_model(
        "codex",
        &["-m", "gpt-5.5", "-c", "model_reasoning_effort=high"],
    );
    validate_codex_model_arg_overlap("gpt-high", &model, &empty_policy).unwrap();

    let unrelated_policy = codex_providers_with_typed_policy(&["other_key=val"]);
    validate_codex_model_arg_overlap("gpt-high", &model, &unrelated_policy).unwrap();

    load_temp_models(
        r#"
[codex]
command = "codex"
args = ["exec", "-c", "sandbox=workspace-write"]
"#,
        "gpt-high",
        r#"
[[providers]]
name = "codex"
args = ["-c", "model_reasoning_effort=high"]
"#,
    )
    .unwrap();
}

#[test]
fn codex_overlap_guard_characterizes_sibling_alias_policy() {
    let mut entries = HashMap::new();
    entries.insert(
        "codex2".to_string(),
        ProviderEntry {
            command: Some("codex2".to_string()),
            args: vec!["exec".to_string()],
            tool_restrictions: Some(ToolRestrictions {
                kind: ToolRestrictionKind::Codex,
                claude: ClaudeRestrictions::default(),
                codex: CodexRestrictions {
                    config_pairs: vec!["model_reasoning_effort=high".to_string()],
                    disabled_features: Vec::new(),
                },
            }),
            ..ProviderEntry::default()
        },
    );
    let providers = ProvidersConfig { entries };
    let model = test_model(
        "codex2",
        &["-m", "gpt-5.5", "-c", "model_reasoning_effort=high"],
    );

    let err = validate_codex_model_arg_overlap("gpt-high", &model, &providers).unwrap_err();

    assert!(err.contains("codex2"), "{err}");
    assert!(err.contains("typed-policy"), "{err}");
    assert!(err.contains("model_reasoning_effort=high"), "{err}");
}

#[test]
fn codex_overlap_guard_uses_prefixed_root_command_string() {
    let mut entries = HashMap::new();
    entries.insert(
        "primary".to_string(),
        ProviderEntry {
            command: Some("env -u CODEX_ENV codex".to_string()),
            args: vec!["exec".to_string()],
            tool_restrictions: Some(ToolRestrictions {
                kind: ToolRestrictionKind::Codex,
                claude: ClaudeRestrictions::default(),
                codex: CodexRestrictions {
                    config_pairs: vec!["model_reasoning_effort=high".to_string()],
                    disabled_features: Vec::new(),
                },
            }),
            ..ProviderEntry::default()
        },
    );
    let providers = ProvidersConfig { entries };
    let model = test_model(
        "primary",
        &["-m", "gpt-5.5", "-c", "model_reasoning_effort=high"],
    );

    let err = validate_codex_model_arg_overlap("gpt-high", &model, &providers).unwrap_err();

    assert!(err.contains("primary"), "{err}");
    assert!(err.contains("typed-policy"), "{err}");
    assert!(err.contains("model_reasoning_effort=high"), "{err}");
}

#[test]
fn split_codex_arg_parts_groups_c_and_config_pairs() {
    assert_eq!(
        split_codex_arg_parts(&[
            "exec".to_string(),
            "-c".to_string(),
            "sandbox=workspace-write".to_string(),
            "--config".to_string(),
            "model_reasoning_effort=high".to_string(),
        ]),
        vec![
            CodexArgPart::Standalone("exec".to_string()),
            CodexArgPart::Pair {
                flag: "-c".to_string(),
                value: "sandbox=workspace-write".to_string(),
            },
            CodexArgPart::Pair {
                flag: "--config".to_string(),
                value: "model_reasoning_effort=high".to_string(),
            },
        ]
    );

    assert_eq!(
        codex_arg_overlap(
            &[
                "exec".to_string(),
                "--config".to_string(),
                "a=1".to_string()
            ],
            &["--config".to_string(), "a=1".to_string()]
        ),
        Some("(--config, a=1)".to_string())
    );
    assert_eq!(
        codex_arg_overlap(
            &[
                "exec".to_string(),
                "--config".to_string(),
                "a=1".to_string()
            ],
            &["--config".to_string(), "b=2".to_string()]
        ),
        None
    );
}

#[test]
fn load_models_accepts_age29_migrated_config_split() {
    load_temp_models(
        r#"
[codex]
command = "codex"
args = ["exec", "--config", "sandbox=workspace-write"]
"#,
        "gpt-high",
        r#"
[[providers]]
name = "codex"
args = ["--config", "model_reasoning_effort=high"]
"#,
    )
    .unwrap();
}

#[test]
fn load_models_rejects_duplicate_config_pair() {
    let err = load_temp_models(
        r#"
[codex]
command = "codex"
args = ["exec", "--config", "sandbox=workspace-write"]
"#,
        "gpt-high",
        r#"
[[providers]]
name = "codex"
args = ["--config", "sandbox=workspace-write"]
"#,
    )
    .unwrap_err();

    assert!(err.contains("(--config, sandbox=workspace-write)"));
    assert!(err.contains("[codex].args"));
}

#[test]
fn load_models_ignores_codex_root_overlap_for_non_codex_model_provider() {
    load_temp_models(
        r#"
[codex]
command = "codex"
args = ["exec", "--config", "sandbox=workspace-write"]
"#,
        "claude-sonnet",
        r#"
[[providers]]
name = "claude"
args = ["exec", "--config", "sandbox=workspace-write"]
"#,
    )
    .unwrap();
}

#[test]
fn validator_error_message_names_token_and_repair_path() {
    let err = load_temp_models(
        r#"
[codex]
command = "codex"
args = ["exec", "--dangerously-bypass-approvals-and-sandbox"]
"#,
        "gpt-high",
        r#"
[[providers]]
name = "codex"
args = ["--dangerously-bypass-approvals-and-sandbox", "-m", "gpt-5.5"]
"#,
    )
    .unwrap_err();

    assert!(err.contains("gpt-high"));
    assert!(err.contains("codex"));
    assert!(err.contains("args"));
    assert!(err.contains("--dangerously-bypass-approvals-and-sandbox"));
    assert!(err.contains("[codex].args"));
    assert!(err.contains("agents migrate-config"));
}

#[test]
fn load_models_rejects_codex_interactive_args_overlap() {
    let err = load_temp_models(
        r#"
[codex]
command = "codex"
interactive_args = ["exec", "--dangerously-bypass-approvals-and-sandbox"]
"#,
        "gpt-high",
        r#"
[[providers]]
name = "codex"
args = ["-m", "gpt-5.5"]
interactive_args = ["--dangerously-bypass-approvals-and-sandbox"]
"#,
    )
    .unwrap_err();

    assert!(err.contains("interactive_args"));
    assert!(err.contains("[codex].interactive_args"));
}

#[test]
fn overlap_predicate_boundary_table() {
    let cases = [
        (
            "exact standalone duplicate",
            "codex",
            vec!["foo", "bar"],
            vec!["bar"],
            false,
        ),
        (
            "non-equal substring",
            "codex",
            vec!["foobar"],
            vec!["foo"],
            true,
        ),
        (
            "different -c key=value",
            "codex",
            vec!["-c", "a=1"],
            vec!["-c", "b=2"],
            true,
        ),
        (
            "identical -c key=value",
            "codex",
            vec!["-c", "a=1"],
            vec!["-c", "a=1"],
            false,
        ),
        (
            "different provider name",
            "claude",
            vec!["foo"],
            vec!["foo"],
            true,
        ),
        (
            "non-codex provider",
            "gemini",
            vec!["foo"],
            vec!["foo"],
            true,
        ),
    ];

    for (name, provider_name, root_args, model_args, expect_ok) in cases {
        let providers = test_providers(provider_name, &root_args);
        let model = test_model(provider_name, &model_args);

        // AGE-40 Step 6c adds `validate_codex_model_arg_overlap`.
        let result = validate_codex_model_arg_overlap("gpt-high", &model, &providers);

        assert_eq!(
            result.is_ok(),
            expect_ok,
            "{name}: expected ok={expect_ok}, got {result:?}"
        );
    }
}

#[test]
fn provider_config_constructors_default_invocation_mode_to_headless() {
    let direct = ProviderConfig::new("claude", vec!["-p".to_string()]);
    let model = ProviderConfig::model_provider("claude", vec!["--model".to_string()]);

    assert_eq!(direct.invocation_mode, InvocationMode::Headless);
    assert_eq!(model.invocation_mode, InvocationMode::Headless);
}

#[test]
fn model_provider_rejects_invocation_mode_as_root_only() {
    let err = ModelConfig::from_toml_with_name(
        "claude-opus",
        r#"
[[providers]]
name = "claude"
args = ["--model", "opus"]
invocation_mode = "proxy"
"#,
        None,
    )
    .unwrap_err();

    match err {
        ModelError::InvocationModeIsRootOnly { model, provider } => {
            assert_eq!(model, "claude-opus");
            assert_eq!(provider, "claude");
        }
        other => panic!("expected ModelError::InvocationModeIsRootOnly, got {other:?}"),
    }
}

#[test]
fn load_models_rejects_proxy_claude_tools_mcp_filter_from_model_args_when_effective() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let models_dir = root.join("models");
    write_providers_toml(
        root,
        r#"
[claude]
command = "claude"
invocation_mode = "proxy"
"#,
    );
    write_model_toml(
        &models_dir,
        "claude-opus",
        r#"
[[providers]]
name = "claude"
args = ["--tools", "mcp__age104p2__Task"]
"#,
    );
    let providers = ProvidersConfig::load(&root.join("providers.toml")).unwrap();

    let err = load_models(&models_dir, Some(&providers)).unwrap_err();

    assert!(err.contains("claude-opus"), "{err}");
    assert!(err.contains("proxy-mode Claude"), "{err}");
    assert!(err.contains("--tools mcp__"), "{err}");
}

#[test]
fn render_validated_model_toml_omits_root_only_invocation_mode() {
    let mut model = test_model("claude", &["--model", "opus"]);
    model.providers[0].invocation_mode = InvocationMode::Proxy;

    let rendered = render_validated_model_toml(&model, None).unwrap();

    assert!(!rendered.contains("invocation_mode"), "{rendered}");
    let reparsed = ModelConfig::from_toml_with_name("claude-opus", &rendered, None).unwrap();
    assert_eq!(
        reparsed.providers[0].invocation_mode,
        InvocationMode::Headless
    );
}

#[test]
fn model_roundtrip_rejects_provider_invocation_mode_as_root_only() {
    let rendered = r#"
[[providers]]
name = "claude"
args = ["--model", "opus"]
invocation_mode = "proxy"
"#;

    let err = ModelConfig::from_toml_with_name("claude-opus", rendered, None).unwrap_err();

    match err {
        ModelError::InvocationModeIsRootOnly { model, provider } => {
            assert_eq!(model, "claude-opus");
            assert_eq!(provider, "claude");
        }
        other => panic!("expected ModelError::InvocationModeIsRootOnly, got {other:?}"),
    }
}

#[test]
fn parse_model_toml_returns_raw_struct_for_valid_text() {
    let raw = parse_model_toml(
        r#"
[[providers]]
name = "claude"
args = ["--model", "opus"]
"#,
    )
    .unwrap();

    assert_eq!(raw.providers.unwrap().len(), 1);
}

#[test]
fn parse_model_toml_returns_err_for_malformed_text() {
    let err = parse_model_toml("not = [").unwrap_err();

    match err {
        ModelError::Toml(model, message) => {
            assert_eq!(model, "<unknown>");
            assert!(!message.is_empty());
        }
        other => panic!("expected ModelError::Toml, got {other:?}"),
    }
}

#[test]
fn validate_model_toml_against_providers_rejects_root_only_mode() {
    let raw = parse_model_toml(
        r#"
[[providers]]
name = "claude"
invocation_mode = "proxy"
"#,
    )
    .unwrap();

    let err = validate_model_toml_against_providers(&raw, None).unwrap_err();

    match err {
        ModelError::InvocationModeIsRootOnly { model, provider } => {
            assert_eq!(model, "<unknown>");
            assert_eq!(provider, "claude");
        }
        other => panic!("expected ModelError::InvocationModeIsRootOnly, got {other:?}"),
    }
}

#[test]
fn validate_model_toml_against_providers_passes_for_legal_model() {
    let raw = parse_model_toml(
        r#"
[[providers]]
name = "claude"
args = ["--model", "opus"]
"#,
    )
    .unwrap();

    assert_eq!(validate_model_toml_against_providers(&raw, None), Ok(()));
}

#[test]
fn construct_model_config_from_raw_preserves_legal_fields() {
    let raw = parse_model_toml(
        r#"
[[providers]]
name = "claude"
args = ["--model", "opus"]
"#,
    )
    .unwrap();

    let model = construct_model_config_from_raw(raw);

    assert_eq!(model.providers[0].name, "claude");
    assert_eq!(model.providers[0].args, ["--model", "opus"]);
    assert_eq!(model.providers[0].invocation_mode, InvocationMode::Headless);
}

#[test]
fn emit_model_toml_omits_invocation_mode_per_providers_block() {
    let mut model = test_model("claude", &["--model", "opus"]);
    model.providers[0].invocation_mode = InvocationMode::Proxy;

    let rendered = emit_model_toml(&model);

    assert!(rendered.contains("[[providers]]"), "{rendered}");
    assert!(!rendered.contains("invocation_mode"), "{rendered}");
}

#[test]
fn read_model_files_returns_paths_and_text_pairs() {
    let temp = TempDir::new().unwrap();
    let models_dir = temp.path().join("models");
    write_model_toml(
        &models_dir,
        "claude-opus",
        r#"
[[providers]]
name = "claude"
"#,
    );

    let files = read_model_files(&models_dir).unwrap();

    assert_eq!(files.len(), 1);
    assert!(files[0].0.ends_with("claude-opus.toml"));
    assert!(files[0].1.contains("[[providers]]"));
}

#[test]
fn parse_model_files_returns_paths_and_raw_pairs() {
    let path = PathBuf::from("claude-opus.toml");
    let parsed = parse_model_files(vec![(
        path.clone(),
        r#"
[[providers]]
name = "claude"
"#
        .to_string(),
    )])
    .unwrap();

    assert_eq!(parsed[0].0, path);
    assert_eq!(parsed[0].1.providers.as_ref().unwrap().len(), 1);
}

#[test]
fn validate_models_against_providers_rejects_unsafe_proxy_claude_shape_via_effective_merge() {
    let temp = TempDir::new().unwrap();
    write_providers_toml(
        temp.path(),
        r#"
[claude]
command = "claude"
invocation_mode = "proxy"
"#,
    );
    let providers = ProvidersConfig::load(&temp.path().join("providers.toml")).unwrap();
    let raw = parse_model_toml(
        r#"
[[providers]]
name = "claude"
args = ["--tools", "mcp__age104p2__Task"]
"#,
    )
    .unwrap();
    let raws = vec![(PathBuf::from("claude-opus.toml"), raw)];

    let err = validate_models_against_providers(&raws, Some(&providers)).unwrap_err();

    assert!(err.contains("proxy-mode Claude"), "{err}");
    assert!(err.contains("--tools mcp__"), "{err}");
}
