fn lib_source() -> &'static str {
    include_str!("../src/lib.rs")
}

fn setup_flow_source() -> &'static str {
    concat!(
        include_str!("../src/commands/setup_flow/mod.rs"),
        "\n",
        include_str!("../src/commands/setup_flow/orchestration.rs"),
        "\n",
        include_str!("../src/commands/setup_flow/accessor.rs"),
        "\n",
        include_str!("../src/commands/setup_flow/formatter.rs"),
    )
}

fn providers_accounts_source() -> &'static str {
    concat!(
        include_str!("../src/commands/providers_accounts/mod.rs"),
        "\n",
        include_str!("../src/commands/providers_accounts/orchestration.rs"),
        "\n",
        include_str!("../src/commands/providers_accounts/accessor.rs"),
        "\n",
        include_str!("../src/commands/providers_accounts/mapper.rs"),
        "\n",
        include_str!("../src/commands/providers_accounts/validator.rs"),
        "\n",
        include_str!("../src/commands/providers_accounts/formatter.rs"),
        "\n",
        include_str!("../src/commands/providers_accounts/display_name.rs"),
    )
}

fn discovery_source() -> &'static str {
    concat!(
        include_str!("../src/commands/discovery/mod.rs"),
        "\n",
        include_str!("../src/commands/discovery/orchestration.rs"),
        "\n",
        include_str!("../src/commands/discovery/accessor.rs"),
        "\n",
        include_str!("../src/commands/discovery/predicate.rs"),
        "\n",
        include_str!("../src/commands/discovery/formatter.rs"),
    )
}

fn discovery_accessor_source() -> &'static str {
    include_str!("../src/commands/discovery/accessor.rs")
}

fn run_tauri_source() -> &'static str {
    include_str!("../src/run_tauri.rs")
}

fn lib_commands_source() -> &'static str {
    include_str!("../src/lib_commands.rs")
}

fn command_accessor_source() -> &'static str {
    include_str!("../src/commands/accessor.rs")
}

fn schema_invariant_source() -> &'static str {
    include_str!("../../crates/oulipoly-state/tests/schema_invariant.rs")
}

fn tauri_client_spec_source() -> &'static str {
    include_str!("../../planning/coverage/spec-tauri-client.md")
}

fn function_body<'a>(source: &'a str, marker: &str) -> &'a str {
    let start = source
        .find(marker)
        .unwrap_or_else(|| panic!("missing function marker `{marker}`"));
    let open = source[start..]
        .find('{')
        .map(|idx| start + idx)
        .unwrap_or_else(|| panic!("missing opening brace for `{marker}`"));
    let mut depth = 0usize;
    for (offset, ch) in source[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[open..=open + offset];
                }
            }
            _ => {}
        }
    }
    panic!("missing closing brace for `{marker}`");
}

fn source_between<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let start_idx = source
        .find(start)
        .unwrap_or_else(|| panic!("missing start marker `{start}`"));
    let end_idx = source[start_idx..]
        .find(end)
        .map(|idx| start_idx + idx)
        .unwrap_or_else(|| panic!("missing end marker `{end}` after `{start}`"));
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

fn occurrence_count(haystack: &str, needle: &str) -> usize {
    haystack.match_indices(needle).count()
}

#[test]
fn age238_check_setup_needed_keeps_empty_models_and_claude_probe_residual() {
    let body = function_body(setup_flow_source(), "fn check_setup_needed");
    let compacted = compact(body);

    assert_contains(
        &compacted,
        "letmodels_empty=accessor::models_cache_is_empty(&state)?;",
        "model cache lookup must route through a named accessor helper",
    );
    assert_contains(
        &compacted,
        "ifmodels_empty{returnOk(true);}",
        "empty model cache must require setup",
    );
    assert_contains(
        body,
        "std::process::Command::new(\"which\").arg(\"claude\").output()",
        "existing Claude availability probe residual must remain visible before L6",
    );
    assert_contains(
        &compacted,
        "Ok(o)ifo.status.success()=>Ok(false)",
        "successful which claude probe currently suppresses setup",
    );
    assert_contains(
        &compacted,
        "_=>Ok(true)",
        "failed which claude probe currently requires setup",
    );
}

