use crate::model::tests::*;

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
