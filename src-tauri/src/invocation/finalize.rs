//! ## Declared roles
//!
//! `orchestration`, `accessor`, `predicate`, `formatter`, `validator`

use oulipoly_runtime::services::ServiceError;
#[cfg(test)]
use oulipoly_runtime::services::{
    InvocationLifecycleFinalizeRequest, InvocationLifecycleServicePort,
    ProductionInvocationLifecycleService,
};
use oulipoly_state::StateDb;

pub(crate) struct FinalizerGuard<'a> {
    db: &'a StateDb,
    invocation_id: i64,
    finalized: bool,
}

impl<'a> FinalizerGuard<'a> {
    pub(crate) fn new(db: &'a StateDb, invocation_id: i64) -> Self {
        finalizer_guard(db, invocation_id, false)
    }

    pub(crate) fn mark_finalized(&mut self) {
        self.finalized = true;
    }

    pub(crate) fn preserve_running_after_process_integrity(&mut self, error: &ServiceError) {
        let preserves_running = match error {
            ServiceError::Contention { .. } => true,
            ServiceError::Dependency { message } => message.starts_with("process_integrity:"),
            _ => false,
        };
        if preserves_running {
            self.finalized = true;
        }
    }
}

fn finalizer_guard<'a>(db: &'a StateDb, invocation_id: i64, finalized: bool) -> FinalizerGuard<'a> {
    FinalizerGuard {
        db,
        invocation_id,
        finalized,
    }
}

impl Drop for FinalizerGuard<'_> {
    fn drop(&mut self) {
        finalize_guard_on_drop(self);
    }
}

fn finalize_guard_on_drop(guard: &FinalizerGuard<'_>) {
    if should_skip_guard_drop_finalize(guard.finalized) {
        return;
    }
    finalize_unfinalized_guard_invocation(guard);
}

fn should_skip_guard_drop_finalize(finalized: bool) -> bool {
    finalized
}

fn finalize_unfinalized_guard_invocation(guard: &FinalizerGuard<'_>) {
    // Source guard marker: self.db.finalize_invocation(
    emit_guard_finalize_failure(finalize_invocation_from_guard(
        guard.db,
        guard.invocation_id,
    ));
}

fn emit_guard_finalize_failure(result: Result<(), String>) {
    result.unwrap_or_else(|err| emit_finalizer_guard_warning(&err));
}

fn finalize_invocation_from_guard(db: &StateDb, invocation_id: i64) -> Result<(), String> {
    db.finalize_invocation(
        invocation_id,
        false,
        -1,
        Some("guard_drop"),
        Some("guard_drop"),
    )
}

