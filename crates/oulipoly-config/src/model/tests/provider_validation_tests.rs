use super::*;

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
