//! ## Declared roles
//!
//! `accessor`, `parser`, `filter`, `validator`

use oulipoly_state::{migrations, schema};
use std::path::{Path, PathBuf};

#[test]
fn no_schema_bump_for_age_166() {
    // AGE-166 must not introduce its own state DB migration. The schema
    // baseline is whatever main currently provides; AGE-166 only verifies
    // no migration file is named after this WU.
    let migrations_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let has_age166_migration = std::fs::read_dir(&migrations_dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .any(|file_name| file_name.contains("age_166") || file_name.contains("age166"));

    assert!(
        !has_age166_migration,
        "AGE-166 must not add a state DB migration"
    );

    // Belt-and-suspenders: the manifest's last entry must agree with the
    // exported CURRENT_SCHEMA_VERSION constant; AGE-166 is not allowed to
    // bump either value past main.
    let manifest = migrations::manifest();
    let last = manifest.last().expect("migration manifest is non-empty");
    assert_eq!(last.target_version, schema::CURRENT_SCHEMA_VERSION);
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("state crate lives under crates/oulipoly-state")
        .to_path_buf()
}

fn source(file_path: &str) -> String {
    std::fs::read_to_string(repo_root().join(file_path))
        .unwrap_or_else(|err| panic!("failed to read {file_path}: {err}"))
}

fn assert_declared_roles(file_path: &str, roles: &[&str]) {
    let source = source(file_path);
    if !matches!(
        file_path,
        "src-tauri/src/app_state.rs"
            | "src-tauri/src/app_paths.rs"
            | "src-tauri/src/run_tauri.rs"
            | "src-tauri/src/lib.rs"
    ) {
        assert!(
            source.contains("## Declared roles"),
            "{file_path} must include a ## Declared roles carrier"
        );
        for role in roles {
            assert!(
                source.contains(role),
                "{file_path} declared roles carrier must include {role:?}"
            );
        }
        return;
    }

    let declared = declared_roles(&source)
        .unwrap_or_else(|| panic!("{file_path} must include a ## Declared roles carrier"));
    let expected = roles
        .iter()
        .copied()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    assert_eq!(
        declared, expected,
        "{file_path} declared roles carrier must exactly match expected roles"
    );
}

fn declared_roles(source: &str) -> Option<Vec<String>> {
    let after_header = source.split("## Declared roles").nth(1)?;
    let block = after_header
        .lines()
        .skip_while(|line| line.trim().is_empty() || line.trim() == "//!")
        .take_while(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && trimmed != "//!" && !trimmed.contains("## ")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let roles = block
        .split('`')
        .skip(1)
        .step_by(2)
        .flat_map(|chunk| chunk.split(','))
        .map(str::trim)
        .filter(|role| !role.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    (!roles.is_empty()).then_some(roles)
}

fn assert_carrier(file_path: &str, header: &str, snippets: &[&str]) {
    let source = source(file_path);
    assert!(
        source.contains(header),
        "{file_path} must include a {header} carrier"
    );
    for snippet in snippets {
        assert!(
            source.contains(snippet),
            "{file_path} {header} carrier must include {snippet:?}"
        );
    }
}

#[test]
fn declaration_carriers_present_in_source() {
    for (file_path, roles) in [
        (
            "crates/oulipoly-runtime/src/executor/cli.rs",
            &[
                "orchestration",
                "mapper",
                "parser",
                "validator",
                "formatter",
                "filter",
                "accessor",
                "predicate",
            ][..],
        ),
        (
            "crates/oulipoly-runtime/src/diagnostics/mod.rs",
            &[
                "accessor",
                "mapper",
                "parser",
                "formatter",
                "predicate",
                "orchestration",
            ][..],
        ),
        ("src-tauri/src/lib.rs", &["none"][..]),
        (
            "src-tauri/src/app_state.rs",
            &["orchestration", "mapper"][..],
        ),
        ("src-tauri/src/app_paths.rs", &["accessor", "mapper"][..]),
        (
            "src-tauri/src/run_tauri.rs",
            &["orchestration", "mapper"][..],
        ),
        ("src-tauri/src/commands/models/mod.rs", &["none"][..]),
        (
            "src-tauri/src/commands/models/accessor.rs",
            &["accessor"][..],
        ),
        (
            "src-tauri/src/commands/models/validator.rs",
            &["validator"][..],
        ),
        (
            "src-tauri/src/commands/models/formatter.rs",
            &["formatter"][..],
        ),
        (
            "src-tauri/src/commands/models/orchestration.rs",
            &["orchestration"][..],
        ),
        (
            "src-tauri/src/commands/models/reload.rs",
            &["orchestration"][..],
        ),
        ("src-tauri/src/commands/pools/mod.rs", &["none"][..]),
        (
            "src-tauri/src/commands/pools/derive.rs",
            &["mapper", "filter"][..],
        ),
        (
            "src-tauri/src/commands/pools/update.rs",
            &["orchestration", "mapper"][..],
        ),
        (
            "src-tauri/src/commands/pools/accessor.rs",
            &["accessor"][..],
        ),
        (
            "src-tauri/src/commands/pools/validator.rs",
            &["validator"][..],
        ),
        (
            "src-tauri/src/commands/pools/writer.rs",
            &["accessor", "formatter"][..],
        ),
        (
            "src-tauri/src/commands/quota_refresh/mod.rs",
            &["mapper"][..],
        ),
        (
            "src-tauri/src/commands/quota_refresh/orchestration.rs",
            &["orchestration"][..],
        ),
        (
            "src-tauri/src/commands/quota_refresh/candidates.rs",
            &["filter", "mapper"][..],
        ),
        (
            "src-tauri/src/commands/quota_refresh/accessor.rs",
            &["accessor", "mapper", "predicate", "formatter"][..],
        ),
        (
            "src-tauri/src/commands/quota_refresh/mapper.rs",
            &["mapper"][..],
        ),
        ("src-tauri/src/commands/accessor.rs", &["accessor"][..]),
        (
            "src-tauri/src/commands/setup_flow/mod.rs",
            &["orchestration"][..],
        ),
        (
            "src-tauri/src/commands/setup_flow/accessor.rs",
            &["accessor"][..],
        ),
        (
            "src-tauri/src/commands/setup_flow/formatter.rs",
            &["formatter"][..],
        ),
        (
            "src-tauri/src/commands/setup_flow/orchestration.rs",
            &["orchestration"][..],
        ),
        (
            "src-tauri/src/commands/setup_flow/provider_probe.rs",
            &["predicate"][..],
        ),
        (
            "src-tauri/src/commands/providers_accounts/mod.rs",
            &["orchestration"][..],
        ),
        (
            "src-tauri/src/commands/providers_accounts/accessor.rs",
            &["accessor"][..],
        ),
        (
            "src-tauri/src/commands/providers_accounts/validator.rs",
            &["validator"][..],
        ),
        (
            "src-tauri/src/commands/providers_accounts/mapper.rs",
            &["mapper"][..],
        ),
        (
            "src-tauri/src/commands/providers_accounts/formatter.rs",
            &["formatter"][..],
        ),
        (
            "src-tauri/src/commands/providers_accounts/display_name.rs",
            &["mapper"][..],
        ),
        (
            "src-tauri/src/commands/providers_accounts/orchestration.rs",
            &["orchestration"][..],
        ),
        (
            "src-tauri/src/commands/test_model/diagnostics_fallback.rs",
            &["orchestration"][..],
        ),
        (
            "src-tauri/src/commands/discovery/mod.rs",
            &["orchestration"][..],
        ),
        (
            "src-tauri/src/commands/discovery/accessor.rs",
            &["accessor"][..],
        ),
        (
            "src-tauri/src/commands/discovery/predicate.rs",
            &["predicate"][..],
        ),
        (
            "src-tauri/src/commands/discovery/formatter.rs",
            &["formatter"][..],
        ),
        (
            "src-tauri/src/commands/discovery/orchestration.rs",
            &["orchestration"][..],
        ),
        (
            "src-tauri/src/terminal_outcome_adapter.rs",
            &[
                "mapper",
                "formatter",
                "parser",
                "predicate",
                "orchestration",
            ][..],
        ),
        (
            "src-tauri/src/dispatch.rs",
            &[
                "orchestration",
                "parser",
                "validator",
                "accessor",
                "formatter",
                "mapper",
                "predicate",
                "filter",
            ][..],
        ),
        (
            "src-tauri/src/zero_turn_orchestration.rs",
            &[
                "orchestration",
                "accessor",
                "formatter",
                "mapper",
                "predicate",
            ][..],
        ),
        (
            "src-tauri/src/usage/cli.rs",
            &["parser", "validator", "mapper"][..],
        ),
        (
            "src-tauri/src/resume_cli.rs",
            &["orchestration", "mapper", "predicate"][..],
        ),
        (
            "crates/oulipoly-runtime/src/sessions/mod.rs",
            &[
                "orchestration",
                "parser",
                "validator",
                "mapper",
                "formatter",
                "predicate",
            ][..],
        ),
        (
            "crates/oulipoly-config/src/sessions.rs",
            &["parser", "mapper", "accessor", "formatter", "predicate"][..],
        ),
        (
            "crates/oulipoly-config/src/model.rs",
            &[
                "parser",
                "validator",
                "mapper",
                "formatter",
                "accessor",
                "predicate",
            ][..],
        ),
        (
            "crates/oulipoly-state/src/repositories/mod.rs",
            &["accessor", "mapper", "validator", "orchestration"][..],
        ),
    ] {
        assert_declared_roles(file_path, roles);
    }

    for (file_path, snippets) in [
        (
            "src-tauri/src/lib.rs",
            &[
                "## Intrinsic-surface declarations",
                "Domain: tauri-client facade",
                "functionless facade sentinel",
                "public re-export compatibility",
                "module declaration boundary",
            ][..],
        ),
        (
            "src-tauri/src/dispatch.rs",
            &[
                "## Intrinsic-surface declarations",
                "Domain: cli_lifecycle_orchestration",
                "lifecycle loops",
                "finalization sequencing",
            ][..],
        ),
        (
            "src-tauri/src/zero_turn_orchestration.rs",
            &[
                "## Intrinsic-surface declarations",
                "Domain: zero_turn_confirmation",
                "confirmation state",
                "baseline/delta classification",
            ][..],
        ),
        (
            "crates/oulipoly-runtime/src/sessions/mod.rs",
            &[
                "## Intrinsic-surface declarations",
                "Domain: session_turn_ingest",
                "adapter-script scan/ingest",
                "ScanReport.new_turns",
            ][..],
        ),
        (
            "crates/oulipoly-state/src/db.rs",
            &[
                "## Intrinsic-surface declarations",
                "Domain: state_db_persistence",
                "provider_quotas.exhausted_at",
                "count_session_turns",
            ][..],
        ),
        (
            "src-tauri/src/commands/models/orchestration.rs",
            &[
                "## Intrinsic-surface declarations",
                "Domain: model-save lifecycle",
                "src-tauri/src/commands/models/validator.rs",
            ][..],
        ),
        (
            "src-tauri/src/commands/pools/update.rs",
            &[
                "## Intrinsic-surface declarations",
                "Domain: pool-update lifecycle",
                "src-tauri/src/commands/pools/accessor.rs",
                "src-tauri/src/commands/pools/writer.rs",
            ][..],
        ),
        (
            "src-tauri/src/commands/quota_refresh/orchestration.rs",
            &[
                "## Intrinsic-surface declarations",
                "Domain: quota-refresh lifecycle",
                "src-tauri/src/commands/quota_refresh/candidates.rs",
                "src-tauri/src/commands/quota_refresh/accessor.rs",
                "src-tauri/src/commands/quota_refresh/mapper.rs",
            ][..],
        ),
        (
            "src-tauri/src/commands/setup_flow/orchestration.rs",
            &[
                "## Intrinsic-surface declarations",
                "Domain: setup-flow command lifecycle",
                "src-tauri/src/commands/setup_flow/accessor.rs",
                "src-tauri/src/commands/setup_flow/formatter.rs",
            ][..],
        ),
        (
            "src-tauri/src/commands/providers_accounts/orchestration.rs",
            &[
                "## Intrinsic-surface declarations",
                "Domain: provider-account command lifecycle",
                "src-tauri/src/commands/providers_accounts/accessor.rs",
                "src-tauri/src/commands/accessor.rs",
            ][..],
        ),
        (
            "src-tauri/src/commands/discovery/orchestration.rs",
            &[
                "## Intrinsic-surface declarations",
                "Domain: discovery persistence lifecycle",
                "src-tauri/src/commands/discovery/predicate.rs",
                "src-tauri/src/commands/accessor.rs",
            ][..],
        ),
    ] {
        assert_carrier(file_path, "## Intrinsic-surface declarations", snippets);
    }

    for (file_path, snippets) in [
        (
            "src-tauri/src/terminal_outcome_adapter.rs",
            &[
                "ExecutionResult.terminal_signal",
                "TerminalSignalKind",
                "ErrorCategory",
                "AGE-153 forced terminal-signal fixture",
            ][..],
        ),
        (
            "src-tauri/src/resume_cli.rs",
            &[
                "resume acceptance result",
                "typed terminal outcome category",
                "diagnostics fallback category",
            ][..],
        ),
        (
            "src-tauri/src/commands/test_model/mod.rs",
            &[
                "test_model",
                "Tauri IPC",
                "executor service output",
                "provider quota mutation",
                "TestModelResult",
            ][..],
        ),
        (
            "src-tauri/src/app_paths.rs",
            &["config-directory contract", "providers.toml parent-path"][..],
        ),
        (
            "src-tauri/src/run_tauri.rs",
            &["Tauri IPC command-registration", "RuntimePaths"][..],
        ),
        (
            "src-tauri/src/commands/models/mod.rs",
            &[
                "ModelSummary serialization contract",
                "frontend model-list DTO",
            ][..],
        ),
        (
            "src-tauri/src/commands/models/orchestration.rs",
            &[
                "Tauri IPC model command contract",
                "provider-settings refresh lifecycle contract",
            ][..],
        ),
        (
            "src-tauri/src/commands/models/reload.rs",
            &[
                "model-reload lifecycle contract",
                "provider-config load contract",
                "model-cache mutation contract",
                "provider-settings refresh contract",
                "Tauri command registration contract",
            ][..],
        ),
        (
            "src-tauri/src/commands/models/accessor.rs",
            &[
                "AppState model-cache mutex contract",
                "model file persistence contract",
            ][..],
        ),
        (
            "src-tauri/src/commands/models/validator.rs",
            &[
                "Tauri-side model prevalidation contract",
                "provider-name emptiness contract",
            ][..],
        ),
        (
            "src-tauri/src/commands/models/formatter.rs",
            &[
                "provider-aware model TOML rendering contract",
                "model command IO error-string contract",
            ][..],
        ),
        (
            "src-tauri/src/commands/pools/mod.rs",
            &[
                "PoolSummary serialization contract",
                "frontend pool-list DTO",
            ][..],
        ),
        (
            "src-tauri/src/commands/pools/update.rs",
            &["Tauri IPC pool command contract", "pool rewrite lifecycle"][..],
        ),
        (
            "src-tauri/src/commands/pools/derive.rs",
            &[
                "ProviderConfig.name pool grouping contract",
                "sorted/deduplicated command-set contract",
            ][..],
        ),
        (
            "src-tauri/src/commands/pools/accessor.rs",
            &[
                "AppState pool model-cache mutex contract",
                "pool model cache update contract",
            ][..],
        ),
        (
            "src-tauri/src/commands/pools/validator.rs",
            &[
                "pool command-set validation contract",
                "zero-provider prevention contract",
            ][..],
        ),
        (
            "src-tauri/src/commands/pools/writer.rs",
            &[
                "provider-aware pool TOML rendering contract",
                "pool model file write contract",
            ][..],
        ),
        (
            "src-tauri/src/commands/quota_refresh/mod.rs",
            &[
                "QuotaRefreshEntry serialization contract",
                "QuotaRefreshWindow serialization contract",
                "frontend quota-refresh DTO compatibility contract",
            ][..],
        ),
        (
            "src-tauri/src/commands/quota_refresh/orchestration.rs",
            &[
                "Tauri IPC quota-refresh command contract",
                "runtime quota service orchestration contract",
            ][..],
        ),
        (
            "src-tauri/src/commands/quota_refresh/candidates.rs",
            &[
                "multi-provider quota-refresh candidate contract",
                "sorted provider-name output contract",
                "deduplicated provider-name output contract",
            ][..],
        ),
        (
            "src-tauri/src/commands/quota_refresh/accessor.rs",
            &[
                "providers.toml parent-path contract",
                "state.db parent-path contract",
                "quota service request contract",
                "quota staleness predicate contract",
            ][..],
        ),
        (
            "src-tauri/src/commands/quota_refresh/mapper.rs",
            &[
                "quota refresh outcome to QuotaRefreshEntry DTO contract",
                "in-flight status string wire contract",
                "quota window timestamp string wire contract",
            ][..],
        ),
        (
            "src-tauri/src/commands/accessor.rs",
            &[
                "AppState setup repository access contract",
                "test SetupRepository injection preference contract",
            ][..],
        ),
        (
            "src-tauri/src/commands/setup_flow/mod.rs",
            &[
                "Tauri IPC setup-flow command contract",
                "setup session id string contract",
            ][..],
        ),
        (
            "src-tauri/src/commands/setup_flow/orchestration.rs",
            &[
                "Tauri IPC setup-flow command contract",
                "setup input channel lifecycle contract",
            ][..],
        ),
        (
            "src-tauri/src/commands/setup_flow/provider_probe.rs",
            &[
                "std::process::Command host-command probe contract",
                "which executable lookup contract",
                "claude provider availability invocation contract",
                "setup-needed boolean contract",
            ][..],
        ),
        (
            "src-tauri/src/commands/setup_flow/accessor.rs",
            &[
                "AppState model-cache mutex contract",
                "setup input sender storage contract",
            ][..],
        ),
        (
            "src-tauri/src/commands/setup_flow/formatter.rs",
            &[
                "setup memory-open error event string contract",
                "setup response send-error string contract",
            ][..],
        ),
        (
            "src-tauri/src/commands/providers_accounts/mod.rs",
            &[
                "AddAccountInput deserialization contract",
                "AddAccountInput field-name wire contract",
            ][..],
        ),
        (
            "src-tauri/src/commands/providers_accounts/orchestration.rs",
            &[
                "Tauri IPC provider/account command contract",
                "provider sync detection delegation contract",
            ][..],
        ),
        (
            "src-tauri/src/commands/providers_accounts/accessor.rs",
            &[
                "SetupRepository provider/account read contract",
                "SetupRepository account mutation contract",
            ][..],
        ),
        (
            "src-tauri/src/commands/providers_accounts/validator.rs",
            &[
                "AddAccountInput emptiness validation contract",
                "account validation error string contract",
            ][..],
        ),
        (
            "src-tauri/src/commands/providers_accounts/mapper.rs",
            &[
                "AddAccountInput to AccountRecord mapping contract",
                "RFC3339 timestamp field contract",
            ][..],
        ),
        (
            "src-tauri/src/commands/providers_accounts/formatter.rs",
            &["provider-not-found error string contract"][..],
        ),
        (
            "src-tauri/src/commands/providers_accounts/display_name.rs",
            &["provider CLI name to display-name residual contract"][..],
        ),
        (
            "src-tauri/src/commands/test_model/diagnostics_fallback.rs",
            &[
                "terminal-text diagnostics-fallback decision contract",
                "local diagnostic-input duplicate contract",
                "diagnostics classify-exhaustion request contract",
                "fallback disposition result contract",
            ][..],
        ),
        (
            "src-tauri/src/commands/discovery/mod.rs",
            &[
                "Tauri IPC discovery command contract",
                "discovered model DTO wire contract",
            ][..],
        ),
        (
            "src-tauri/src/commands/discovery/orchestration.rs",
            &[
                "Tauri IPC discovery command contract",
                "discovery persistence ordering contract",
            ][..],
        ),
        (
            "src-tauri/src/commands/discovery/accessor.rs",
            &[
                "runtime discovery invocation contract",
                "SetupRepository discovery persistence/read contract",
            ][..],
        ),
        (
            "src-tauri/src/commands/discovery/predicate.rs",
            &["non-empty discovery stale-delete guard contract"][..],
        ),
        (
            "src-tauri/src/commands/discovery/formatter.rs",
            &["discovery blocking task join-error string contract"][..],
        ),
    ] {
        assert_carrier(file_path, "## Adapter declarations", snippets);
    }
}