#[test]
fn age238_check_setup_needed_probe_shape_is_exact_and_emits_no_output() {
    let body = function_body(setup_flow_source(), "fn check_setup_needed");
    let compacted = compact(body);

    assert_contains(
        &compacted,
        "letoutput=std::process::Command::new(\"which\").arg(\"claude\").output();",
        "Claude residual probe must keep exact which/arg/output shape",
    );
    assert_order(
        body,
        "if models_empty",
        "std::process::Command::new(\"which\")",
        "empty cache return must precede provider-specific probe",
    );

    for forbidden in [
        "println!",
        "eprintln!",
        "dbg!",
        "SetupEvent",
        "Channel<",
        "mpsc::",
        ".send(",
        "stdout",
        "stderr",
    ] {
        assert_not_contains(
            body,
            forbidden,
            "check_setup_needed must not emit terminal, channel, or setup event output",
        );
    }
}

#[test]
fn age238_start_setup_keeps_channel_memory_and_run_wiring() {
    let body = function_body(setup_flow_source(), "async fn start_setup");
    let compacted = compact(body);

    for required in [
        "uuid::Uuid::new_v4().to_string()",
        "accessor::create_user_response_channel()",
        "accessor::store_setup_sender(&state,tx)?;",
        "spawn_setup_flow(on_event,rx,db_path,sid);",
    ] {
        assert_contains(
            &compacted,
            required,
            "start_setup must preserve setup flow launch contract",
        );
    }
    for required in [
        "state.models_dir",
        ".parent()",
        ".unwrap_or(&state.models_dir)",
        ".join(\"state.db\")",
    ] {
        assert_contains(
            setup_flow_source(),
            required,
            "setup memory DB path helper must preserve models_dir parent derivation",
        );
    }
}

#[test]
fn age238_start_setup_and_cli_setup_memory_open_errors_are_nonrecoverable_events() {
    for (marker, runner) in [
        ("async fn start_setup", "flow.run().await;"),
        ("async fn start_cli_setup", "flow.run_for_cli(&cli).await;"),
    ] {
        let body = function_body(
            setup_flow_source(),
            if marker == "async fn start_setup" {
                "fn spawn_setup_flow"
            } else {
                "fn spawn_cli_setup_flow"
            },
        );
        let compacted = compact(body);

        assert_eq!(
            occurrence_count(body, "memory_open_error_event"),
            1,
            "{marker} must emit exactly one memory-open error event path"
        );
        assert_contains(
            &compacted,
            "let_=on_event.send(formatter::memory_open_error_event(e));return;",
            "{marker} must keep byte-sensitive memory-open error event shape",
        );
        assert_order(
            body,
            "open_memory_graph(&db_path)",
            runner,
            "{marker} must open memory before launching setup flow",
        );
    }
}

#[test]
fn age238_start_cli_setup_keeps_channel_memory_and_run_for_cli_wiring() {
    let body = function_body(setup_flow_source(), "async fn start_cli_setup");
    let compacted = compact(body);

    for required in [
        "uuid::Uuid::new_v4().to_string()",
        "accessor::create_user_response_channel()",
        "accessor::store_setup_sender(&state,tx)?;",
        "letcli=cli_name.clone();",
        "spawn_cli_setup_flow(on_event,rx,db_path,sid,cli);",
    ] {
        assert_contains(
            &compacted,
            required,
            "start_cli_setup must preserve CLI-specific setup flow launch contract",
        );
    }
    for required in [
        "state.models_dir",
        ".parent()",
        ".unwrap_or(&state.models_dir)",
        ".join(\"state.db\")",
    ] {
        assert_contains(
            setup_flow_source(),
            required,
            "setup memory DB path helper must preserve models_dir parent derivation",
        );
    }
}

#[test]
fn age238_setup_response_and_cancel_keep_sender_lifecycle_strings() {
    let respond = function_body(setup_flow_source(), "fn setup_respond");
    let cancel = function_body(setup_flow_source(), "fn cancel_setup");
    let respond_compacted = compact(respond);
    let cancel_compacted = compact(cancel);

    for required in [
        "accessor::current_setup_sender(&state)?",
        "accessor::send_user_response(&tx,response)",
        "formatter::setup_send_error",
        "formatter::no_active_setup_session_error()",
    ] {
        assert_contains(
            &respond_compacted,
            required,
            "setup_respond must keep sender use and byte-sensitive errors",
        );
    }

    assert_contains(
        &cancel_compacted,
        "accessor::clear_setup_sender(&state)",
        "cancel_setup must clear the active setup sender",
    );
}

