//! ## Declared roles
//!
//! - formatter
//! - mapper
//! - orchestration
//! - predicate
//!
//! Role set: { formatter, mapper, orchestration, predicate }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/invocation_lifecycle_finalize.rs
//!     role: intrinsic-surface
//!     Domain: invocation-lifecycle-finalize-persistence
//!     Owns:
//!       - StateDb invocation-lifecycle-finalize persistence surface: the StateDb methods, owned
//!         tables/rows, and SQL this concern extends, split out of the StateDb
//!         facade by the WU #65 decomposition with the public API preserved
//!       - Intrinsic StateDb/rusqlite carriers and concern-owned DTOs referenced
//!         via `use super::*`, subordinate to this domain: FinalizeInvocationRow, FinalizeLifecycleInput, OperationResult, StateDb, lc_log_adapter, lifecycle_terminal_status, sqlite
//!       - external contract symbols referenced by this concern via its `use`
//!         declarations, intrinsic and subordinate to this persistence domain: ResultEnvelopeFailureIdentity, ResultEnvelopeInput
//! ```
//!
//! Invocation finalize orchestration and lifecycle-log classification.

use super::*;
use crate::result_envelope::{ResultEnvelopeFailureIdentity, ResultEnvelopeInput};

struct FinalizeInvocationWrite<'a> {
    exit_code: i32,
    error_category: Option<&'a str>,
    terminal_reason: Option<&'a str>,
    finished_at: &'a str,
}

impl StateDb {
    pub fn finalize_invocation(
        &self,
        id: i64,
        success: bool,
        exit_code: i32,
        error_category: Option<&str>,
        terminal_reason: Option<&str>,
    ) -> Result<(), String> {
        let lifecycle_row = self.lifecycle_context_for_row_or_none(id);
        let timer = lc_log_adapter::start_timer();
        let finished_at = Self::current_rfc3339_timestamp();
        let transaction_result = self.finalize_invocation_transaction(
            id,
            success,
            exit_code,
            error_category,
            terminal_reason,
            &finished_at,
        );
        self.report_finalize_invocation(
            id,
            success,
            exit_code,
            error_category,
            terminal_reason,
            &finished_at,
            lifecycle_row.as_ref(),
            timer,
            transaction_result,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn report_finalize_invocation(
        &self,
        id: i64,
        success: bool,
        exit_code: i32,
        error_category: Option<&str>,
        terminal_reason: Option<&str>,
        finished_at: &str,
        lifecycle_row: Option<&LifecycleInvocationRow>,
        timer: std::time::Instant,
        transaction_result: Result<FinalizeInvocationRow, String>,
    ) -> Result<(), String> {
        self.warn_result_artifact_for_finalize_result(
            success,
            exit_code,
            error_category,
            terminal_reason,
            finished_at,
            &transaction_result,
        );
        let result = Self::translate_finalize_invocation_result(transaction_result);
        let finalize_success = Self::is_finalize_result_success(&result);
        let sqlite_error = Self::is_finalize_sqlite_error(id, lifecycle_row, &result);
        let operation_result =
            Self::classify_finalize_operation_result(finalize_success, sqlite_error);
        let terminal_status = Self::format_terminal_status(success, exit_code, terminal_reason);
        let input = Self::finalize_lifecycle_input(
            &terminal_status,
            exit_code,
            error_category,
            terminal_reason,
            operation_result,
        );
        let context = self.finalize_context(id, lifecycle_row, input);
        lc_log_adapter::emit_finalize(
            &self.lifecycle_sink,
            timer,
            context,
            &result,
            terminal_status,
        );
        result
    }

    pub(super) fn warn_result_artifact_for_finalize_result(
        &self,
        success: bool,
        exit_code: i32,
        error_category: Option<&str>,
        terminal_reason: Option<&str>,
        finished_at: &str,
        result: &Result<FinalizeInvocationRow, String>,
    ) {
        if let Ok(invocation) = result {
            let failure_identity =
                self.finalize_result_artifact_failure_identity(success, invocation);
            let input = Self::map_finalize_result_envelope_input(
                invocation,
                success,
                exit_code,
                error_category,
                terminal_reason,
                finished_at,
                failure_identity.as_ref(),
            );
            self.warn_result_artifact_failure(input);
        }
    }

    fn finalize_result_artifact_failure_identity(
        &self,
        success: bool,
        invocation: &FinalizeInvocationRow,
    ) -> Option<ResultEnvelopeFailureIdentity> {
        (!success).then(|| self.result_artifact_failure_identity(invocation))
    }

    fn map_finalize_result_envelope_input<'a>(
        invocation: &'a FinalizeInvocationRow,
        success: bool,
        exit_code: i32,
        error_category: Option<&'a str>,
        terminal_reason: Option<&'a str>,
        finished_at: &'a str,
        failure_identity: Option<&'a ResultEnvelopeFailureIdentity>,
    ) -> ResultEnvelopeInput<'a> {
        ResultEnvelopeInput {
            id: &invocation.invocation_uuid,
            success,
            exit_code,
            error_category,
            terminal_reason,
            finished_at,
            failure_identity,
        }
    }

