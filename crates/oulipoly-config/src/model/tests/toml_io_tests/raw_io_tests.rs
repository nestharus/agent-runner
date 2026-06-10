use crate::model::tests::*;
use std::path::PathBuf;

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
