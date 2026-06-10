use crate::model::tests::*;

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
