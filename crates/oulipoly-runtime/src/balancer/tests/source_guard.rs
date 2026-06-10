//! ## Declared roles
//!
//! `orchestration, accessor`.

use super::source_text::*;
use super::*;

#[test]
fn age153_source_guard_balancer_has_no_terminal_signal_or_provider_output_authority() {
    for (module_path, source) in crate::balancer::balancer_production_sources() {
        let source = source_without_comments(source);
        for forbidden in ["TerminalSignal", "TerminalSignalKind", "terminal_signal"] {
            assert!(
                !contains_identifier_token(&source, forbidden),
                "{module_path} must not reference terminal-signal identifier token {forbidden:?}; AGE-153 routing authority is provider_quotas.exhausted_at"
            );
        }
        assert!(
            !contains_terminal_signal_use_import(&source),
            "{module_path} must not import terminal_signal modules or TerminalSignal types"
        );
        assert!(
            !contains_provider_output_parser_identifier(&source),
            "{module_path} must not call provider-output parser functions as routing authority"
        );
    }
}

// risk: Inline AGE-153 guard could false-green after B4 modules are extracted; level: component/structural; source: AGE-225 contract Guard And Spec Contract.
#[test]
fn age153_inline_guard_source_list_declares_age225_b4_modules() {
    let guard_body = balancer_source_list_body(
        include_str!("../mod.rs"),
        "fn balancer_production_sources() -> [",
        "fn production_balancer_source(",
    );
    assert_age225_b4_balancer_modules_are_declared("inline AGE-153 guard", guard_body);
}
#[test]
fn age153_decide_migration_observes_exhausted_at_without_terminal_signal_dependency() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = migratable_model(&[
        ("alpha-age153-a", "project_storage"),
        ("alpha-age153-b", "project_storage"),
    ]);
    seed_windows_with_deltas(&db, "alpha-age153-a", &[(0.20, 5, 0.01, 22)]);
    seed_windows_with_deltas(&db, "alpha-age153-b", &[(0.30, 5, 0.01, 22)]);
    db.mark_exhausted("alpha-age153-a").unwrap();

    let decision = decide_migration(&db, &model, &resolved_for(&model, 0), None).unwrap();

    assert_eq!(
        decision,
        MigrationDecision::Migrate {
            target_provider_index: 1,
            reason: TransitionReason::Exhausted,
        }
    );
}
