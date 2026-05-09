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

fn lib_source() -> String {
    let source_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs");
    fs::read_to_string(&source_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", source_path.display()))
}

#[test]
fn age38_test_model_with_db_path_routes_executor_effective_request_through_service() {
    let source = lib_source();
    let body = source_block_after(&source, "fn test_model_with_db_path(");

    assert!(
        body.contains("ExecutorServiceRequest::Effective"),
        "test_model_with_db_path must build an ExecutorServiceRequest::Effective"
    );
    assert!(
        body.contains(".execute("),
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
    let source = lib_source();
    let body = source_block_after(&source, "fn test_model_with_db_path(");

    assert!(
        body.contains("DiagnosticsServiceRequest::ClassifyExhaustion"),
        "nonzero exits must classify exhaustion through DiagnosticsServiceRequest"
    );
    assert!(
        body.contains(".diagnose("),
        "nonzero exits must call DiagnosticsServicePort::diagnose"
    );
    assert!(
        body.contains("result.exit_code != 0"),
        "diagnostics must remain gated to nonzero executor exits"
    );
    assert!(
        !body.contains("diagnostics::classify_exhaustion"),
        "test_model_with_db_path must not bypass DiagnosticsServicePort"
    );
}

#[test]
fn age38_test_model_with_db_path_marks_exhausted_through_quota_repository_only_when_classified() {
    let source = lib_source();
    let body = source_block_after(&source, "fn test_model_with_db_path(");

    assert!(
        body.contains("ProviderQuotaRepository")
            || body.contains("<StateDb as")
                && body.contains("mark_exhausted(&")
                && body.contains("provider.name"),
        "exhaustion marking must route through ProviderQuotaRepository::mark_exhausted"
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
    let source = lib_source();
    let body = source_block_after(&source, "fn test_model_with_db_path(");

    assert!(
        body.contains(".open_at(&db_path)") || body.contains(".open_at(db_path"),
        "test_model_with_db_path must open state through StateDbOpener::open_at"
    );
    assert!(
        body.contains(".load_providers(&providers_path)")
            || body.contains(".load_providers(providers_path"),
        "test_model_with_db_path must load providers through ProvidersConfigRepository"
    );
    assert!(
        body.contains("ctx: None"),
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
