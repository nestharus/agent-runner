fn main_source() -> &'static str {
    include_str!("../src/main.rs")
}

fn lib_source() -> &'static str {
    include_str!("../src/lib.rs")
}

fn wiring_source() -> &'static str {
    include_str!("../src/wiring.rs")
}

fn source_slice<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let start_idx = source
        .find(start)
        .unwrap_or_else(|| panic!("missing {start}"));
    let end_idx = source[start_idx..]
        .find(end)
        .map(|idx| start_idx + idx)
        .unwrap_or_else(|| panic!("missing {end} after {start}"));
    &source[start_idx..end_idx]
}

fn compact(source: &str) -> String {
    source.split_whitespace().collect::<String>()
}

fn assert_contains(haystack: &str, needle: &str, context: &str) {
    assert!(
        haystack.contains(needle),
        "{context}: expected source to contain `{needle}`"
    );
}

fn assert_not_contains(haystack: &str, needle: &str, context: &str) {
    assert!(
        !haystack.contains(needle),
        "{context}: source must not contain `{needle}`"
    );
}

fn assert_order(haystack: &str, first: &str, second: &str, context: &str) {
    let first_idx = haystack
        .find(first)
        .unwrap_or_else(|| panic!("{context}: missing first marker `{first}`"));
    let second_idx = haystack
        .find(second)
        .unwrap_or_else(|| panic!("{context}: missing second marker `{second}`"));
    assert!(
        first_idx < second_idx,
        "{context}: `{first}` must appear before `{second}`"
    );
}

fn run_slice() -> &'static str {
    source_slice(
        main_source(),
        "fn run(cli: Cli)",
        "fn run_session_schema_probe(",
    )
}

fn ingest_slice() -> &'static str {
    source_slice(
        main_source(),
        "fn ingest_and_emit_session_id_resume_aware(",
        "fn emit_known_session_id(",
    )
}

fn repl_slice() -> &'static str {
    source_slice(main_source(), "fn run_repl(", "fn run_resume(")
}

fn resume_slice() -> &'static str {
    source_slice(main_source(), "fn run_resume(", "fn run_with_balancing(")
}

fn one_shot_slice() -> &'static str {
    source_slice(
        main_source(),
        "fn run_with_balancing(",
        "fn supervise_captured_child_invocations(",
    )
}

fn diagnostics_slice() -> &'static str {
    source_slice(main_source(), "fn run_diagnostics(", "fn run_resume_list(")
}

fn entrypoint_slice() -> &'static str {
    source_slice(main_source(), "fn main() -> ExitCode", "#[cfg(test)]")
}

#[test]
fn age_39_repl_non_resume_routing_uses_routing_service_port() {
    let repl = compact(repl_slice());

    assert_contains(&repl, "AgentRuntimeServices", "run_repl signature");
    assert_not_contains(
        &repl,
        "balancer::select_provider(",
        "non-resume REPL routing cut-over",
    );
    assert_contains(&repl, "routing_service", "non-resume REPL routing cut-over");
    assert_contains(&repl, ".select_route(", "non-resume REPL routing cut-over");
    assert_contains(
        &repl,
        "RoutingServiceRequest",
        "non-resume REPL routing cut-over",
    );
    assert_contains(&repl, "ctx:Some(", "non-resume REPL routing cut-over");
}

#[test]
fn age_39_one_shot_routing_uses_routing_service_port_after_parent_resolution() {
    let one_shot = compact(one_shot_slice());

    assert_contains(
        &one_shot,
        "AgentRuntimeServices",
        "run_with_balancing signature",
    );
    assert_not_contains(
        &one_shot,
        "balancer::select_provider(",
        "one-shot routing cut-over",
    );
    assert_contains(&one_shot, "routing_service", "one-shot routing cut-over");
    assert_contains(&one_shot, ".select_route(", "one-shot routing cut-over");
    assert_contains(
        &one_shot,
        "RoutingServiceRequest",
        "one-shot routing cut-over",
    );
    assert_contains(&one_shot, "ctx:Some(", "one-shot routing cut-over");
    assert_order(
        &one_shot,
        "resolve_parent_invocation_id(&state)",
        ".select_route(",
        "one-shot parent resolution must precede routing",
    );
}