    pub(super) fn translate_finalize_invocation_result(
        result: Result<FinalizeInvocationRow, String>,
    ) -> Result<(), String> {
        result.map(|_| ())
    }

    pub(super) fn is_finalize_result_success(result: &Result<(), String>) -> bool {
        result.is_ok()
    }

    pub(super) fn is_finalize_sqlite_error(
        id: i64,
        lifecycle_row: Option<&LifecycleInvocationRow>,
        result: &Result<(), String>,
    ) -> bool {
        result.as_ref().err().is_some_and(|message| {
            !Self::is_finalize_context_resolution_error(id, lifecycle_row, message)
        })
    }

    pub(super) fn is_finalize_context_resolution_error(
        id: i64,
        lifecycle_row: Option<&LifecycleInvocationRow>,
        message: &str,
    ) -> bool {
        lifecycle_row.is_none() && Self::is_invocation_not_found_error(id, message)
    }

    pub(super) fn finalize_lifecycle_input<'a>(
        terminal_status_attempt: &'a str,
        exit_code: i32,
        error_category: Option<&'a str>,
        terminal_reason: Option<&'a str>,
        operation_result: OperationResult,
    ) -> FinalizeLifecycleInput<'a> {
        FinalizeLifecycleInput {
            terminal_status_attempt,
            exit_code,
            error_category,
            terminal_reason,
            operation_result,
        }
    }

    pub(super) fn format_terminal_status(
        success: bool,
        _exit_code: i32,
        _terminal_reason: Option<&str>,
    ) -> String {
        lifecycle_terminal_status(success).to_string()
    }

    pub(super) fn finalize_invocation_transaction(
        &self,
        id: i64,
        success: bool,
        exit_code: i32,
        error_category: Option<&str>,
        terminal_reason: Option<&str>,
        finished_at: &str,
    ) -> Result<FinalizeInvocationRow, String> {
        self.finalize_invocation_transaction_on(
            id,
            success,
            FinalizeInvocationWrite {
                exit_code,
                error_category,
                terminal_reason,
                finished_at,
            },
            || {},
            || {},
        )
    }

    fn finalize_invocation_transaction_on<BeforeValidation, AfterValidation>(
        &self,
        id: i64,
        success: bool,
        write: FinalizeInvocationWrite<'_>,
        before_validation: BeforeValidation,
        after_validation: AfterValidation,
    ) -> Result<FinalizeInvocationRow, String>
    where
        BeforeValidation: FnOnce(),
        AfterValidation: FnOnce(),
    {
        let tx =
            sqlite::Transaction::new_unchecked(&self.conn, sqlite::TransactionBehavior::Immediate)
                .map_err(Self::format_begin_transaction_error)?;

        let invocation = Self::load_invocation_for_finalize(&tx, id)?;
        Self::validate_invocation_is_running(id, &invocation.status)?;
        let obligations = success
            .then(|| {
                Self::completion_obligations_for_invocation_on(&tx, &invocation.invocation_uuid)
            })
            .transpose()
            .map_err(|error| {
                Self::format_completion_authority_storage_error(&invocation.invocation_uuid, error)
            })?
            .unwrap_or_default();
        let completion_authority_state_path = if obligations.is_empty() {
            self.db_path.as_path()
        } else {
            self.completion_authority_state_path().ok_or_else(|| {
                format!(
                    "process_integrity: invocation {} has admitted completion authority but the state database no longer has an absolute, non-symlink, single-link local identity",
                    invocation.invocation_uuid
                )
            })?
        };
        let sidecar_path =
            crate::mailbox::MailboxDb::path_for_state_db(completion_authority_state_path);
        let sidecar_authority = (!obligations.is_empty())
            .then(|| {
                Self::acquire_finalize_sidecar_authority(
                    &sidecar_path,
                    &invocation.invocation_uuid,
                    &obligations,
                )
            })
            .transpose()?;
        let mut sidecar = self.open_completion_authority_sidecar(
            sidecar_authority.as_ref(),
            &invocation.invocation_uuid,
            &obligations,
        )?;
        let sidecar_fence = sidecar
            .as_mut()
            .map(crate::mailbox::MailboxDb::begin_completion_authority_fence)
            .transpose()
            .map_err(|error| {
                Self::format_unreadable_completion_sidecar(
                    &invocation.invocation_uuid,
                    &obligations,
                    error,
                )
            })?;
        if let Some(sidecar_fence) = sidecar_fence.as_ref() {
            self.validate_completion_sidecar_authority(
                sidecar_fence,
                &invocation.invocation_uuid,
                &obligations,
            )?;
        }
        Self::write_invocation_final_row(
            &tx,
            id,
            success,
            write.exit_code,
            write.error_category,
            write.terminal_reason,
            write.finished_at,
        )?;
        Self::upsert_provider_finalize_aggregate(
            &tx,
            &invocation.model_name,
            invocation.provider_name.as_deref(),
            success,
            write.terminal_reason,
            write.finished_at,
        )?;

        before_validation();
        if let Some(sidecar_fence) = sidecar_fence.as_ref() {
            self.validate_completion_sidecar_authority(
                sidecar_fence,
                &invocation.invocation_uuid,
                &obligations,
            )?;
        }
        after_validation();

        tx.commit().map_err(Self::format_commit_transaction_error)?;
        Ok(invocation)
    }

    fn acquire_finalize_sidecar_authority(
        sidecar_path: &std::path::Path,
        invocation_uuid: &str,
        obligations: &[CompletionObligationExpectation],
    ) -> Result<crate::mailbox::MailboxAuthorityFence, String> {
        match crate::mailbox::MailboxAuthorityFence::acquire(sidecar_path) {
            Ok(authority) => Ok(authority),
            Err(error @ crate::mailbox::MailboxAuthorityFenceError::Timeout { .. }) => Err(
                Self::format_completion_sidecar_contention(invocation_uuid, obligations, error),
            ),
            Err(error) => Err(Self::format_unreadable_completion_sidecar(
                invocation_uuid,
                obligations,
                error.to_string(),
            )),
        }
    }

    fn open_completion_authority_sidecar(
        &self,
        authority: Option<&crate::mailbox::MailboxAuthorityFence>,
        invocation_uuid: &str,
        obligations: &[CompletionObligationExpectation],
    ) -> Result<Option<crate::mailbox::MailboxDb>, String> {
        if obligations.is_empty() {
            return Ok(None);
        }
        let authority = authority.ok_or_else(|| {
            format!(
                "process_integrity: invocation {invocation_uuid} has completion obligations but no sidecar authority"
            )
        })?;
        let sidecar_path = authority.path();
        if !sidecar_path.exists() {
            return Err(Self::format_missing_completion_sidecar(
                invocation_uuid,
                obligations,
            ));
        }
        crate::mailbox::MailboxDb::open_existing_for_completion_authority(authority)
            .map(Some)
            .map_err(|error| {
                Self::format_unreadable_completion_sidecar(invocation_uuid, obligations, error)
            })
    }

    fn validate_completion_sidecar_authority(
        &self,
        sidecar: &crate::mailbox::CompletionAuthorityFence<'_>,
        invocation_uuid: &str,
        obligations: &[CompletionObligationExpectation],
    ) -> Result<(), String> {
        let observed_generation = sidecar.sidecar_generation().map_err(|error| {
            Self::format_unreadable_completion_sidecar(invocation_uuid, obligations, error)
        })?;
        if let Some(mismatch) = obligations
            .iter()
            .find(|obligation| obligation.expected_sidecar_generation != observed_generation)
        {
            return Err(format!(
                "process_integrity: invocation {invocation_uuid} cannot succeed because completion obligation {} owned by {} expects mailbox sidecar generation {}, observed {}",
                mismatch.admission_id,
                mismatch.owner_invocation_uuid,
                mismatch.expected_sidecar_generation,
                observed_generation,
            ));
        }
        for obligation in obligations {
            let present = sidecar
                .contains_completion_obligation(
                    &obligation.event_id,
                    &obligation.owner_invocation_uuid,
                    &obligation.owner_session_id,
                )
                .map_err(|error| {
                    Self::format_unreadable_completion_sidecar(invocation_uuid, obligations, error)
                })?;
            if !present {
                return Err(format!(
                    "process_integrity: invocation {invocation_uuid} cannot succeed because completion obligation {} owned by {} expects event {} in mailbox sidecar generation {}, but the event listener is absent",
                    obligation.admission_id,
                    obligation.owner_invocation_uuid,
                    obligation.event_id,
                    obligation.expected_sidecar_generation,
                ));
            }
        }
        Ok(())
    }

    fn format_completion_authority_storage_error(
        invocation_uuid: &str,
        error: OwnershipAuthorityError,
    ) -> String {
        format!(
            "process_integrity: failed to resolve completion authority for invocation {invocation_uuid}: {error}"
        )
    }

    fn format_missing_completion_sidecar(
        invocation_uuid: &str,
        obligations: &[CompletionObligationExpectation],
    ) -> String {
        let expectation = &obligations[0];
        format!(
            "process_integrity: invocation {invocation_uuid} cannot succeed because completion obligation {} owned by {} expects mailbox sidecar generation {}, but the sidecar is missing",
            expectation.admission_id,
            expectation.owner_invocation_uuid,
            expectation.expected_sidecar_generation,
        )
    }

    fn format_unreadable_completion_sidecar(
        invocation_uuid: &str,
        obligations: &[CompletionObligationExpectation],
        error: String,
    ) -> String {
        let expectation = &obligations[0];
        format!(
            "process_integrity: invocation {invocation_uuid} cannot succeed because completion obligation {} owned by {} expects mailbox sidecar generation {}, but sidecar authority is unavailable: {error}",
            expectation.admission_id,
            expectation.owner_invocation_uuid,
            expectation.expected_sidecar_generation,
        )
    }

    fn format_completion_sidecar_contention(
        invocation_uuid: &str,
        obligations: &[CompletionObligationExpectation],
        error: crate::mailbox::MailboxAuthorityFenceError,
    ) -> String {
        let expectation = &obligations[0];
        format!(
            "process_integrity: completion_authority_contention: invocation {invocation_uuid} could not acquire mailbox sidecar authority for completion obligation {} owned by {}: {error}",
            expectation.admission_id, expectation.owner_invocation_uuid,
        )
    }

    pub(super) fn format_begin_transaction_error(err: sqlite::Error) -> String {
        format!("Failed to begin invocation finalize tx: {err}")
    }

    pub(super) fn format_commit_transaction_error(err: sqlite::Error) -> String {
        format!("Failed to commit invocation finalize tx: {err}")
    }

    pub(super) fn classify_finalize_operation_result(
        success: bool,
        sqlite_error: bool,
    ) -> OperationResult {
        if success {
            lc_log_adapter::finalize_operation_result(true, false)
        } else {
            lc_log_adapter::finalize_operation_result(false, sqlite_error)
        }
    }

    pub(super) fn is_invocation_not_found_error(id: i64, message: &str) -> bool {
        message == Self::format_invocation_not_found_error(id)
    }

    pub(super) fn format_invocation_not_found_error(id: i64) -> String {
        format!("Invocation {id} not found")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mailbox::{CompletionEventRegistrationInput, MailboxDb};
    use crate::{CompletionObligationAdmission, InvocationStart};
    use std::sync::mpsc;
    use std::time::Duration;

    const INVOCATION_UUID: &str = "77777777-7777-4777-8777-777777777777";
    const EVENT_ID: &str = "age299-s2-finalize-fence-event";
    const SESSION_ID: &str = "age299-s2-finalize-fence-session";

    fn state_with_completion_obligation() -> (tempfile::TempDir, StateDb, i64, std::path::PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("state.db");
        let sidecar_path = MailboxDb::path_for_state_db(&state_path);
        let mut state = StateDb::open(&state_path).unwrap();
        let invocation_row_id = state
            .start_invocation(&InvocationStart {
                invocation_uuid: INVOCATION_UUID.to_string(),
                model_name: "age299-s2".to_string(),
                provider_name: "test-provider".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        state
            .register_completion_event_with_obligation(
                "age299-s2-finalize-contention-admission",
                CompletionEventRegistrationInput {
                    event_id: EVENT_ID,
                    delivery_mode: "async",
                    owner_session_id: Some(SESSION_ID),
                    owner_invocation_uuid: Some(INVOCATION_UUID),
                    state_dir: "/tmp/age299-s2-finalize-contention-state",
                    meta_path: "/tmp/age299-s2-finalize-contention-meta",
                    log_path: "/tmp/age299-s2-finalize-contention-log",
                    rc_path: "/tmp/age299-s2-finalize-contention-rc",
                },
            )
            .unwrap();
        (directory, state, invocation_row_id, sidecar_path)
    }

    #[test]
    fn finalization_waits_within_one_sidecar_authority_budget() {
        let (_directory, state, invocation_row_id, sidecar_path) =
            state_with_completion_obligation();
        let authority = crate::mailbox::MailboxAuthorityFence::acquire(&sidecar_path).unwrap();
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            drop(authority);
        });

        state
            .finalize_invocation(invocation_row_id, true, 0, None, None)
            .unwrap();
        releaser.join().unwrap();
        assert_eq!(
            state
                .get_invocation_by_uuid(INVOCATION_UUID)
                .unwrap()
                .unwrap()
                .status,
            InvocationStatus::Succeeded
        );
    }

    #[test]
    fn exhausted_sidecar_authority_contention_is_distinct_from_identity_failure() {
        let (_directory, state, invocation_row_id, sidecar_path) =
            state_with_completion_obligation();
        let _authority = crate::mailbox::MailboxAuthorityFence::acquire(&sidecar_path).unwrap();

        let error = state
            .finalize_invocation(invocation_row_id, true, 0, None, None)
            .unwrap_err();

        assert!(
            error.starts_with("process_integrity: completion_authority_contention:"),
            "{error}"
        );
        assert!(
            !error.contains("sidecar authority is unavailable"),
            "{error}"
        );
        assert_eq!(
            state
                .get_invocation_by_uuid(INVOCATION_UUID)
                .unwrap()
                .unwrap()
                .status,
            InvocationStatus::Running
        );
    }

    #[test]
    fn sidecar_namespace_mutation_after_validation_linearizes_after_state_commit() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("state.db");
        let sidecar_path = MailboxDb::path_for_state_db(&state_path);
        let renamed_sidecar_path = directory.path().join("pid-identity.renamed");
        let state = StateDb::open(&state_path).unwrap();
        let invocation_row_id = state
            .start_invocation(&InvocationStart {
                invocation_uuid: INVOCATION_UUID.to_string(),
                model_name: "age299-s2".to_string(),
                provider_name: "test-provider".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        let mut sidecar = MailboxDb::open(&sidecar_path).unwrap();
        let generation = sidecar.sidecar_generation().unwrap();
        sidecar
            .register_completion_event(CompletionEventRegistrationInput {
                event_id: EVENT_ID,
                delivery_mode: "async",
                owner_session_id: Some(SESSION_ID),
                owner_invocation_uuid: Some(INVOCATION_UUID),
                state_dir: "/tmp/age299-s2-finalize-fence-state",
                meta_path: "/tmp/age299-s2-finalize-fence-meta",
                log_path: "/tmp/age299-s2-finalize-fence-log",
                rc_path: "/tmp/age299-s2-finalize-fence-rc",
            })
            .unwrap();
        drop(sidecar);
        state
            .record_completion_obligation(CompletionObligationAdmission {
                admission_id: "age299-s2-finalize-fence-admission",
                invocation_uuid: INVOCATION_UUID,
                event_id: EVENT_ID,
                owner_invocation_uuid: INVOCATION_UUID,
                owner_session_id: SESSION_ID,
                expected_sidecar_generation: &generation,
            })
            .unwrap();

        let (start_mutation_tx, start_mutation_rx) = mpsc::channel();
        let (mutation_started_tx, mutation_started_rx) = mpsc::channel();
        let (authority_acquired_tx, authority_acquired_rx) = mpsc::channel();
        let mutator_sidecar_path = sidecar_path.clone();
        let mutator_renamed_path = renamed_sidecar_path.clone();
        let mutator = std::thread::spawn(move || {
            start_mutation_rx.recv().unwrap();
            mutation_started_tx.send(()).unwrap();
            let authority =
                crate::mailbox::MailboxAuthorityFence::acquire(&mutator_sidecar_path).unwrap();
            authority_acquired_tx.send(()).unwrap();
            std::fs::rename(&mutator_sidecar_path, &mutator_renamed_path).unwrap();
            drop(authority);
        });

        state
            .finalize_invocation_transaction_on(
                invocation_row_id,
                true,
                FinalizeInvocationWrite {
                    exit_code: 0,
                    error_category: None,
                    terminal_reason: None,
                    finished_at: &StateDb::current_rfc3339_timestamp(),
                },
                || {},
                || {
                    start_mutation_tx.send(()).unwrap();
                    mutation_started_rx.recv().unwrap();
                    assert!(
                        authority_acquired_rx
                            .recv_timeout(Duration::from_millis(100))
                            .is_err(),
                        "sidecar namespace authority must remain fenced through state commit"
                    );
                },
            )
            .unwrap();
        authority_acquired_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        mutator.join().unwrap();
        assert_eq!(
            state
                .get_invocation_by_uuid(INVOCATION_UUID)
                .unwrap()
                .unwrap()
                .status,
            InvocationStatus::Succeeded
        );

        std::fs::rename(renamed_sidecar_path, sidecar_path).unwrap();
    }
}