#[test]
fn age238_detection_commands_keep_current_detection_delegation() {
    let detect_clis = function_body(setup_flow_source(), "fn detect_clis");
    let sync_provider = function_body(providers_accounts_source(), "fn sync_provider");

    assert_contains(
        detect_clis,
        "setup_core::detection::detect_all()",
        "detect_clis must keep delegating to setup_core detection",
    );
    assert_contains(
        sync_provider,
        "accessor::detect_single_cli(&cli_name)",
        "sync_provider must keep detecting the named CLI before persistence",
    );
    assert_contains(
        providers_accounts_source(),
        "setup_core::detection::detect_single_cli(cli_name)",
        "named provider/account accessor must preserve the concrete detection call",
    );
}

#[test]
fn age238_provider_account_commands_keep_repository_routing_and_validation_contracts() {
    let source = providers_accounts_source();
    let repo_source = command_accessor_source();
    let open_state_db = function_body(repo_source, "fn open_state_db");
    let with_setup_repository = function_body(repo_source, "fn with_setup_repository");
    let list_providers = function_body(source, "fn list_cli_providers_inner");
    let get_provider = function_body(source, "fn get_cli_provider_inner");
    let list_accounts = function_body(source, "fn list_accounts_inner");
    let add_account = function_body(source, "fn add_account_inner");
    let remove_account = function_body(source, "fn remove_account_inner");

    assert_contains(
        open_state_db,
        "state.state_db_opener.open_at(&state.db_path())",
        "provider/account helpers must open the GUI-derived state DB through the injected opener",
    );
    assert_contains(
        with_setup_repository,
        "if let Some(repo) = state.setup_repository.as_ref()",
        "repository helper must preserve test-only SetupRepository injection",
    );
    assert_contains(
        with_setup_repository,
        "let db = open_state_db(state)?;",
        "repository helper must fall back to the real StateDb repository",
    );

    for (body, required, context) in [
        (
            list_providers,
            "accessor::list_cli_providers_inner(state)",
            "provider list routing",
        ),
        (
            get_provider,
            "accessor::get_cli_provider_inner(state, &cli_name)?",
            "provider get routing",
        ),
        (
            list_accounts,
            "accessor::list_accounts_inner(state, provider.as_deref())",
            "account list routing",
        ),
        (
            remove_account,
            "accessor::remove_account_inner(state, &id, &provider)",
            "account delete routing",
        ),
    ] {
        assert_contains(body, required, context);
    }

    for required in [
        "repo.list_cli_providers()",
        "repo.get_cli_provider(cli_name)",
        "repo.list_accounts(provider)",
        "repo.delete_account(id, provider)",
    ] {
        assert_contains(
            source,
            required,
            "provider/account repository accessor routing",
        );
    }

    for required in [
        "formatter::provider_not_found_error(&provider)",
        "accessor::insert_account_inner(state, &record)?",
    ] {
        assert_contains(
            add_account,
            required,
            "add_account must preserve validation, not-found, timestamp, and persistence shape",
        );
    }
    for required in [
        "Account id cannot be empty",
        "Account provider cannot be empty",
        "Account profile_name cannot be empty",
        "AuthStatus::Unknown",
        "chrono::Utc::now().to_rfc3339()",
    ] {
        assert_contains(
            source,
            required,
            "provider/account helpers must preserve validation and mapping contracts",
        );
    }

    assert_contains(
        get_provider,
        "formatter::provider_not_found_error(&cli_name)",
        "direct get_cli_provider missing-provider error must stay byte-identical",
    );
}

#[test]
fn age238_add_account_input_wire_fields_remain_stable() {
    let source = providers_accounts_source();
    let dto = source_between(
        source,
        "/// Input payload for adding a new account.",
        "\n}\n\npub(crate) use orchestration",
    );

    for required in [
        "#[derive(Deserialize)]",
        "pub id: String",
        "pub provider: String",
        "pub profile_name: String",
        "pub auth_method: AuthMethod",
    ] {
        assert_contains(
            dto,
            required,
            "AddAccountInput must preserve its current IPC field shape",
        );
    }
}