#[test]
fn age_39_invocation_start_uses_lifecycle_service_in_repl_resume_and_one_shot() {
    for (name, body) in [
        ("run_repl", compact(repl_slice())),
        ("run_resume", compact(resume_slice())),
        ("run_with_balancing", compact(one_shot_slice())),
    ] {
        assert_contains(&body, "AgentRuntimeServices", name);
        assert_not_contains(&body, "state.start_invocation(", name);
        assert_contains(&body, "invocation_lifecycle_service", name);
        assert_contains(&body, ".start_invocation(", name);
        assert_contains(&body, "InvocationLifecycleStartRequest", name);
    }
}

#[test]
fn age_39_repl_completion_finalization_uses_lifecycle_service_port() {
    let repl = compact(repl_slice());

    assert_contains(&repl, "AgentRuntimeServices", "run_repl signature");
    assert_not_contains(
        &repl,
        "state.finalize_invocation(",
        "REPL finalization cut-over",
    );
    assert_contains(
        &repl,
        "invocation_lifecycle_service",
        "REPL finalization cut-over",
    );
    assert_contains(&repl, ".finalize_invocation(", "REPL finalization cut-over");
    assert_contains(
        &repl,
        "InvocationLifecycleFinalizeRequest",
        "REPL finalization cut-over",
    );
    assert_order(
        &repl,
        ".finalize_invocation(",
        "guard.mark_finalized()",
        "explicit REPL finalization must precede guard suppression",
    );
}

#[test]
fn age_39_headless_resume_finalization_uses_lifecycle_service_port() {
    let resume = compact(resume_slice());

    assert_contains(&resume, "AgentRuntimeServices", "run_resume signature");
    assert_not_contains(
        &resume,
        "state.finalize_invocation(",
        "headless resume finalization cut-over",
    );
    assert_contains(
        &resume,
        "invocation_lifecycle_service",
        "headless resume finalization cut-over",
    );
    assert_contains(
        &resume,
        ".finalize_invocation(",
        "headless resume finalization cut-over",
    );
    assert_contains(
        &resume,
        "InvocationLifecycleFinalizeRequest",
        "headless resume finalization cut-over",
    );
}

#[test]
fn age_39_one_shot_finalization_uses_lifecycle_service_port() {
    let one_shot = compact(one_shot_slice());

    assert_contains(
        &one_shot,
        "AgentRuntimeServices",
        "run_with_balancing signature",
    );
    assert_not_contains(
        &one_shot,
        "state.finalize_invocation(",
        "one-shot finalization cut-over",
    );
    assert_contains(
        &one_shot,
        "invocation_lifecycle_service",
        "one-shot finalization cut-over",
    );
    assert_contains(
        &one_shot,
        ".finalize_invocation(",
        "one-shot finalization cut-over",
    );
    assert_contains(
        &one_shot,
        "InvocationLifecycleFinalizeRequest",
        "one-shot finalization cut-over",
    );
}

#[test]
fn age_39_one_shot_executor_uses_executor_service_effective_request() {
    let one_shot = compact(one_shot_slice());

    assert_contains(
        &one_shot,
        "AgentRuntimeServices",
        "run_with_balancing signature",
    );
    assert_not_contains(
        &one_shot,
        "executor::execute_effective_with_inputs_and_env(",
        "one-shot executor cut-over",
    );
    assert_contains(&one_shot, "executor_service", "one-shot executor cut-over");
    assert_contains(&one_shot, ".execute(", "one-shot executor cut-over");
    assert_contains(
        &one_shot,
        "ExecutorServiceRequest::Effective",
        "one-shot executor cut-over",
    );
}

