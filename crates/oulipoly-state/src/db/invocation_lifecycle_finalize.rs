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
use crate::diagnostic_producer::{
    TransactionAttempt, TransactionPhaseGuard, record_sqlite_failure, record_unacquired_release,
};
use crate::diagnostic_recorder::{
    DiagnosticPhase, DiagnosticSpan, PhaseObservation, SpanStart, SqliteDatabaseRole,
    SqliteEventIdentity, SqlitePathClass, SqliteTransactionMode, process_recorder,
};
use crate::result_envelope::{ResultEnvelopeFailureIdentity, ResultEnvelopeInput};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvocationFinalizeError {
    /// State acquisition/commit or nonwaiting sidecar acquisition was contended.
    /// No aggregate end-to-end wait or absence of committed earlier work is implied.
    Contention {
        message: String,
    },
    Failure {
        message: String,
    },
}

impl InvocationFinalizeError {
    fn classify(message: String) -> Self {
        if message.starts_with("process_integrity: completion_authority_contention:") {
            Self::Contention { message }
        } else {
            Self::Failure { message }
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Self::Contention { message } | Self::Failure { message } => message,
        }
    }
}

impl std::fmt::Display for InvocationFinalizeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for InvocationFinalizeError {}

struct FinalizeInvocationWrite<'a> {
    exit_code: i32,
    error_category: Option<&'a str>,
    terminal_reason: Option<&'a str>,
    finished_at: &'a str,
}

impl StateDb {
    pub fn finalize_invocation(
        &self,
        mutation_authority: crate::InvocationMutationAuthority<'_>,
        id: i64,
        success: bool,
        exit_code: i32,
        error_category: Option<&str>,
        terminal_reason: Option<&str>,
    ) -> Result<(), String> {
        self.finalize_invocation_untyped(
            mutation_authority,
            id,
            success,
            exit_code,
            error_category,
            terminal_reason,
        )
    }

    pub fn finalize_invocation_typed(
        &self,
        mutation_authority: crate::InvocationMutationAuthority<'_>,
        id: i64,
        success: bool,
        exit_code: i32,
        error_category: Option<&str>,
        terminal_reason: Option<&str>,
    ) -> Result<(), InvocationFinalizeError> {
        self.finalize_invocation_untyped(
            mutation_authority,
            id,
            success,
            exit_code,
            error_category,
            terminal_reason,
        )
        .map_err(InvocationFinalizeError::classify)
    }

