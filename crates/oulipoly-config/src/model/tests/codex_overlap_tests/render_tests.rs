use crate::model::tests::*;

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

    let rendered = render_validated_model_toml(&model, Some(&providers)).unwrap();
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

    let err = render_validated_model_toml(&model, Some(&providers)).unwrap_err();

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

    let err = render_validated_model_toml(&model, Some(&providers)).unwrap_err();

    assert!(
        err.contains("duplicates root [codex].interactive_args"),
        "{err}"
    );
}

#[test]
fn render_validated_model_toml_without_providers_bypasses_overlap_check() {
    let model = test_model("codex", &["exec", "-m", "gpt-5.5"]);

    let rendered = render_validated_model_toml(&model, None).unwrap();
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