#[test]
fn age_39_cli_diagnostics_uses_diagnostics_service_diagnose_error_request() {
    let diagnostics = compact(diagnostics_slice());

    assert_contains(
        &diagnostics,
        "AgentRuntimeServices",
        "run_diagnostics signature",
    );
    assert_not_contains(
        &diagnostics,
        "diagnostics::diagnose_error(",
        "CLI diagnostics cut-over",
    );
    assert_contains(
        &diagnostics,
        "diagnostics_service",
        "CLI diagnostics cut-over",
    );
    assert_contains(&diagnostics, ".diagnose(", "CLI diagnostics cut-over");
    assert_contains(
        &diagnostics,
        "DiagnosticsServiceRequest::DiagnoseError",
        "CLI diagnostics cut-over",
    );
}

#[test]
fn age_39_session_ingest_uses_shared_session_lifecycle_service() {
    let ingest = compact(ingest_slice());

    assert_contains(
        &ingest,
        "AgentRuntimeServices",
        "session ingest helper signature",
    );
    assert_not_contains(
        &ingest,
        "ProductionSessionLifecycleService::new()",
        "session lifecycle graph cut-over",
    );
    assert_contains(
        &ingest,
        "session_lifecycle_service",
        "session lifecycle graph cut-over",
    );
    assert_contains(
        &ingest,
        ".ingest_session(",
        "session lifecycle graph cut-over",
    );
    assert_contains(
        &ingest,
        "SessionLifecycleRequest",
        "session lifecycle graph cut-over",
    );
}

#[test]
fn age_39_resume_resolution_uses_shared_resume_service_in_repl_and_headless() {
    for (name, body) in [
        ("run_repl", compact(repl_slice())),
        ("run_resume", compact(resume_slice())),
    ] {
        assert_contains(&body, "AgentRuntimeServices", name);
        assert_not_contains(&body, "ProductionResumeService::new()", name);
        assert_contains(&body, "resume_service", name);
        assert_contains(&body, ".resolve_resume(", name);
        assert_contains(&body, "ResumeServiceRequest", name);
    }
}

#[test]
fn age_39_migration_uses_shared_migration_service_in_repl_and_headless() {
    for (name, body) in [
        ("run_repl", compact(repl_slice())),
        ("run_resume", compact(resume_slice())),
    ] {
        assert_contains(&body, "AgentRuntimeServices", name);
        assert_not_contains(&body, "ProductionMigrationService::new()", name);
        assert_contains(&body, "migration_service", name);
        assert_contains(&body, ".migrate(", name);
        assert_contains(&body, "MigrationServiceRequest", name);
    }
}

#[test]
fn age_39_resume_acceptance_uses_shared_resume_service_record_acceptance() {
    let resume = compact(resume_slice());

    assert_contains(&resume, "AgentRuntimeServices", "run_resume signature");
    assert_not_contains(
        &resume,
        "ProductionResumeService::new()",
        "resume acceptance graph cut-over",
    );
    assert_contains(
        &resume,
        "resume_service",
        "resume acceptance graph cut-over",
    );
    assert_contains(
        &resume,
        ".record_acceptance(",
        "resume acceptance graph cut-over",
    );
    assert_contains(
        &resume,
        "ResumeAcceptanceRequest",
        "resume acceptance graph cut-over",
    );
}

#[test]
fn age_39_returned_artifacts_are_persisted_before_lifecycle_finalization() {
    for (name, body) in [
        ("run_resume", compact(resume_slice())),
        ("run_with_balancing", compact(one_shot_slice())),
    ] {
        assert_contains(&body, "record_returned_artifacts(", name);
        assert_contains(&body, "invocation_lifecycle_service", name);
        assert_contains(&body, ".finalize_invocation(", name);

        let artifacts = body
            .find("record_returned_artifacts(")
            .unwrap_or_else(|| panic!("{name}: missing returned-artifact persistence"));
        let finalization = body
            .rfind(".finalize_invocation(")
            .unwrap_or_else(|| panic!("{name}: missing lifecycle finalization"));
        assert!(
            artifacts < finalization,
            "{name}: returned artifacts must be persisted before normal finalization"
        );
    }
}