    fn finalize_invocation_untyped(
        &self,
        mutation_authority: crate::InvocationMutationAuthority<'_>,
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
            mutation_authority,
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

    // Mandatory authority accompanies the existing terminal/admission transaction inputs.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn finalize_invocation_transaction(
        &self,
        mutation_authority: crate::InvocationMutationAuthority<'_>,
        id: i64,
        success: bool,
        exit_code: i32,
        error_category: Option<&str>,
        terminal_reason: Option<&str>,
        finished_at: &str,
    ) -> Result<FinalizeInvocationRow, String> {
        self.finalize_invocation_transaction_on(
            mutation_authority,
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
        mutation_authority: crate::InvocationMutationAuthority<'_>,
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
        let start = SpanStart::new("invocation_terminal_finalize", "state_sqlite")
            .with_lifecycle_phase("terminal_finalize")
            .with_sqlite_identity(
                SqliteEventIdentity::new(
                    SqliteDatabaseRole::State,
                    SqlitePathClass::ManagedFile,
                    "invocation.terminal_finalize.state",
                )
                .with_transaction_mode(SqliteTransactionMode::Immediate),
            )
            .with_busy_timeout(super::opening_write::state_writer_busy_timeout())
            .with_identifier("invocation_row_id", id.to_string())
            .with_identifier("success", success.to_string())
            .with_identifier("exit_code", write.exit_code.to_string());
        process_recorder().with_requested_span(start, |span| {
            let attempt = TransactionAttempt::start();
            let tx = match sqlite::Transaction::new_unchecked(
                &self.conn,
                sqlite::TransactionBehavior::Immediate,
            ) {
                Ok(tx) => tx,
                Err(error) => {
                    record_sqlite_failure(span, &error, attempt);
                    record_unacquired_release(span);
                    return Err(Self::format_finalize_begin_transaction_error(id, error));
                }
            };
            let mut phases = TransactionPhaseGuard::acquired(span, attempt);
            let sidecar_failure_recorded = std::cell::Cell::new(false);
            let result = (|| {
                super::completed_turns::refuse_pending_turn(&tx, id)?;
                super::provider_launch_lifecycle::validate_invocation_mutation_authority(
                    &tx,
                    id,
                    mutation_authority,
                )?;
                let invocation = Self::load_invocation_for_finalize(&tx, id)?;
                Self::validate_invocation_is_running(id, &invocation.status)?;
                self.with_finalization_completion_authority_instrumented(
                    tx,
                    &invocation.invocation_uuid,
                    success,
                    span,
                    &sidecar_failure_recorded,
                    |tx| {
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
                        after_validation();

                        phases.commit_started();
                        match tx.commit() {
                            Ok(()) => phases.committed(),
                            Err(error) => {
                                phases.sqlite_failure(&error);
                                return Err(Self::format_finalize_commit_transaction_error(
                                    id, error,
                                ));
                            }
                        }
                        Ok(())
                    },
                )?;
                Ok(invocation)
            })();
            if result.is_err() && !sidecar_failure_recorded.get() {
                phases.failed("invocation_terminal_finalize_failed");
            }
            phases.release_after_owner();
            result
        })
    }

    /// Both ordinary and confirmed-input success use this fence. The callback
    /// owns the already-held State transaction so its entire write AND commit
    /// occur before either sidecar fence is released. It must not replay effects.
    pub(super) fn with_finalization_completion_authority<'tx, T>(
        &self,
        tx: sqlite::Transaction<'tx>,
        invocation_uuid: &str,
        success: bool,
        write_and_commit: impl FnOnce(sqlite::Transaction<'tx>) -> Result<T, String>,
    ) -> Result<T, String> {
        self.with_finalization_completion_authority_observed(
            tx,
            invocation_uuid,
            success,
            None,
            write_and_commit,
        )
    }

    fn with_finalization_completion_authority_instrumented<'tx, T>(
        &self,
        tx: sqlite::Transaction<'tx>,
        invocation_uuid: &str,
        success: bool,
        parent_span: &DiagnosticSpan,
        sidecar_failure_recorded: &std::cell::Cell<bool>,
        write_and_commit: impl FnOnce(sqlite::Transaction<'tx>) -> Result<T, String>,
    ) -> Result<T, String> {
        self.with_finalization_completion_authority_observed(
            tx,
            invocation_uuid,
            success,
            Some((parent_span, sidecar_failure_recorded)),
            write_and_commit,
        )
    }

    fn with_finalization_completion_authority_observed<'tx, T>(
        &self,
        tx: sqlite::Transaction<'tx>,
        invocation_uuid: &str,
        success: bool,
        diagnostic: Option<(&DiagnosticSpan, &std::cell::Cell<bool>)>,
        write_and_commit: impl FnOnce(sqlite::Transaction<'tx>) -> Result<T, String>,
    ) -> Result<T, String> {
        if success {
            require_completion_continuity_registration_ready(&tx)?;
        }
        let obligation = success
            .then(|| Self::first_completion_obligation_for_invocation_on(&tx, invocation_uuid))
            .transpose()
            .map_err(|error| {
                Self::format_completion_authority_storage_error(invocation_uuid, error)
            })?
            .flatten();
        let authority_summary = success
            .then(|| Self::completion_authority_summary_on(&tx, invocation_uuid))
            .transpose()
            .map_err(|error| {
                Self::format_completion_authority_storage_error(invocation_uuid, error)
            })?
            .flatten();
        let materialization_expectation = success
            .then(|| Self::completion_materialization_expectation_on(&tx, invocation_uuid))
            .transpose()
            .map_err(|error| {
                Self::format_completion_authority_storage_error(invocation_uuid, error)
            })?
            .flatten();
        Self::validate_completion_authority_summary(
            invocation_uuid,
            obligation.as_ref(),
            authority_summary.as_ref(),
            materialization_expectation.as_ref(),
        )?;
        let completion_authority_state_path = if obligation.is_none() {
            self.db_path.as_path()
        } else {
            self.completion_authority_state_path().ok_or_else(|| {
                format!(
                    "process_integrity: invocation {} has admitted completion authority but the state database no longer rejoins its retained canonical identity",
                    invocation_uuid
                )
            })?
        };
        let state_continuity_head = obligation
            .as_ref()
            .map(|_| completion_continuity_head_on(&tx))
            .transpose()
            .map_err(|error| {
                Self::format_completion_authority_storage_error(invocation_uuid, error)
            })?
            .flatten();
        let sidecar_path =
            crate::mailbox::MailboxDb::path_for_state_db(completion_authority_state_path);
        if obligation.is_none() {
            return write_and_commit(tx);
        }
        if let Some((parent_span, sidecar_failure_recorded)) = diagnostic {
            let obligation = obligation
                .as_ref()
                .expect("instrumented completion authority requires an obligation");
            let materialization_expectation = materialization_expectation
                .as_ref()
                .expect("validated completion authority requires materialization");
            let state_continuity_head = state_continuity_head
                .as_ref()
                .expect("validated completion authority requires continuity");
            let sidecar_start = SpanStart::new(
                "invocation_terminal_finalize_completion_authority",
                "pid_mailbox_sqlite",
            )
            .with_lifecycle_phase("completion_authority")
            .with_sqlite_identity(
                SqliteEventIdentity::new(
                    SqliteDatabaseRole::PidMailbox,
                    SqlitePathClass::ManagedFile,
                    "invocation.terminal_finalize.sidecar",
                )
                .with_transaction_mode(SqliteTransactionMode::Immediate),
            )
            .with_diagnostic_id(parent_span.diagnostic_id().clone())
            .with_parent_span_id(parent_span.span_id().clone())
            .with_identifier("invocation_uuid", invocation_uuid)
            .with_busy_timeout(std::time::Duration::ZERO);
            return parent_span.with_deferred_requested_span(sidecar_start, |sidecar_span| {
                #[cfg(test)]
                tests::BEFORE_SIDECAR_ACQUISITION.with_borrow_mut(|hook| {
                    if let Some(hook) = hook.take() {
                        hook();
                    }
                });
                let sidecar_authority = match Self::acquire_finalize_sidecar_authority(
                    &sidecar_path,
                    invocation_uuid,
                    obligation,
                ) {
                    Ok(authority) => authority,
                    Err(error) => {
                        sidecar_failure_recorded.set(true);
                        let _ = sidecar_span.record(
                            DiagnosticPhase::Failed,
                            PhaseObservation::not_started()
                                .with_cause("finalization_completion_authority_namespace_failed"),
                        );
                        record_unacquired_release(sidecar_span);
                        return Err(error);
                    }
                };
                let mut sidecar = match self.open_completion_authority_sidecar(
                    Some(&sidecar_authority),
                    invocation_uuid,
                    Some(obligation),
                    Some(sidecar_span),
                ) {
                    Ok(Some(sidecar)) => sidecar,
                    Ok(None) => unreachable!("an admitted obligation requires a sidecar"),
                    Err(error) => {
                        sidecar_failure_recorded.set(true);
                        let _ = sidecar_span.record(
                            DiagnosticPhase::Failed,
                            PhaseObservation::not_started()
                                .with_cause("finalization_completion_authority_open_failed"),
                        );
                        record_unacquired_release(sidecar_span);
                        return Err(error);
                    }
                };
                let (sidecar_fence, sidecar_attempt) =
                    match sidecar.begin_completion_authority_fence_instrumented(sidecar_span) {
                        Ok(fence) => fence,
                        Err(error) => {
                            sidecar_failure_recorded.set(true);
                            return Err(if error.starts_with("completion_authority_contention:") {
                                Self::format_completion_sidecar_sqlite_contention(
                                    invocation_uuid,
                                    obligation,
                                    error,
                                )
                            } else {
                                Self::format_unreadable_completion_sidecar(
                                    invocation_uuid,
                                    obligation,
                                    error,
                                )
                            });
                        }
                    };
                let mut sidecar_phases =
                    TransactionPhaseGuard::acquired(sidecar_span, sidecar_attempt);
                let sidecar_result = if let Err(error) = self.validate_completion_sidecar_authority(
                    &sidecar_fence,
                    invocation_uuid,
                    obligation,
                    materialization_expectation,
                    state_continuity_head,
                ) {
                    sidecar_failure_recorded.set(true);
                    sidecar_phases.failed("finalization_completion_authority_validation_failed");
                    Err(error)
                } else {
                    write_and_commit(tx)
                };
                drop(sidecar_fence);
                sidecar_phases.release_after_rollback();
                sidecar_result
            });
        }
        #[cfg(test)]
        tests::BEFORE_SIDECAR_ACQUISITION.with_borrow_mut(|hook| {
            if let Some(hook) = hook.take() {
                hook();
            }
        });
        let sidecar_authority = obligation
            .as_ref()
            .map(|obligation| {
                Self::acquire_finalize_sidecar_authority(&sidecar_path, invocation_uuid, obligation)
            })
            .transpose()?;
        let mut sidecar = self.open_completion_authority_sidecar(
            sidecar_authority.as_ref(),
            invocation_uuid,
            obligation.as_ref(),
            None,
        )?;
        let sidecar_fence = sidecar
            .as_mut()
            .map(crate::mailbox::MailboxDb::begin_completion_authority_fence)
            .transpose()
            .map_err(|error| {
                let obligation = obligation
                    .as_ref()
                    .expect("sidecar fence requires a completion obligation");
                if error.starts_with("completion_authority_contention:") {
                    Self::format_completion_sidecar_sqlite_contention(
                        invocation_uuid,
                        obligation,
                        error,
                    )
                } else {
                    Self::format_unreadable_completion_sidecar(invocation_uuid, obligation, error)
                }
            })?;
        if let (
            Some(sidecar_fence),
            Some(obligation),
            Some(materialization_expectation),
            Some(state_continuity_head),
        ) = (
            sidecar_fence.as_ref(),
            obligation.as_ref(),
            materialization_expectation.as_ref(),
            state_continuity_head.as_ref(),
        ) {
            self.validate_completion_sidecar_authority(
                sidecar_fence,
                invocation_uuid,
                obligation,
                materialization_expectation,
                state_continuity_head,
            )?;
        }
        write_and_commit(tx)
    }

    fn acquire_finalize_sidecar_authority(
        sidecar_path: &std::path::Path,
        invocation_uuid: &str,
        obligation: &CompletionObligationExpectation,
    ) -> Result<crate::mailbox::MailboxAuthorityFence, String> {
        match crate::mailbox::MailboxAuthorityFence::try_acquire(sidecar_path) {
            Ok(authority) => Ok(authority),
            Err(error @ crate::mailbox::MailboxAuthorityFenceError::Timeout { .. }) => Err(
                Self::format_completion_sidecar_contention(invocation_uuid, obligation, error),
            ),
            Err(error) => Err(Self::format_unreadable_completion_sidecar(
                invocation_uuid,
                obligation,
                error.to_string(),
            )),
        }
    }

    fn open_completion_authority_sidecar(
        &self,
        authority: Option<&crate::mailbox::MailboxAuthorityFence>,
        invocation_uuid: &str,
        obligation: Option<&CompletionObligationExpectation>,
        parent: Option<&DiagnosticSpan>,
    ) -> Result<Option<crate::mailbox::MailboxDb>, String> {
        let Some(obligation) = obligation else {
            return Ok(None);
        };
        let authority = authority.ok_or_else(|| {
            format!(
                "process_integrity: invocation {invocation_uuid} has completion obligations but no sidecar authority"
            )
        })?;
        let sidecar_path = authority.path();
        if !sidecar_path.exists() {
            return Err(Self::format_missing_completion_sidecar(
                invocation_uuid,
                obligation,
            ));
        }
        let opened = if let Some(parent) = parent {
            crate::mailbox::MailboxDb::open_existing_for_completion_authority_instrumented(
                authority, parent,
            )
        } else {
            crate::mailbox::MailboxDb::open_existing_for_completion_authority(authority)
        };
        opened.map(Some).map_err(|error| {
            Self::format_unreadable_completion_sidecar(invocation_uuid, obligation, error)
        })
    }

    fn validate_completion_sidecar_authority(
        &self,
        sidecar: &crate::mailbox::CompletionAuthorityFence<'_>,
        invocation_uuid: &str,
        obligation: &CompletionObligationExpectation,
        expected: &CompletionMaterializationExpectation,
        state_continuity_head: &crate::mailbox::CompletionContinuityHead,
    ) -> Result<(), String> {
        let observed_generation = sidecar.sidecar_generation().map_err(|error| {
            Self::format_unreadable_completion_sidecar(invocation_uuid, obligation, error)
        })?;
        if obligation.expected_sidecar_generation != observed_generation {
            return Err(format!(
                "process_integrity: invocation {invocation_uuid} cannot succeed because completion obligation {} owned by {} expects mailbox sidecar generation {}, observed {}",
                obligation.admission_id,
                obligation.owner_invocation_uuid,
                obligation.expected_sidecar_generation,
                observed_generation,
            ));
        }
        let sidecar_continuity_head = sidecar.completion_continuity_head().map_err(|error| {
            Self::format_unreadable_completion_sidecar(invocation_uuid, obligation, error)
        })?;
        if sidecar_continuity_head.as_ref() != Some(state_continuity_head) {
            return Err(format!(
                "process_integrity: invocation {invocation_uuid} cannot succeed because completion obligation {} owned by {} has no exact matching State/sidecar continuity proof",
                obligation.admission_id, obligation.owner_invocation_uuid,
            ));
        }
        let observed = sidecar
            .completion_materialization_summary(invocation_uuid)
            .map_err(|error| {
                Self::format_unreadable_completion_sidecar(invocation_uuid, obligation, error)
            })?;
        let exact_match = observed.as_ref().is_some_and(|observed| {
            observed.materialized_count == expected.materialized_count
                && observed.authority_ordinal == expected.authority_ordinal
                && observed.sidecar_generation == expected.sidecar_generation
                && observed.continuity_digest == expected.continuity_digest
        });
        if !exact_match {
            let observed_shape = observed.as_ref().map_or_else(
                || "the sidecar materialization summary is absent".to_string(),
                |observed| {
                    format!(
                        "only {} remain in the sidecar materialization summary",
                        observed.materialized_count
                    )
                },
            );
            return Err(format!(
                "process_integrity: invocation {invocation_uuid} cannot succeed because its completion authority requires {} exact agent_bash_complete event/listener obligations at ordinal {} and digest {}, but {observed_shape}; first obligation {} is owned by invocation {} and session {}",
                expected.materialized_count,
                expected.authority_ordinal,
                expected.continuity_digest,
                obligation.admission_id,
                obligation.owner_invocation_uuid,
                obligation.owner_session_id,
            ));
        }
        Ok(())
    }

    fn validate_completion_authority_summary(
        invocation_uuid: &str,
        obligation: Option<&CompletionObligationExpectation>,
        summary: Option<&CompletionAuthoritySummary>,
        materialization: Option<&CompletionMaterializationExpectation>,
    ) -> Result<(), String> {
        match (obligation, summary, materialization) {
            (None, None, None) => Ok(()),
            (Some(_), Some(summary), Some(materialization))
                if summary.obligation_count > 0
                    && summary.obligation_count == summary.continuity_count
                    && summary.obligation_count == materialization.materialized_count =>
            {
                Ok(())
            }
            _ => Err(format!(
                "process_integrity: invocation {invocation_uuid} cannot succeed because its completion obligations lack exact State continuity and materialization proof: obligations={}, continuity={}, materialized={}",
                summary.map_or(0, |summary| summary.obligation_count),
                summary.map_or(0, |summary| summary.continuity_count),
                materialization.map_or(0, |summary| summary.materialized_count),
            )),
        }
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
        expectation: &CompletionObligationExpectation,
    ) -> String {
        format!(
            "process_integrity: invocation {invocation_uuid} cannot succeed because completion obligation {} owned by {} expects mailbox sidecar generation {}, but the sidecar is missing",
            expectation.admission_id,
            expectation.owner_invocation_uuid,
            expectation.expected_sidecar_generation,
        )
    }

    fn format_unreadable_completion_sidecar(
        invocation_uuid: &str,
        expectation: &CompletionObligationExpectation,
        error: String,
    ) -> String {
        format!(
            "process_integrity: invocation {invocation_uuid} cannot succeed because completion obligation {} owned by {} expects mailbox sidecar generation {}, but sidecar authority is unavailable: {error}",
            expectation.admission_id,
            expectation.owner_invocation_uuid,
            expectation.expected_sidecar_generation,
        )
    }

    fn format_completion_sidecar_contention(
        invocation_uuid: &str,
        expectation: &CompletionObligationExpectation,
        error: crate::mailbox::MailboxAuthorityFenceError,
    ) -> String {
        format!(
            "process_integrity: completion_authority_contention: invocation {invocation_uuid} could not acquire mailbox sidecar authority for completion obligation {} owned by {}: {error}",
            expectation.admission_id, expectation.owner_invocation_uuid,
        )
    }

    fn format_completion_sidecar_sqlite_contention(
        invocation_uuid: &str,
        expectation: &CompletionObligationExpectation,
        error: String,
    ) -> String {
        format!(
            "process_integrity: completion_authority_contention: invocation {invocation_uuid} could not acquire the PID mailbox SQLite writer for completion obligation {} owned by {}: {error}",
            expectation.admission_id, expectation.owner_invocation_uuid,
        )
    }

    pub(super) fn format_finalize_begin_transaction_error(id: i64, err: sqlite::Error) -> String {
        if sqlite_error_is_contention(&err) {
            return format!(
                "process_integrity: completion_authority_contention: invocation row {id} could not acquire the State writer: {err}"
            );
        }
        format!("Failed to begin invocation finalize tx: {err}")
    }

    pub(super) fn format_finalize_commit_transaction_error(id: i64, err: sqlite::Error) -> String {
        if sqlite_error_is_contention(&err) {
            return format!(
                "process_integrity: completion_authority_contention: invocation row {id} could not commit while holding the State writer: {err}"
            );
        }
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

fn sqlite_error_is_contention(error: &sqlite::Error) -> bool {
    matches!(
        error.sqlite_error_code(),
        Some(sqlite::ffi::ErrorCode::DatabaseBusy) | Some(sqlite::ffi::ErrorCode::DatabaseLocked)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InvocationStart;
    use crate::diagnostic_recorder::{
        FlightRecorder, FlightRecorderReader, RecorderConfig, SqliteMeasurementGap,
        with_test_process_recorder,
    };
    use crate::mailbox::{CompletionEventRegistrationInput, MailboxDb};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    thread_local! {
        pub(super) static BEFORE_SIDECAR_ACQUISITION: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    }

    fn concurrent_state_probe(path: &std::path::Path) -> std::thread::JoinHandle<()> {
        let path = path.to_owned();
        let (start, receive_start) = mpsc::channel();
        let (reserved, receive_reserved) = mpsc::channel();
        let probe = std::thread::spawn(move || {
            let unrelated = sqlite::Connection::open(path).unwrap();
            unrelated.busy_timeout(Duration::ZERO).unwrap();
            receive_start.recv().unwrap();
            let error = unrelated.execute_batch("BEGIN IMMEDIATE").unwrap_err();
            assert!(sqlite_error_is_contention(&error));
            // Measure actual independent acquisition from the known-held cut.
            // 250ms is below the old namespace test budget (500ms) and SQLite
            // patience (seconds), not a new product timeout or suite cutoff.
            unrelated.busy_timeout(Duration::from_millis(250)).unwrap();
            let started = std::time::Instant::now();
            reserved.send(()).unwrap();
            unrelated
                .execute_batch("BEGIN IMMEDIATE; ROLLBACK;")
                .unwrap();
            assert!(started.elapsed() < Duration::from_millis(250));
            eprintln!("concurrent State writer acquired before the old sidecar wait budget");
        });
        BEFORE_SIDECAR_ACQUISITION.with_borrow_mut(|hook| {
            *hook = Some(Box::new(move || {
                start.send(()).unwrap();
                receive_reserved.recv().unwrap();
            }));
        });
        probe
    }

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
                crate::InvocationMutationAuthority::Standalone,
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
        let authority =
            crate::mailbox::MailboxAuthorityFence::acquire_exclusive(&sidecar_path).unwrap();
        // Root Act2 supersedes the historical waiting oracle, not finalization
        // authority: contend without retaining State during the sidecar wait.
        let probe = concurrent_state_probe(&state.db_path);
        let error = state
            .finalize_invocation(
                crate::InvocationMutationAuthority::Standalone,
                invocation_row_id,
                true,
                0,
                None,
                None,
            )
            .unwrap_err();
        probe.join().unwrap();
        assert!(error.contains("completion_authority_contention"), "{error}");
        assert_eq!(
            state
                .get_invocation_by_uuid(INVOCATION_UUID)
                .unwrap()
                .unwrap()
                .status,
            InvocationStatus::Running
        );
        let unrelated = sqlite::Connection::open(&state.db_path).unwrap();
        unrelated
            .execute_batch("BEGIN IMMEDIATE; ROLLBACK;")
            .unwrap();
        drop(authority);
        state
            .finalize_invocation(
                crate::InvocationMutationAuthority::Standalone,
                invocation_row_id,
                true,
                0,
                None,
                None,
            )
            .unwrap();
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
    fn storage_finalize_sidecar_sqlite_contention_unwinds_state_without_waiting() {
        let (directory, state, invocation_row_id, sidecar_path) =
            state_with_completion_obligation();
        let holder = sqlite::Connection::open(&sidecar_path).unwrap();
        holder.execute_batch("BEGIN IMMEDIATE").unwrap();
        let probe = concurrent_state_probe(&state.db_path);
        let recorder_root = directory.path().join("recorder");
        let recorder = FlightRecorder::open(&recorder_root, RecorderConfig::default()).unwrap();
        let error = with_test_process_recorder(recorder.clone(), || {
            state
                .finalize_invocation(
                    crate::InvocationMutationAuthority::Standalone,
                    invocation_row_id,
                    true,
                    0,
                    None,
                    None,
                )
                .unwrap_err()
        });
        recorder.drain_deferred_for_test().unwrap();
        probe.join().unwrap();
        assert!(error.contains("without waiting"), "{error}");
        assert_eq!(
            state
                .get_invocation_by_uuid(INVOCATION_UUID)
                .unwrap()
                .unwrap()
                .status,
            InvocationStatus::Running
        );
        let unrelated = sqlite::Connection::open(&state.db_path).unwrap();
        unrelated.busy_timeout(Duration::ZERO).unwrap();
        unrelated
            .execute_batch("BEGIN IMMEDIATE; ROLLBACK;")
            .unwrap();
        holder.execute_batch("ROLLBACK").unwrap();

        let report = FlightRecorderReader::new(&recorder_root).inspect();
        let events = report
            .events
            .iter()
            .filter(|record| {
                matches!(
                    record.event.operation.as_str(),
                    "invocation_terminal_finalize"
                        | "invocation_terminal_finalize_completion_authority"
                )
            })
            .map(|record| &record.event)
            .collect::<Vec<_>>();
        assert_eq!(
            events
                .iter()
                .map(|event| (event.operation.as_str(), event.phase))
                .collect::<Vec<_>>(),
            vec![
                ("invocation_terminal_finalize", DiagnosticPhase::Requested),
                ("invocation_terminal_finalize", DiagnosticPhase::Acquired),
                (
                    "invocation_terminal_finalize_completion_authority",
                    DiagnosticPhase::Requested,
                ),
                (
                    "invocation_terminal_finalize_completion_authority",
                    DiagnosticPhase::Contention,
                ),
                (
                    "invocation_terminal_finalize_completion_authority",
                    DiagnosticPhase::Released,
                ),
                ("invocation_terminal_finalize", DiagnosticPhase::Released),
            ]
        );
        let state_requested = events[0];
        for event in &events[2..5] {
            assert_eq!(event.diagnostic_id, state_requested.diagnostic_id);
            assert_eq!(
                event.parent_span_id.as_ref(),
                Some(&state_requested.span_id)
            );
            assert_eq!(event.resource, "pid_mailbox_sqlite");
            assert_eq!(event.observation.busy_timeout_millis, Some(0));
        }
        let contention = events[3].observation.sqlite_failure.as_ref().unwrap();
        assert!(contention.contention);
        assert_eq!(events[3].observation.wait_micros, None);
        let evidence = events[3].observation.sqlite.as_ref().unwrap();
        assert!(evidence.writer_authority_acquisition_micros.is_some());
        assert_eq!(evidence.writer_authority_wait_micros, None);
        assert!(
            evidence
                .measurement_gaps
                .contains(&SqliteMeasurementGap::WriterWaitNotExposedByApi)
        );
    }

    #[test]
    fn successful_finalize_releases_sidecar_before_terminal_observation() {
        let (directory, state, invocation_row_id, sidecar_path) =
            state_with_completion_obligation();
        let recorder_root = directory.path().join("recorder");
        let recorder = FlightRecorder::open(&recorder_root, RecorderConfig::default()).unwrap();
        let release_checks = Arc::new(AtomicUsize::new(0));
        let hook_checks = Arc::clone(&release_checks);
        let hook_path = sidecar_path.clone();

        with_test_process_recorder(recorder.clone(), || {
            crate::diagnostic_producer::with_before_release_observation_for_test(
                move || {
                    let contender = sqlite::Connection::open(&hook_path).unwrap();
                    contender.busy_timeout(Duration::ZERO).unwrap();
                    contender
                        .execute_batch("BEGIN IMMEDIATE; ROLLBACK;")
                        .expect("sidecar owner must be released before Released evidence");
                    hook_checks.fetch_add(1, Ordering::Relaxed);
                },
                || {
                    state
                        .finalize_invocation(
                            crate::InvocationMutationAuthority::Standalone,
                            invocation_row_id,
                            true,
                            0,
                            None,
                            None,
                        )
                        .unwrap();
                },
            );
        });
        recorder.drain_deferred_for_test().unwrap();
        assert_eq!(release_checks.load(Ordering::Relaxed), 2);

        let report = FlightRecorderReader::new(&recorder_root).inspect();
        let released = report
            .events
            .iter()
            .find(|record| {
                record.event.operation == "invocation_terminal_finalize_completion_authority"
                    && record.event.phase == DiagnosticPhase::Released
            })
            .expect("sidecar release evidence");
        let evidence = released.event.observation.sqlite.as_ref().unwrap();
        assert!(evidence.execution_micros.is_some());
        assert_eq!(evidence.commit_micros, None);
        assert!(
            evidence
                .measurement_gaps
                .contains(&SqliteMeasurementGap::CommitNotApplicable)
        );
        assert!(
            evidence
                .measurement_gaps
                .contains(&SqliteMeasurementGap::PostCommitNotApplicable)
        );
    }

    #[test]
    fn exhausted_sidecar_authority_contention_is_distinct_from_identity_failure() {
        let (_directory, state, invocation_row_id, sidecar_path) =
            state_with_completion_obligation();
        let _authority =
            crate::mailbox::MailboxAuthorityFence::acquire_exclusive(&sidecar_path).unwrap();

        let error = state
            .finalize_invocation(
                crate::InvocationMutationAuthority::Standalone,
                invocation_row_id,
                true,
                0,
                None,
                None,
            )
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
                crate::InvocationMutationAuthority::Standalone,
                "age299-s2-finalize-fence-admission",
                CompletionEventRegistrationInput {
                    event_id: EVENT_ID,
                    delivery_mode: "async",
                    owner_session_id: Some(SESSION_ID),
                    owner_invocation_uuid: Some(INVOCATION_UUID),
                    state_dir: "/tmp/age299-s2-finalize-fence-state",
                    meta_path: "/tmp/age299-s2-finalize-fence-meta",
                    log_path: "/tmp/age299-s2-finalize-fence-log",
                    rc_path: "/tmp/age299-s2-finalize-fence-rc",
                },
            )
            .unwrap();

        let (start_mutation_tx, start_mutation_rx) = mpsc::channel();
        let (mutation_started_tx, mutation_started_rx) = mpsc::channel();
        let (authority_acquired_tx, authority_acquired_rx) = mpsc::channel();
        let mutator_sidecar_path = sidecar_path.clone();
        let mutator_renamed_path = renamed_sidecar_path.clone();
        let mutator = std::thread::spawn(move || {
            start_mutation_rx.recv().unwrap();
            mutation_started_tx.send(()).unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(4);
            let authority = loop {
                match crate::mailbox::MailboxAuthorityFence::acquire_exclusive(
                    &mutator_sidecar_path,
                ) {
                    Ok(authority) => break authority,
                    Err(crate::mailbox::MailboxAuthorityFenceError::Timeout { .. })
                        if std::time::Instant::now() < deadline => {}
                    Err(error) => {
                        panic!("failed to acquire post-commit sidecar authority: {error}")
                    }
                }
            };
            authority_acquired_tx.send(()).unwrap();
            std::fs::rename(&mutator_sidecar_path, &mutator_renamed_path).unwrap();
            drop(authority);
        });

        state
            .finalize_invocation_transaction_on(
                crate::InvocationMutationAuthority::Standalone,
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

    #[test]
    fn mature_owner_finalization_has_bounded_sqlite_vm_work_and_one_head_lookup_per_store() {
        fn finalization_work(obligation_count: usize) -> (usize, usize, usize) {
            let directory = tempfile::tempdir().unwrap();
            let state_path = directory.path().join("state.db");
            let mut state = StateDb::open(&state_path).unwrap();
            let invocation_uuid = uuid::Uuid::new_v4().to_string();
            let invocation_row_id = state
                .start_invocation(&InvocationStart {
                    invocation_uuid: invocation_uuid.clone(),
                    model_name: "age299-s2-mature-owner".to_string(),
                    provider_name: "test-provider".to_string(),
                    provider_index: 0,
                    parent_invocation_id: None,
                })
                .unwrap();
            for ordinal in 0..obligation_count {
                let event_id = format!("age299-s2-mature-owner-event-{ordinal}");
                state
                    .register_completion_event_with_obligation(
                        crate::InvocationMutationAuthority::Standalone,
                        &format!("age299-s2-mature-owner-admission-{ordinal}"),
                        CompletionEventRegistrationInput {
                            event_id: &event_id,
                            delivery_mode: "async",
                            owner_session_id: Some("age299-s2-mature-owner-session"),
                            owner_invocation_uuid: Some(&invocation_uuid),
                            state_dir: "/tmp/age299-s2-mature-owner-state",
                            meta_path: "/tmp/age299-s2-mature-owner-meta",
                            log_path: "/tmp/age299-s2-mature-owner-log",
                            rc_path: "/tmp/age299-s2-mature-owner-rc",
                        },
                    )
                    .unwrap();
            }
            crate::db::ownership_authority::reset_completion_continuity_head_query_count();
            crate::mailbox::reset_completion_continuity_head_query_count();
            crate::mailbox::install_completion_finalization_vm_counter(&state.conn);
            crate::mailbox::begin_completion_finalization_vm_count();

            state
                .finalize_invocation(
                    crate::InvocationMutationAuthority::Standalone,
                    invocation_row_id,
                    true,
                    0,
                    None,
                    None,
                )
                .unwrap();
            let vm_steps = crate::mailbox::end_completion_finalization_vm_count();

            (
                crate::db::ownership_authority::completion_continuity_head_query_count(),
                crate::mailbox::completion_continuity_head_query_count(),
                vm_steps,
            )
        }

        let small = finalization_work(1);
        let mature = finalization_work(256);
        assert_eq!((small.0, small.1), (1, 1));
        assert_eq!((mature.0, mature.1), (1, 1));
        assert!(
            small.2 < 2_000,
            "small-owner finalization used {} VM steps",
            small.2
        );
        assert!(
            mature.2 <= small.2 + 128,
            "mature-owner finalization grew with retained obligations: small={}, mature={}",
            small.2,
            mature.2
        );
    }
    fn combined_input<'a>(id: i64) -> crate::ProviderTurnEffectInput<'a> {
        crate::ProviderTurnEffectInput {
            invocation_row_id: id,
            delivery_ids: &[],
            accept_delivery_if_missing: true,
            session_id: SESSION_ID,
            turn_generation_id: INVOCATION_UUID,
            submitted_evidence: Some("fixture exact submitted input"),
            confirmed_evidence: Some("fixture independent confirmed input"),
            observed_at: 17,
            returned_artifacts: &[],
            resume_acceptance_status: Some("accepted"),
            resume_acceptance_evidence: Some("fixture independent confirmed input"),
            success: true,
            exit_code: 0,
            error_category: None,
            terminal_reason: None,
        }
    }

    fn assert_combined_uncommitted(state: &StateDb, id: i64) {
        let row = state.get_invocation_by_id(id).unwrap().unwrap();
        assert_eq!(row.status, InvocationStatus::Running);
        assert_eq!(row.success, None);
        for table in [
            "session_delivery_acknowledgements",
            "invocation_returned_artifacts",
        ] {
            let count: i64 = state
                .conn
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0, "partial combined write in {table}");
        }
        let completed: i64 = state
            .conn
            .query_row(
                "SELECT coalesce(sum(invocation_count),0) FROM providers",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(completed, 0);
    }

    #[test]
    fn combined_success_sidecar_contention_unwinds_state_without_waiting() {
        let (_dir, state, id, sidecar) = state_with_completion_obligation();
        let authority = crate::mailbox::MailboxAuthorityFence::acquire_exclusive(&sidecar).unwrap();
        let probe = concurrent_state_probe(&state.db_path);
        let error = state
            .apply_provider_turn_effects(
                crate::InvocationMutationAuthority::Standalone,
                combined_input(id),
            )
            .err()
            .unwrap();
        probe.join().unwrap();
        assert!(error.contains("completion_authority_contention"), "{error}");
        assert_combined_uncommitted(&state, id);
        drop(authority);
        state
            .apply_provider_turn_effects(
                crate::InvocationMutationAuthority::Standalone,
                combined_input(id),
            )
            .unwrap();
    }

    #[test]
    fn combined_success_sqlite_contention_unwinds_state_without_waiting() {
        let (_dir, state, id, sidecar) = state_with_completion_obligation();
        let holder = sqlite::Connection::open(sidecar).unwrap();
        holder.execute_batch("BEGIN IMMEDIATE").unwrap();
        let probe = concurrent_state_probe(&state.db_path);
        let error = state
            .apply_provider_turn_effects(
                crate::InvocationMutationAuthority::Standalone,
                combined_input(id),
            )
            .err()
            .unwrap();
        probe.join().unwrap();
        assert!(error.contains("without waiting"), "{error}");
        assert_combined_uncommitted(&state, id);
        holder.execute_batch("ROLLBACK").unwrap();
        state
            .apply_provider_turn_effects(
                crate::InvocationMutationAuthority::Standalone,
                combined_input(id),
            )
            .unwrap();
    }

    #[test]
    fn combined_success_rejects_replaced_sidecar_generation() {
        let (_dir, state, id, sidecar) = state_with_completion_obligation();
        let held = sidecar.with_extension("held");
        std::fs::rename(&sidecar, &held).unwrap();
        drop(MailboxDb::open(&sidecar).unwrap());
        let error = state
            .apply_provider_turn_effects(
                crate::InvocationMutationAuthority::Standalone,
                combined_input(id),
            )
            .err()
            .unwrap();
        assert!(
            error.contains("expects mailbox sidecar generation"),
            "{error}"
        );
        assert_combined_uncommitted(&state, id);
    }

    #[test]
    fn combined_success_rejects_lost_retained_state_identity() {
        let (_dir, state, id, _sidecar) = state_with_completion_obligation();
        std::fs::rename(&state.db_path, state.db_path.with_extension("held")).unwrap();
        let error = state
            .apply_provider_turn_effects(
                crate::InvocationMutationAuthority::Standalone,
                combined_input(id),
            )
            .err()
            .unwrap();
        assert!(error.contains("retained canonical identity"), "{error}");
        assert_combined_uncommitted(&state, id);
    }

    #[test]
    fn combined_failure_does_not_require_outgoing_materialization() {
        let (_dir, state, id, sidecar) = state_with_completion_obligation();
        let _authority =
            crate::mailbox::MailboxAuthorityFence::acquire_exclusive(&sidecar).unwrap();
        let mut input = combined_input(id);
        input.success = false;
        input.exit_code = 23;
        input.terminal_reason = Some("genuine-provider-failure");
        state
            .apply_provider_turn_effects(crate::InvocationMutationAuthority::Standalone, input)
            .unwrap();
        let row = state.get_invocation_by_id(id).unwrap().unwrap();
        assert_eq!(row.status, InvocationStatus::Failed);
        assert_eq!(row.exit_code, Some(23));
        assert_eq!(
            row.terminal_reason.as_deref(),
            Some("genuine-provider-failure")
        );
    }

    #[test]
    fn combined_aggregate_fault_rolls_back_ack_artifacts_and_terminal_row() {
        let (_dir, state, id, _sidecar) = state_with_completion_obligation();
        // Valid caller-supplied artifact reference: these tests establish DB
        // atomicity, not the upstream artifact producer or provider semantics.
        let refs = [oulipoly_agent_messenger::ReturnedArtifactRef {
            version_id: format!("store://return/{INVOCATION_UUID}/result/1"),
            name: "result".into(),
            store_address: oulipoly_agent_messenger::StoreAddress {
                workflow_run_id: format!("return:{INVOCATION_UUID}"),
                artifact_name: "result".into(),
                version: 1,
            },
            sha256: "a".repeat(64),
            content_len: 1,
            format_hint: None,
            verdict_line: None,
            source: oulipoly_agent_messenger::ReturnedArtifactSource::InlineBytes,
            producer_invocation_uuid: INVOCATION_UUID.parse().unwrap(),
            returned_at: chrono::Utc::now(),
        }];
        let ids = ["distinct-incoming-delivery".to_string()];
        state.conn.execute_batch("CREATE TRIGGER private_aggregate_fault BEFORE INSERT ON providers BEGIN SELECT RAISE(ABORT,'private aggregate fault'); END").unwrap();
        let mut input = combined_input(id);
        input.delivery_ids = &ids;
        input.returned_artifacts = &refs;
        let error = state
            .apply_provider_turn_effects(crate::InvocationMutationAuthority::Standalone, input)
            .err()
            .unwrap();
        assert!(error.contains("private aggregate fault"), "{error}");
        assert_combined_uncommitted(&state, id);
        state
            .conn
            .execute_batch("DROP TRIGGER private_aggregate_fault")
            .unwrap();
        let mut input = combined_input(id);
        input.delivery_ids = &ids;
        input.returned_artifacts = &refs;
        state
            .apply_provider_turn_effects(crate::InvocationMutationAuthority::Standalone, input)
            .unwrap();
        assert_eq!(
            state.get_invocation_by_id(id).unwrap().unwrap().status,
            InvocationStatus::Succeeded
        );
        assert_eq!(
            state
                .conn
                .query_row(
                    "SELECT count(*) FROM invocation_returned_artifacts",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        let ack: (String,String,String) = state.conn.query_row("SELECT delivery_id,turn_generation_id,confirmed_evidence FROM session_delivery_acknowledgements", [], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        assert_eq!(
            ack,
            (
                ids[0].clone(),
                INVOCATION_UUID.into(),
                "fixture independent confirmed input".into()
            )
        );
    }

    #[test]
    fn combined_success_without_outgoing_obligations_needs_no_sidecar() {
        let directory = tempfile::tempdir().unwrap();
        let state = StateDb::open(&directory.path().join("state.db")).unwrap();
        let id = state
            .start_invocation(&InvocationStart {
                invocation_uuid: INVOCATION_UUID.into(),
                model_name: "no-obligation".into(),
                provider_name: "fixture".into(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        let ids = ["distinct-incoming-delivery".into()];
        let mut input = combined_input(id);
        input.delivery_ids = &ids;
        state
            .apply_provider_turn_effects(crate::InvocationMutationAuthority::Standalone, input)
            .unwrap();
        assert_eq!(
            state.get_invocation_by_id(id).unwrap().unwrap().status,
            InvocationStatus::Succeeded
        );
        assert!(!MailboxDb::path_for_state_db(&state.db_path).exists());
    }

    #[test]
    fn combined_success_requires_exact_materialization_not_only_continuity() {
        let (_dir, state, id, sidecar) = state_with_completion_obligation();
        let projection = sqlite::Connection::open(sidecar).unwrap();
        // Fault only the materialized summary; generation and continuity remain.
        projection
            .execute(
                "DELETE FROM completion_authority_materialization_summary WHERE invocation_uuid=?1",
                [INVOCATION_UUID],
            )
            .unwrap();
        let error = state
            .apply_provider_turn_effects(
                crate::InvocationMutationAuthority::Standalone,
                combined_input(id),
            )
            .err()
            .unwrap();
        assert!(error.contains("exact agent_bash_complete"), "{error}");
        assert_combined_uncommitted(&state, id);
    }

    #[test]
    fn combined_refusal_preserves_exact_existing_incoming_evidence() {
        use crate::SessionLifecycleRepository;
        let (_dir, mut state, id, sidecar) = state_with_completion_obligation();
        let delivery = "older-independent-input";
        state
            .accept_pending(delivery, SESSION_ID, INVOCATION_UUID, 1)
            .unwrap();
        state
            .mark_submitted(
                delivery,
                SESSION_ID,
                INVOCATION_UUID,
                "fixture exact submitted input",
                2,
            )
            .unwrap();
        state
            .mark_confirmed(
                delivery,
                SESSION_ID,
                INVOCATION_UUID,
                "fixture independent confirmed input",
                3,
            )
            .unwrap();
        let original = state.acknowledgement(delivery).unwrap().unwrap();
        let ids = [delivery.to_string()];
        for (session, turn, confirmation) in [
            (
                "wrong-session",
                INVOCATION_UUID,
                "fixture independent confirmed input",
            ),
            (
                SESSION_ID,
                "stale-turn",
                "fixture independent confirmed input",
            ),
            (SESSION_ID, INVOCATION_UUID, "replacement-confirmation"),
        ] {
            let mut input = combined_input(id);
            input.delivery_ids = &ids;
            input.session_id = session;
            input.turn_generation_id = turn;
            input.confirmed_evidence = Some(confirmation);
            assert!(
                state
                    .apply_provider_turn_effects(
                        crate::InvocationMutationAuthority::Standalone,
                        input
                    )
                    .is_err()
            );
            assert_eq!(state.acknowledgement(delivery).unwrap().unwrap(), original);
            assert_eq!(
                state.get_invocation_by_id(id).unwrap().unwrap().status,
                InvocationStatus::Running
            );
        }
        let authority = crate::mailbox::MailboxAuthorityFence::acquire_exclusive(&sidecar).unwrap();
        let mut input = combined_input(id);
        input.delivery_ids = &ids;
        assert!(
            state
                .apply_provider_turn_effects(crate::InvocationMutationAuthority::Standalone, input)
                .is_err()
        );
        assert_eq!(state.acknowledgement(delivery).unwrap().unwrap(), original);
        drop(authority);
        let mut input = combined_input(id);
        input.delivery_ids = &ids;
        state
            .apply_provider_turn_effects(crate::InvocationMutationAuthority::Standalone, input)
            .unwrap();
        assert_eq!(state.acknowledgement(delivery).unwrap().unwrap(), original);
    }
}