fn emit_finalizer_guard_warning(err: &str) {
    eprintln!("Warning: Failed to finalize invocation in guard: {err}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use oulipoly_state::mailbox::{CompletionEventRegistrationInput, MailboxDb};
    use oulipoly_state::{InvocationStart, InvocationStatus};
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::path::Path;
    use uuid::Uuid;

    fn test_db() -> StateDb {
        open_test_db(Path::new(":memory:"))
    }

    fn open_test_db(path: &Path) -> StateDb {
        StateDb::open(path).unwrap()
    }

    #[test]
    fn finalizer_guard_mark_finalized_makes_drop_a_no_op() {
        let db = test_db();
        let start = InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "fixture-model".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        };
        let invocation_id = db.start_invocation(&start).unwrap();

        {
            let mut guard = FinalizerGuard::new(&db, invocation_id);
            db.finalize_invocation(invocation_id, true, 0, None, None)
                .unwrap();
            guard.mark_finalized();
        }

        let row = db
            .get_invocation_by_uuid(&start.invocation_uuid)
            .unwrap()
            .unwrap();
        assert_eq!(row.status, InvocationStatus::Succeeded);
        assert_eq!(row.success, Some(true));
        assert_eq!(row.exit_code, Some(0));
    }

    // RISK: FinalizerGuard panic/drop fallback could leave terminal_reason null while setting guard_drop error_category (proposal §test-intent "FinalizerGuard panic-path characterization", assumption A4)
    // LEVEL: unit
    // SOURCE: contracts/nes-250-contract.md § Test catalog § Finalize cascade (T-FINAL-GUARD)
    #[test]
    fn finalizer_guard_drop_finalizes_failed_row_during_panic_unwind() {
        // CHARACTERIZATION: T-FINAL-GUARD writes error_category=guard_drop and terminal_reason=guard_drop.
        let db = test_db();
        let start = InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "fixture-model".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        };
        let invocation_id = db.start_invocation(&start).unwrap();

        let panic_result = catch_unwind(AssertUnwindSafe(|| {
            let _guard = FinalizerGuard::new(&db, invocation_id);
            panic!("force guard drop");
        }));
        assert!(panic_result.is_err());

        let row = db
            .get_invocation_by_uuid(&start.invocation_uuid)
            .unwrap()
            .unwrap();
        assert_eq!(row.status, InvocationStatus::Failed);
        assert_eq!(row.success, Some(false));
        assert_eq!(row.exit_code, Some(-1));
        assert_eq!(row.error_category.as_deref(), Some("guard_drop"));
        assert_eq!(row.terminal_reason.as_deref(), Some("guard_drop"));
    }

    #[test]
    fn finalizer_guard_drop_is_no_op_after_explicit_spawn_error_finalize() {
        let db = test_db();
        let start = InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "fixture-model".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        };
        let invocation_id = db.start_invocation(&start).unwrap();

        {
            let mut guard = FinalizerGuard::new(&db, invocation_id);
            db.finalize_invocation(
                invocation_id,
                false,
                1,
                Some("spawn_error"),
                Some("spawn failed"),
            )
            .unwrap();
            guard.mark_finalized();
        }

        let row = db
            .get_invocation_by_uuid(&start.invocation_uuid)
            .unwrap()
            .unwrap();
        assert_eq!(row.status, InvocationStatus::Failed);
        assert_eq!(row.success, Some(false));
        assert_eq!(row.exit_code, Some(1));
        assert_eq!(row.error_category.as_deref(), Some("spawn_error"));
    }

    #[test]
    fn finalizer_guard_preserves_running_row_after_process_integrity_failure() {
        let db = test_db();
        let start = InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "fixture-model".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        };
        let invocation_id = db.start_invocation(&start).unwrap();

        {
            let mut guard = FinalizerGuard::new(&db, invocation_id);
            guard.preserve_running_after_process_integrity(&ServiceError::Dependency {
                message: "process_integrity: completion sidecar authority is unavailable"
                    .to_string(),
            });
        }

        let row = db
            .get_invocation_by_uuid(&start.invocation_uuid)
            .unwrap()
            .unwrap();
        assert_eq!(row.status, InvocationStatus::Running);
        assert_eq!(row.success, None);
        assert_eq!(row.exit_code, None);
    }

    #[test]
    fn finalizer_guard_preserves_running_after_completion_authority_contention() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("state.db");
        let sidecar_path = MailboxDb::path_for_state_db(&state_path);
        let mut db = open_test_db(&state_path);
        let start = InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "fixture-model".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        };
        let invocation_id = db.start_invocation(&start).unwrap();
        db.register_completion_event_with_obligation(
            "finalizer-guard-contention-admission",
            CompletionEventRegistrationInput {
                event_id: "finalizer-guard-contention-event",
                delivery_mode: "async",
                owner_session_id: Some("finalizer-guard-contention-session"),
                owner_invocation_uuid: Some(&start.invocation_uuid),
                state_dir: "/tmp/finalizer-guard-contention-state",
                meta_path: "/tmp/finalizer-guard-contention-meta",
                log_path: "/tmp/finalizer-guard-contention-log",
                rc_path: "/tmp/finalizer-guard-contention-rc",
            },
        )
        .unwrap();
        let mut authority_path = sidecar_path.as_os_str().to_os_string();
        authority_path.push(".authority.lock");
        let authority_file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(std::path::PathBuf::from(authority_path))
            .unwrap();
        <std::fs::File as fs4::FileExt>::lock(&authority_file).unwrap();

        let started = std::time::Instant::now();
        {
            let mut guard = FinalizerGuard::new(&db, invocation_id);
            let result = ProductionInvocationLifecycleService.finalize_invocation(
                InvocationLifecycleFinalizeRequest {
                    state: &db,
                    invocation_row_id: invocation_id,
                    success: true,
                    exit_code: 0,
                    error_category: None,
                    terminal_reason: None,
                },
            );
            let error = match result {
                Err(error @ ServiceError::Contention { .. }) => error,
                other => panic!("expected typed contention, got {other:?}"),
            };
            let message = error.to_string();
            assert!(started.elapsed() < std::time::Duration::from_secs(7));
            assert!(
                message.starts_with("process_integrity: completion_authority_contention:"),
                "{message}"
            );
            guard.preserve_running_after_process_integrity(&error);
        }

        let row = db
            .get_invocation_by_uuid(&start.invocation_uuid)
            .unwrap()
            .unwrap();
        assert_eq!(row.status, InvocationStatus::Running);
        assert_eq!(row.success, None);
        assert_eq!(row.exit_code, None);
        assert_eq!(row.error_category, None);
        assert_eq!(row.terminal_reason, None);

        <std::fs::File as fs4::FileExt>::unlock(&authority_file).unwrap();
        drop(authority_file);
        db.finalize_invocation(invocation_id, true, 0, None, None)
            .unwrap();
        assert_eq!(
            db.get_invocation_by_uuid(&start.invocation_uuid)
                .unwrap()
                .unwrap()
                .status,
            InvocationStatus::Succeeded
        );
    }

    #[test]
    fn finalizer_guard_still_finalizes_after_an_unrelated_dependency_failure() {
        let db = test_db();
        let start = InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "fixture-model".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        };
        let invocation_id = db.start_invocation(&start).unwrap();

        {
            let mut guard = FinalizerGuard::new(&db, invocation_id);
            guard.preserve_running_after_process_integrity(&ServiceError::Dependency {
                message: "completion sidecar is temporarily unavailable".to_string(),
            });
        }

        let row = db
            .get_invocation_by_uuid(&start.invocation_uuid)
            .unwrap()
            .unwrap();
        assert_eq!(row.status, InvocationStatus::Failed);
        assert_eq!(row.success, Some(false));
        assert_eq!(row.exit_code, Some(-1));
        assert_eq!(row.error_category.as_deref(), Some("guard_drop"));
        assert_eq!(row.terminal_reason.as_deref(), Some("guard_drop"));
    }
}
