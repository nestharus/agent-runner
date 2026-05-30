use oulipoly_config::PromptMode;
use serde_json::json;
use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &str) -> String {
    std::fs::read_to_string(workspace_root().join(path))
        .unwrap_or_else(|err| panic!("failed to read {path}: {err}"))
}

fn compact(source: &str) -> String {
    source.split_whitespace().collect::<String>()
}

fn function_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("missing function signature: {signature}"));
    let brace_start = source[start..]
        .find('{')
        .map(|offset| start + offset)
        .unwrap_or_else(|| panic!("missing function body for: {signature}"));
    let mut depth = 0usize;
    for (offset, ch) in source[brace_start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[brace_start..=brace_start + offset];
                }
            }
            _ => {}
        }
    }
    panic!("unterminated function body for: {signature}");
}

fn assert_in_order(haystack: &str, needles: &[&str]) {
    let mut cursor = 0usize;
    for needle in needles {
        let offset = haystack[cursor..]
            .find(needle)
            .unwrap_or_else(|| panic!("missing ordered snippet: {needle}"));
        cursor += offset + needle.len();
    }
}

// risk: Model summary IPC drift; level: backend DTO; source: AGE-235 L2 characterization inventory
#[test]
fn model_summary_serializes_stable_backend_dto_shape() {
    let summary = agent_runner_lib::ModelSummary {
        name: "codex~high".to_string(),
        prompt_mode: PromptMode::Stdin,
        provider_count: 2,
    };

    let value = serde_json::to_value(summary).unwrap();

    assert_eq!(
        value,
        json!({
            "name": "codex~high",
            "prompt_mode": "stdin",
            "provider_count": 2,
        })
    );
    let object = value.as_object().unwrap();
    assert_eq!(
        object.keys().cloned().collect::<Vec<_>>(),
        vec!["name", "prompt_mode", "provider_count"]
    );
}

// risk: Model list ordering and summary mapping drift; level: backend command contract; source: AGE-235 L2 characterization inventory
#[test]
fn list_models_maps_backend_summaries_and_sorts_by_model_name() {
    let orchestration = read("src/commands/models/orchestration.rs");
    let accessor = read("src/commands/models/accessor.rs");
    let command_body = compact(function_body(&orchestration, "fn list_models("));
    let helper_body = compact(function_body(&accessor, "fn model_summaries("));

    for required in [
        "letmodels=state.models.lock().map_err(|e|e.to_string())?;",
        "Ok(accessor::model_summaries(&models))",
    ] {
        assert!(
            command_body.contains(required),
            "list_models must preserve model cache access/delegation contract: {required}"
        );
    }

    for required in [
        ".values()",
        ".map(|m|ModelSummary{name:m.name.clone(),prompt_mode:m.prompt_mode,provider_count:m.providers.len(),})",
        "summaries.sort_by(|a,b|a.name.cmp(&b.name));",
        "summaries",
    ] {
        assert!(
            helper_body.contains(required),
            "list_models must preserve summary mapping/sort contract: {required}"
        );
    }
}

// risk: Model accessor error drift; level: backend command contract; source: AGE-235 L2 characterization inventory
#[test]
fn get_model_preserves_clone_success_path_and_exact_not_found_error() {
    let orchestration = read("src/commands/models/orchestration.rs");
    let accessor = read("src/commands/models/accessor.rs");
    let formatter = read("src/commands/models/formatter.rs");
    let body = compact(function_body(&orchestration, "fn get_model("));

    assert!(
        function_body(&accessor, "fn clone_model_by_name(").contains("models.get(name).cloned()"),
        "get_model must keep returning a cloned model from the in-memory cache"
    );
    assert!(
        body.contains("ok_or_else(||formatter::model_not_found_error(&name))"),
        "get_model must preserve byte-identical not-found template"
    );
    assert!(
        function_body(&formatter, "fn model_not_found_error(").contains("\"Model '{}' not found\""),
        "get_model must preserve exact not-found string literal"
    );
}

// risk: Model deletion side-effect drift; level: backend command contract; source: AGE-235 L2 characterization inventory
#[test]
fn delete_model_keeps_path_file_cache_and_provider_settings_side_effects() {
    let orchestration = read("src/commands/models/orchestration.rs");
    let formatter = read("src/commands/models/formatter.rs");
    let accessor = read("src/commands/models/accessor.rs");
    let raw_body = function_body(&orchestration, "fn delete_model(");
    let body = compact(raw_body);

    for required in [
        "letpath=formatter::model_file_path(&state.models_dir,&name);",
        "ifpath.exists(){",
        "std::fs::remove_file(&path).map_err(formatter::delete_model_file_error)?;",
        "accessor::remove_cached_model(&state,&name)?;",
        "provider_settings::refresh_provider_settings_host(&state)?;",
        "Ok(())",
    ] {
        assert!(
            body.contains(required),
            "delete_model must preserve deletion side-effect contract: {required}"
        );
    }

    assert!(
        function_body(&formatter, "fn delete_model_file_error(")
            .contains("\"Failed to delete model file: {error}\""),
        "delete_model must preserve exact delete IO error prefix"
    );
    assert!(
        function_body(&accessor, "fn remove_cached_model(").contains("models.remove(name);"),
        "delete_model must keep removing the model from the in-memory cache"
    );
    assert_in_order(
        &body,
        &[
            "ifpath.exists(){",
            "std::fs::remove_file(&path)",
            "accessor::remove_cached_model(&state,&name)?;",
            "provider_settings::refresh_provider_settings_host(&state)?;",
        ],
    );
}