#[test]
fn age238_sync_provider_keeps_display_names_record_mapping_and_upsert() {
    let source = providers_accounts_source();
    let persist = function_body(source, "fn sync_provider_persist_record");

    for required in [
        "\"claude\" => \"Anthropic\"",
        "\"codex\" => \"OpenAI\"",
        "\"gemini\" => \"Google\"",
        "\"opencode\" => \"OpenCode\"",
        "_ => cli_name",
    ] {
        assert_contains(
            source,
            required,
            "provider display-name mapping is a current L6 residual contract",
        );
    }

    for required in [
        "cli_name: cli_info.name",
        "display_name: display_name::sync_provider_display_name(cli_name).to_string()",
        "installed: cli_info.installed",
        "version: cli_info.version",
        "config_dir: cli_info.config_dir.map(|p| p.to_string_lossy().to_string())",
        "last_synced: Some(now)",
        "chrono::Utc::now().to_rfc3339()",
    ] {
        assert_contains(
            source,
            required,
            "sync_provider record mapping must remain output-preserving",
        );
    }

    assert_contains(
        persist,
        "accessor::sync_provider_persist_record(state, record)",
        "sync_provider persistence must keep routing through SetupRepository::upsert_cli_provider",
    );
    assert_contains(
        source,
        "repo.upsert_cli_provider(record)",
        "sync_provider persistence must keep routing through SetupRepository::upsert_cli_provider",
    );
}

#[test]
fn age238_discovery_command_keeps_gui_db_open_spawn_and_join_error_contracts() {
    let body = function_body(discovery_source(), "async fn discover_models_cmd");

    for required in [
        "let db_path = accessor::state_db_path(&state);",
        "let state_db_opener = accessor::state_db_opener(&state);",
        "tauri::async_runtime::spawn_blocking(move ||",
        "accessor::discover_models_for_cli(&cli_name)?",
        "accessor::open_state_db_at(&state_db_opener, &db_path)?",
        "accessor::persist_discovery_result(&db, &cli_name, result)",
        "formatter::discovery_join_error",
    ] {
        assert_contains(
            body,
            required,
            "discover_models_cmd must preserve DB path, opener, blocking task, and error mapping",
        );
    }
}

#[test]
fn age238_discovery_persistence_keeps_empty_guard_and_delete_upsert_order() {
    let signature_and_body = source_between(
        discovery_accessor_source(),
        "pub fn persist_discovery_result",
        "\n}\n\npub fn delete_stale_models",
    );
    let body = function_body(
        discovery_accessor_source(),
        "pub fn persist_discovery_result",
    );
    let compacted = compact(body);

    assert_contains(
        signature_and_body,
        "repo: &dyn SetupRepository",
        "discovery persistence must be expressed against SetupRepository",
    );
    assert_contains(
        &compacted,
        "ifpredicate::has_discovered_models(&result){delete_stale_models(repo,cli_name,&result.cli_version)?;}",
        "empty discovery results must not delete stale rows",
    );
    assert_order(
        body,
        "delete_stale_models",
        "upsert_discovered_model",
        "non-empty discovery persistence ordering",
    );
    assert_order(
        body,
        "upsert_discovered_model",
        "upsert_model_parameter",
        "non-empty discovery persistence ordering",
    );
    assert_contains(
        body,
        "Ok(result.models)",
        "persist_discovery_result must return the discovered models unchanged",
    );
}

#[test]
fn age238_discovery_read_commands_keep_repository_filters() {
    let list_models = function_body(discovery_source(), "fn list_discovered_models_inner");
    let parameters = function_body(discovery_source(), "fn get_model_parameters_inner");

    assert_contains(
        list_models,
        "accessor::list_discovered_models_inner(state, provider.as_deref())",
        "list_discovered_models must route provider filter through SetupRepository",
    );
    assert_contains(
        parameters,
        "accessor::list_model_parameters_inner(state, &model_name, &provider)",
        "get_model_parameters must route model/provider filter through SetupRepository",
    );
}

