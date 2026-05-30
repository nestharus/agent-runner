//! ## Declared roles
//!
//! `parser`, `accessor`, `formatter`, `validator`

use std::fs;
use std::path::Path;

fn source_block_after(source: &str, needle: &str) -> String {
    let start = source
        .find(needle)
        .unwrap_or_else(|| panic!("missing {needle}"));
    let open = source[start..]
        .find('{')
        .map(|idx| start + idx)
        .unwrap_or_else(|| panic!("missing opening brace for {needle}"));
    let mut depth = 0usize;
    for (offset, ch) in source[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return source[open..=open + offset].to_string();
                }
            }
            _ => {}
        }
    }
    panic!("missing closing brace for {needle}");
}

fn source_file(relative_path: &str) -> String {
    let source_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative_path);
    fs::read_to_string(&source_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", source_path.display()))
}

fn test_model_surface() -> String {
    [
        "src/commands/test_model/orchestration.rs",
        "src/commands/test_model/lookup.rs",
        "src/commands/test_model/dispatch.rs",
        "src/commands/test_model/validator.rs",
        "src/commands/test_model/formatter.rs",
        "src/commands/test_model/mapper.rs",
    ]
    .into_iter()
    .map(source_file)
    .collect::<Vec<_>>()
    .join("\n")
}

#[test]
fn age38_test_model_with_db_path_routes_executor_effective_request_through_service() {
    let source = test_model_surface();
    let body = source_block_after(&source, "fn build_effective_executor_request(");
    let dispatch = source_block_after(&source, "fn execute_effective_request(");

    assert!(
        body.contains("ExecutorServiceRequest::Effective"),
        "test_model_with_db_path must build an ExecutorServiceRequest::Effective"
    );
    assert!(
        dispatch.contains(".execute("),
        "test_model_with_db_path must invoke ExecutorServicePort::execute"
    );
    assert!(
        body.contains("extra_inputs") && body.contains("HashMap::new()"),
        "effective executor request must keep empty extra_inputs"
    );
    assert!(
        body.contains("working_dir: None"),
        "effective executor request must keep working_dir: None"
    );
    assert!(
        body.contains("parent_invocation_env: None"),
        "effective executor request must keep parent_invocation_env: None"
    );
    assert!(
        !body.contains("executor::execute_effective_with_inputs_and_env"),
        "test_model_with_db_path must not bypass ExecutorServicePort"
    );
}

#[test]
fn age38_test_model_with_db_path_routes_diagnostics_only_for_nonzero_exit() {
    let source = test_model_surface();
    let body = source_block_after(&source, "fn apply_exhaustion_disposition(");
    let dispatch = source_block_after(&source, "fn diagnostics_output_for_result(");

    assert!(
        dispatch.contains("DiagnosticsServiceRequest::ClassifyExhaustion"),
        "nonzero exits must classify exhaustion through DiagnosticsServiceRequest"
    );
    assert!(
        dispatch.contains(".diagnose("),
        "nonzero exits must call DiagnosticsServicePort::diagnose"
    );
    assert!(
        body.contains("should_run_diagnostics_fallback(result.exit_code)"),
        "diagnostics must remain gated to nonzero executor exits"
    );
    assert!(
        !body.contains("diagnostics::classify_exhaustion"),
        "test_model_with_db_path must not bypass DiagnosticsServicePort"
    );
}

#[test]
fn age38_test_model_with_db_path_marks_exhausted_through_quota_repository_only_when_classified() {
    let source = test_model_surface();
    let body = source_block_after(&source, "fn apply_exhaustion_disposition(");
    let dispatch = source_block_after(&source, "fn mark_effective_provider_exhausted(");

    assert!(
        dispatch.contains("ProviderQuotaRepository")
            && dispatch.contains("mark_exhausted(db, provider_name)"),
        "exhaustion marking must route through ProviderQuotaRepository::mark_exhausted"
    );
    assert!(
        body.contains("mark_effective_provider_exhausted(db, provider_name)")
            && body.contains("should_mark_quota_exhausted(should_mark_exhausted)"),
        "mark_exhausted must receive the effective provider name after exhaustion classification"
    );
    assert!(
        body.contains("is_exhausted") || body.contains("exhausted"),
        "mark_exhausted must remain conditional on the diagnostics classification"
    );
    assert!(
        !body.contains("db.mark_exhausted(&provider.name)"),
        "test_model_with_db_path must not call the StateDb inherent method directly"
    );
}

#[test]
fn age38_test_model_with_db_path_uses_injected_openers_loaders_and_keeps_no_lifecycle() {
    let source = test_model_surface();
    let body = source_block_after(&source, "fn test_model_with_db_path(");
    let lookup = source_block_after(&source, "fn open_test_model_state_db(");
    let providers = source_block_after(&source, "fn load_providers_config_or_default(");
    let route = source_block_after(&source, "fn select_test_model_route(");

    assert!(
        lookup.contains(".open_at(db_path"),
        "test_model_with_db_path must open state through StateDbOpener::open_at"
    );
    assert!(
        providers.contains(".load_providers(&providers_path)")
            || providers.contains(".load_providers(providers_path"),
        "test_model_with_db_path must load providers through ProvidersConfigRepository"
    );
    assert!(
        route.contains("ctx: None"),
        "test_model_with_db_path must keep cached-only routing"
    );
    for forbidden in [
        "start_invocation(",
        "finalize_invocation(",
        "InvocationLifecycle",
        "record_returned_artifacts(",
        "increment_calls_since_refresh(",
    ] {
        assert!(
            !body.contains(forbidden),
            "test_model_with_db_path must remain outside lifecycle calls: {forbidden}"
        );
    }
}