#[test]
fn age_39_startup_recovery_runs_before_cli_service_graph_and_dispatch() {
    let run = compact(run_slice());

    assert_order(
        &run,
        "session_replace::recover_pending_replaces()",
        "ifcli.new",
        "startup recovery must precede --new dispatch",
    );
    assert_order(
        &run,
        "session_replace::recover_pending_replaces()",
        "ifletSome(command)=cli.command.clone()",
        "startup recovery must precede subcommand dispatch",
    );
    assert_order(
        &run,
        "session_replace::recover_pending_replaces()",
        "AgentRuntimeServices::cli_defaults()",
        "startup recovery must precede CLI service graph construction",
    );
    assert_order(
        &run,
        "ifcli.new",
        "AgentRuntimeServices::cli_defaults()",
        "the --new default-provider path remains outside the shared CLI graph",
    );
}

#[test]
fn age_39_no_arg_tauri_launch_branch_remains_before_clap_parse() {
    let entrypoint = compact(entrypoint_slice());

    assert_order(
        &entrypoint,
        "ifstd::env::args().len()==1",
        "Cli::parse_from(",
        "no-arg Tauri launch branch must stay before clap parsing",
    );
    assert_order(
        &entrypoint,
        "agent_runner_lib::run_tauri()",
        "Cli::parse_from(",
        "Tauri launch must remain outside the argv-bearing CLI parser path",
    );
}

#[test]
fn age_39_no_port_residuals_remain_direct_and_explicit() {
    let source = compact(main_source());
    let finalizer_drop = source_slice(
        main_source(),
        "impl Drop for FinalizerGuard",
        "fn should_emit_invocation_line(",
    );
    let one_shot = compact(one_shot_slice());

    assert_contains(
        &compact(finalizer_drop),
        "self.db.finalize_invocation(",
        "FinalizerGuard::drop remains direct residual",
    );
    assert_contains(
        &source,
        "fnemit_known_session_id(",
        "known-session fallback helper remains direct residual",
    );
    assert_contains(
        &one_shot,
        "emit_known_session_id(",
        "known-session fallback remains in one-shot success path",
    );
    assert_contains(
        &one_shot,
        "result.session_capture.method.db_value()",
        "known-session fallback keeps exact capture method",
    );
}

#[test]
fn age_39_tauri_ipc_adjacency_preserves_generate_handler_and_service_graph_fields() {
    let lib = compact(lib_source());
    let wiring = compact(wiring_source());

    assert_contains(
        &lib,
        "tauri::generate_handler![",
        "Tauri command handler registration",
    );
    for command in [
        "check_setup_needed",
        "start_setup",
        "list_models",
        "get_model",
        "save_model",
        "delete_model",
        "list_pools",
        "refresh_quotas",
        "update_pool",
        "test_model",
    ] {
        assert_contains(&lib, command, "Tauri command handler registration");
    }

    for field in [
        "pubrouting_service:Arc<ProductionRoutingService>",
        "pubinvocation_lifecycle_service:Arc<ProductionInvocationLifecycleService>",
        "pubexecutor_service:Arc<dynExecutorServicePort>",
        "pubdiagnostics_service:Arc<dynDiagnosticsServicePort>",
        "pubresume_service:Arc<dynResumeServicePort>",
        "pubsession_lifecycle_service:Arc<dynSessionLifecycleServicePort>",
        "pubmigration_service:Arc<dynMigrationServicePort>",
    ] {
        assert_contains(&wiring, field, "AgentRuntimeServices field surface");
    }
}