#[test]
fn age238_run_tauri_registration_still_names_every_l3_command_in_order() {
    let handler = source_between(
        run_tauri_source(),
        ".invoke_handler(tauri::generate_handler![",
        "])\n        .run",
    );

    let expected = [
        "check_setup_needed",
        "start_setup",
        "start_cli_setup",
        "setup_respond",
        "cancel_setup",
        "detect_clis",
        "get_memory_graph",
        "list_cli_providers",
        "get_cli_provider",
        "list_accounts",
        "add_account",
        "remove_account",
        "sync_provider",
        "discover_models_cmd",
        "list_discovered_models",
        "get_model_parameters",
    ];

    let mut cursor = 0usize;
    for command in expected {
        let offset = handler[cursor..]
            .find(command)
            .unwrap_or_else(|| panic!("missing L3 command registration `{command}`"));
        cursor += offset + command.len();
    }

    for required in [
        "commands::setup_flow::check_setup_needed",
        "commands::providers_accounts::list_cli_providers",
        "commands::discovery::discover_models_cmd",
    ] {
        assert_contains(
            handler,
            required,
            "moved L3 commands must register by direct module path",
        );
    }
    assert_not_contains(
        handler,
        "crate::check_setup_needed",
        "moved setup commands must not register through crate-root re-exports",
    );
    assert_contains(
        lib_commands_source(),
        "pub mod models",
        "lib_commands facade read keeps this test anchored to the command module facade",
    );
}

#[test]
fn age238_run_tauri_registration_keeps_reload_models_adjacency_before_extraction() {
    let handler = source_between(
        run_tauri_source(),
        ".invoke_handler(tauri::generate_handler![",
        "])\n        .run",
    );

    for required in [
        "commands::setup_flow::check_setup_needed",
        "commands::setup_flow::start_setup",
        "commands::setup_flow::start_cli_setup",
        "crate::reload_models",
        "commands::setup_flow::setup_respond",
        "commands::providers_accounts::list_cli_providers",
        "commands::discovery::discover_models_cmd",
    ] {
        assert_contains(
            handler,
            required,
            "pre-extraction registration must preserve current root-path source shape",
        );
    }
    assert_order(
        handler,
        "commands::setup_flow::start_cli_setup",
        "crate::reload_models",
        "reload_models must remain adjacent to setup command registration",
    );
    assert_order(
        handler,
        "crate::reload_models",
        "commands::setup_flow::setup_respond",
        "reload_models must stay before setup response registration",
    );
}

#[test]
fn age238_provider_settings_adjacency_guard_remains_source_local_before_extraction() {
    let reload = function_body(lib_source(), "fn reload_models");

    assert_contains(
        reload,
        "provider_settings::refresh_provider_settings_host(&state)?",
        "provider-settings refresh must remain attached to reload_models while lib.rs is the residual owner",
    );
    assert_contains(
        lib_source(),
        "#[path = \"commands/provider_settings.rs\"]",
        "provider-settings module path must remain available beside the lib.rs residual reload handler",
    );
}

#[test]
fn age238_carrier_and_spec_update_expectations_have_current_anchors() {
    let schema = schema_invariant_source();
    let spec = tauri_client_spec_source();

    for required in [
        "src-tauri/src/lib.rs",
        "src-tauri/src/app_state.rs",
        "src-tauri/src/run_tauri.rs",
        "src-tauri/src/commands/models/orchestration.rs",
        "src-tauri/src/commands/quota_refresh/orchestration.rs",
        "src-tauri/src/commands/setup_flow/orchestration.rs",
        "src-tauri/src/commands/providers_accounts/orchestration.rs",
        "src-tauri/src/commands/discovery/orchestration.rs",
        "src-tauri/src/commands/accessor.rs",
    ] {
        assert_contains(
            schema,
            required,
            "schema_invariant must remain the carrier update point for extracted command modules",
        );
        assert_contains(
            spec,
            required,
            "spec-tauri-client.md must remain the source-list update point for extracted command modules",
        );
    }

    for required in [
        "declaration_carriers_present_in_source",
        "assert_declared_roles",
        "assert_carrier",
    ] {
        assert_contains(
            schema,
            required,
            "carrier guard must keep declared-role and adapter/intrinsic assertion helpers available",
        );
    }
}
