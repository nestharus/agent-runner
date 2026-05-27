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
        (
            "src-tauri/src/lib.rs",
            &["orchestration", "mapper", "predicate", "formatter"][..],
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
            "src-tauri/src/lib.rs",
            &[
                "test_model",
                "Tauri IPC",
                "executor service output",
                "provider quota mutation",
                "TestModelResult",
            ][..],
        ),
    ] {
        assert_carrier(file_path, "## Adapter declarations", snippets);
    }
}
