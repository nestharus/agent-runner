//! AGE-244 S7b external-provider export/replace adapter.
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-runtime/src/session_external_provider
//!     role: intrinsic-surface
//!     Domain: session export/replace provider dispatch adapter
//!     Owns:
//!       - external session provider identity mapping
//!       - session export/replace provider request construction
//!       - session provider capability and client invocation
//!       - canonical export result validation
//!       - replace input preflight, provider fact validation, and host-apply orchestration
//! ```

mod capability_gate;
mod client_error_formatter;
mod client_invoker;
mod export_result_mapper;
mod export_result_parser;
mod export_result_validator;
mod hash_formatter;
mod identity;
mod identity_formatter;
mod provider_error;
mod provider_error_accessor;
mod provider_error_formatter;
mod provider_registry_accessor;
mod registry_error_formatter;
mod registry_handle_validator;
mod registry_transport_mapper;
mod replace_host_apply;
mod replace_input_accessor;
mod replace_input_formatter;
mod replace_input_mapper;
mod replace_no_change_formatter;
mod replace_result_mapper;
mod request_builder;
mod request_id_formatter;
mod service_error_mapper;

use crate::provider_registry::ProviderRegistryHandle;
use crate::services::SessionServiceExternalProviderIdentity;
use crate::session_export::ExportError;
use crate::session_lock::{Lease, LockError, ProcessAuthority, SessionLock};
use crate::session_replace::{
    ProviderReplaceDbPreimage, ProviderReplaceDbTarget, ReplaceError, ReplaceReceipt, ReplaceSource,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::Duration;
use uuid::Uuid;

const PROVIDER_OWNED_JOURNAL_OPERATION: &str = "provider-owned-import-replace";
const PROVIDER_OWNED_TEST_HOOK_ENV: &str = "OULIPOLY_PROVIDER_OWNED_REPLACE_TEST_HOOK";
const PROVIDER_OWNED_LEASE_TTL_ENV: &str = "OULIPOLY_PROVIDER_OWNED_REPLACE_LEASE_TTL_MS";
const TEST_STOP_AFTER_RECOVERY_ID: &str = "stop-after-recovery-id-journal-update";
const TEST_STOP_AFTER_DB_APPLY_MARKER: &str = "stop-after-db-apply-marker";
const TEST_FAIL_JOURNAL_REMOVE: &str = "fail-provider-owned-journal-remove";
const TEST_SLEEP_RECOVERY_AFTER_LEASE_PREFIX: &str = "sleep-recovery-after-lease-ms:";
const TEST_SLEEP_BEFORE_DB_POSTIMAGE_PREFIX: &str = "sleep-before-db-postimage-ms:";
const TEST_SLEEP_BEFORE_RETIRE_PREFIX: &str = "sleep-before-retire-ms:";
const PROVIDER_OWNED_RECONCILIATION_PENDING: &str = "pending";
const PROVIDER_OWNED_RECONCILIATION_CLEANUP_ONLY: &str = "cleanup_only";

pub fn export_session(
    provider_registry: Option<&ProviderRegistryHandle>,
    identity: SessionServiceExternalProviderIdentity,
    session_id: &str,
) -> Result<Vec<u8>, ExportError> {
    let registry = registry_handle_validator::export_registry_handle(provider_registry)
        .map_err(service_error_mapper::export_adapter_error)?
        .current();
    let identity = identity::map_identity(identity);
    let describe = provider_registry_accessor::describe_provider(registry.as_ref(), &identity)
        .map_err(service_error_mapper::export_registry_error)?;
    capability_gate::require_session_capability(&describe)
        .map_err(service_error_mapper::export_adapter_error)?;
    let provider_instance_id = identity_formatter::provider_instance_id(&describe.provider_id);
    let settings_id = identity_formatter::settings_id(&describe, &identity.settings_id);
    let identity = identity::map_described_identity(identity, provider_instance_id, settings_id);
    let request_id = request_id_formatter::session_request_id("export");
    let request = request_builder::build_export_request(
        &identity,
        session_id,
        registry.host_options(),
        request_id,
    )
    .map_err(service_error_mapper::export_adapter_error)?;
    let client =
        provider_registry_accessor::provider_client_for_model(registry.as_ref(), &identity)
            .map_err(registry_transport_mapper::registry_error_as_transport)
            .map_err(service_error_mapper::export_client_error)?;
    let result = client_invoker::invoke_export(&client, request)
        .map_err(service_error_mapper::export_client_error)?;
    let bytes = export_result_parser::decode_base64(&result.data_base64)
        .map_err(service_error_mapper::export_adapter_error)?;
    export_result_validator::validate_export_result(&result, &bytes)
        .map_err(service_error_mapper::export_adapter_error)?;
    Ok(export_result_mapper::map_accepted_export_result(bytes))
}

pub fn replace_session(
    provider_registry: Option<&ProviderRegistryHandle>,
    identity: SessionServiceExternalProviderIdentity,
    session_id: &str,
    source: &ReplaceSource,
    preimage_sha256: Option<&str>,
) -> Result<ReplaceReceipt, ReplaceError> {
    let registry = registry_handle_validator::replace_registry_handle(provider_registry)
        .map_err(service_error_mapper::replace_adapter_error)?
        .current();
    let identity = identity::map_identity(identity);
    let bytes = replace_input_accessor::read_replace_source(source)?;
    let records = crate::session_replace::parse_provider_owned_canonical_input_for_session(
        session_id, &bytes,
    )?;
    let describe = provider_registry_accessor::describe_provider(registry.as_ref(), &identity)
        .map_err(service_error_mapper::replace_registry_error)?;
    capability_gate::require_session_capability(&describe)
        .map_err(service_error_mapper::replace_adapter_error)?;
    let provider_instance_id = identity_formatter::provider_instance_id(&describe.provider_id);
    let settings_id = identity_formatter::settings_id(&describe, &identity.settings_id);
    let identity = identity::map_described_identity(identity, provider_instance_id, settings_id);
    let client =
        provider_registry_accessor::provider_client_for_model(registry.as_ref(), &identity)
            .map_err(registry_transport_mapper::registry_error_as_transport)
            .map_err(service_error_mapper::replace_client_error)?;
    let data_root = provider_owned_data_root()?;
    let lock = provider_owned_session_lock(&data_root)?;
    let lease = ProviderOwnedReplaceLease::acquire(&lock, session_id, &identity.provider_name)?;
    let operation_id = generate_provider_owned_operation_id();
    let input_bytes = bytes;
    let data_base64 = replace_input_formatter::data_base64(&input_bytes);
    let records_sha256 = replace_input_formatter::records_sha256(&input_bytes);
    let input = replace_input_mapper::map_prepared_replace_input(
        input_bytes,
        data_base64,
        records_sha256,
        records.len() as u64,
        preimage_sha256.map(str::to_string),
        operation_id.clone(),
    );
    let pending_journal = ProviderOwnedJournalPublication::publish_initial(
        &data_root,
        &identity,
        session_id,
        &operation_id,
        lease,
    )?;
    let request_id = request_id_formatter::session_request_id("replace");
    let request = match request_builder::build_replace_request(
        &identity,
        session_id,
        &input,
        registry.host_options(),
        request_id,
    ) {
        Ok(request) => request,
        Err(error) => {
            let mapped = service_error_mapper::replace_adapter_error(error);
            return Err(pending_journal.retire_after_error(mapped));
        }
    };
    pending_journal.require_current()?;
    let result = match client_invoker::invoke_replace(&client, request) {
        Ok(result) => result,
        Err(error) => {
            let indeterminate = error.is_host_transport_or_protocol();
            let mapped = service_error_mapper::replace_client_error(error);
            return Err(if indeterminate {
                pending_journal.retain_after_error(mapped)
            } else {
                pending_journal.retire_after_error(mapped)
            });
        }
    };
    if !result.changed {
        pending_journal.retire()?;
        pending_journal.release_lease()?;
        return Ok(replace_no_change_formatter::no_change_receipt(
            identity, session_id, input,
        ));
    }
    if let Some(recovery_id) = result.recovery_id.as_deref() {
        if let Err(error) = pending_journal.update_recovery_id(recovery_id) {
            return Err(pending_journal.retain_after_error(ReplaceError::OperationalError {
                message: format!(
                    "failed to persist observed provider recovery identity {recovery_id:?}: {error:?}"
                ),
            }));
        }
        maybe_provider_owned_test_hook(TEST_STOP_AFTER_RECOVERY_ID)?;
    }
    let accepted = match replace_result_mapper::validate_changed_replace_result(
        &identity, session_id, &input, &result,
    ) {
        Ok(accepted) => accepted,
        Err(error) => {
            let mapped = service_error_mapper::replace_adapter_error(error);
            return Err(pending_journal.retain_after_error(mapped));
        }
    };
    if let Err(error) = validate_accepted_host_state_consistency(&accepted) {
        return Err(pending_journal.retain_after_error(error));
    }
    maybe_sleep_provider_owned_test_hook(TEST_SLEEP_BEFORE_DB_POSTIMAGE_PREFIX)?;
    let journal = match pending_journal.record_db_postimage(&accepted.records, &accepted.source_id)
    {
        Ok(journal) => journal,
        Err(error) => return Err(pending_journal.retain_after_error(error)),
    };
    pending_journal.require_current()?;
    let receipt = match replace_host_apply::apply_provider_owned_replace_to_target(
        &identity,
        session_id,
        &accepted,
        &pending_journal.db_target,
        &provider_owned_admissible_db_states(&journal),
    )
    .map_err(|error| map_provider_owned_reconciliation_error(&pending_journal.pending_path, error))
    {
        Ok(receipt) => receipt,
        Err(error) => return Err(pending_journal.retain_after_error(error)),
    };
    if let Err(error) = pending_journal.mark_db_applied() {
        return Err(pending_journal.retain_after_error(error));
    }
    maybe_provider_owned_test_hook(TEST_STOP_AFTER_DB_APPLY_MARKER)?;
    if accepted.operation_state == "prepared" {
        send_provider_owned_recovery_action(
            &client,
            &identity,
            ProviderOwnedRecoveryJournalContext {
                journal: &journal,
                path: &pending_journal.pending_path,
                lease: pending_journal.authority()?,
            },
            "commit",
            Some(&input),
            registry.host_options(),
        )?;
    }
    pending_journal.retire()?;
    pending_journal.release_lease()?;
    Ok(receipt)
}

pub fn recover_pending_provider_owned_replaces(
    provider_registry: ProviderRegistryHandle,
) -> Result<(), ReplaceError> {
    let registry = provider_registry.current();
    let data_root = provider_owned_data_root()?;
    let journal_root = data_root.join("replace_journal");
    if !journal_root.exists() {
        return Ok(());
    }
    let lock = provider_owned_session_lock(&data_root)?;
    for path in provider_owned_journal_entry_paths(&journal_root)? {
        if !is_pending_provider_owned_journal_filename(&path) {
            continue;
        }
        let session_id = provider_owned_session_id_from_pending_path(&path)?;
        let lease = ProviderOwnedReplaceLease::acquire(
            &lock,
            &session_id,
            PROVIDER_OWNED_JOURNAL_OPERATION,
        )?;
        maybe_sleep_provider_owned_recovery_after_lease()?;
        recover_provider_owned_journal_candidate(registry.as_ref(), &path, &session_id, &lease)?;
        lease.release()?;
    }
    Ok(())
}

fn recover_provider_owned_journal_candidate(
    registry: &crate::provider_registry::ProviderRegistry,
    path: &Path,
    leased_session_id: &str,
    lease: &ProviderOwnedReplaceLease<'_>,
) -> Result<(), ReplaceError> {
    lease.require_active()?;
    let bytes = read_provider_owned_pending_bytes(path)?;
    let header = parse_provider_owned_journal_header(path, &bytes)?;
    if header.operation != PROVIDER_OWNED_JOURNAL_OPERATION {
        if is_known_host_owned_pending_journal_header(&header) {
            return Ok(());
        }
        return Err(operator_recovery_required(
            path,
            format!(
                "unknown pending replacement journal operation {:?} schema {}",
                header.operation, header.schema_version
            ),
        ));
    }
    if header.schema_version != 3 {
        return Err(operator_recovery_required(
            path,
            format!(
                "provider-owned journal schema {} cannot be replayed automatically because it lacks the immutable active-segment generation",
                header.schema_version
            ),
        ));
    }
    let journal = parse_provider_owned_pending_journal(path, &bytes)?;
    if journal.session_id != leased_session_id {
        return Err(operator_recovery_required(
            path,
            format!(
                "provider-owned journal session {} does not match leased session {leased_session_id}",
                journal.session_id
            ),
        ));
    }
    if journal.reconciliation_state == PROVIDER_OWNED_RECONCILIATION_CLEANUP_ONLY {
        return retire_provider_owned_journal(path, &journal, lease);
    }
    if journal.reconciliation_state != PROVIDER_OWNED_RECONCILIATION_PENDING {
        return Err(operator_recovery_required(
            path,
            format!(
                "unknown provider-owned reconciliation state {:?}",
                journal.reconciliation_state
            ),
        ));
    }
    recover_provider_owned_journal(registry, path, journal, lease)
}

fn recover_provider_owned_journal(
    registry: &crate::provider_registry::ProviderRegistry,
    path: &Path,
    mut journal: ProviderOwnedReplaceJournal,
    lease: &ProviderOwnedReplaceLease<'_>,
) -> Result<(), ReplaceError> {
    let identity = map_provider_owned_journal_identity(&journal);
    let client = match provider_registry_accessor::provider_client_for_model(registry, &identity)
        .map_err(|error| ReplaceError::OperationalError {
            message: format!("provider_owned_recovery_unavailable: {error}"),
        }) {
        Ok(client) => client,
        Err(error) => {
            return Err(retain_provider_owned_failure(
                path,
                &journal,
                lease,
                format!("{error:?}"),
            ));
        }
    };
    let recovery_input = debug_recovery_canonical_input(&journal.session_id)?;
    let query = match request_builder::build_recovery_replace_request(
        &identity,
        &journal.session_id,
        request_builder::RecoveryReplaceRequest {
            operation_id: &journal.operation_id,
            recovery_id: journal.recovery_id.as_deref(),
            action: "query",
            input: recovery_input.as_ref(),
        },
        registry.host_options(),
        request_id_formatter::session_request_id("replace"),
    )
    .map_err(service_error_mapper::replace_adapter_error)
    {
        Ok(query) => query,
        Err(error) => {
            return Err(retain_provider_owned_failure(
                path,
                &journal,
                lease,
                format!("failed to build provider recovery query: {error:?}"),
            ));
        }
    };
    lease.require_current_journal(path, &journal.operation_id)?;
    let query_result = match client_invoker::invoke_replace(&client, query) {
        Ok(result) => result,
        Err(error) => {
            let context = format!("provider_owned_recovery_unavailable: {error}");
            return Err(retain_provider_owned_failure(
                path, &journal, lease, context,
            ));
        }
    };
    let state = match query_result.operation_state.as_deref() {
        Some(state @ ("prepared" | "committed" | "atomic_committed" | "rolled_back")) => {
            state.to_string()
        }
        Some(state) => {
            return Err(retain_provider_owned_failure(
                path,
                &journal,
                lease,
                format!("unsupported provider query operation_state {state:?}"),
            ));
        }
        None => {
            return Err(retain_provider_owned_failure(
                path,
                &journal,
                lease,
                "missing_operation_state in provider query response".to_string(),
            ));
        }
    };
    let accepted = match validate_provider_owned_recovery_evidence(
        &identity,
        &journal,
        &query_result,
        recovery_input.as_ref(),
    ) {
        Ok(accepted) => accepted,
        Err(error) => {
            return Err(retain_provider_owned_failure(
                path,
                &journal,
                lease,
                format!("invalid provider query evidence: {error:?}"),
            ));
        }
    };
    if let Some(expected_recovery_id) = journal.recovery_id.as_deref()
        && accepted.recovery_id != expected_recovery_id
    {
        return Err(retain_provider_owned_failure(
            path,
            &journal,
            lease,
            format!(
                "provider query recovery_id_mismatch: expected {expected_recovery_id:?}, observed {:?}",
                accepted.recovery_id
            ),
        ));
    }
    if journal.recovery_id.is_none() {
        journal.recovery_id = Some(accepted.recovery_id.clone());
        write_provider_owned_journal(
            path,
            &journal,
            "fail-recovery-id-journal-replace",
            Some(&journal.operation_id),
            lease,
        )
        .map_err(|error| {
            retain_provider_owned_failure(
                path,
                &journal,
                lease,
                format!(
                    "failed to persist observed provider recovery identity {:?}: {error:?}",
                    journal.recovery_id
                ),
            )
        })?;
    }
    if let Err(error) = validate_accepted_host_state_consistency(&accepted) {
        return Err(retain_provider_owned_failure(
            path,
            &journal,
            lease,
            format!("invalid provider host state evidence: {error:?}"),
        ));
    }
    let postimage = crate::session_replace::provider_replace_db_postimage(
        &map_provider_owned_journal_db_target(&journal),
        &accepted.records,
        &accepted.source_id,
    )
    .map_err(|error| retain_provider_owned_failure(path, &journal, lease, format!("{error:?}")))?;
    journal =
        publish_provider_owned_db_postimage(path, &journal, postimage, lease).map_err(|error| {
            retain_provider_owned_failure(path, &journal, lease, format!("{error:?}"))
        })?;
    let admissible = provider_owned_admissible_db_states(&journal);
    match state.as_str() {
        "rolled_back" => {
            lease.require_current_journal(path, &journal.operation_id)?;
            restore_provider_owned_db_from_journal(&journal, &admissible)
                .map_err(|error| map_provider_owned_reconciliation_error(path, error))
                .map_err(|error| {
                    retain_provider_owned_failure(path, &journal, lease, format!("{error:?}"))
                })?;
            send_provider_owned_recovery_action(
                &client,
                &identity,
                ProviderOwnedRecoveryJournalContext {
                    journal: &journal,
                    path,
                    lease,
                },
                "rollback",
                None,
                registry.host_options(),
            )?;
            retire_provider_owned_journal(path, &journal, lease)?;
        }
        "prepared" => {
            lease.require_current_journal(path, &journal.operation_id)?;
            replace_host_apply::apply_provider_owned_replace_to_target(
                &identity,
                &journal.session_id,
                &accepted,
                &map_provider_owned_journal_db_target(&journal),
                &admissible,
            )
            .map_err(|error| map_provider_owned_reconciliation_error(path, error))
            .map_err(|error| {
                retain_provider_owned_failure(path, &journal, lease, format!("{error:?}"))
            })?;
            mark_provider_owned_journal_path(
                path,
                "applied",
                journal.recovery_id.as_deref(),
                &journal.operation_id,
                lease,
            )
            .map_err(|error| {
                retain_provider_owned_failure(path, &journal, lease, format!("{error:?}"))
            })?;
            send_provider_owned_recovery_action(
                &client,
                &identity,
                ProviderOwnedRecoveryJournalContext {
                    journal: &journal,
                    path,
                    lease,
                },
                "commit",
                recovery_input.as_ref(),
                registry.host_options(),
            )?;
            journal.db_apply_marker = "applied".to_string();
            retire_provider_owned_journal(path, &journal, lease)?;
        }
        "atomic_committed" | "committed" => {
            lease.require_current_journal(path, &journal.operation_id)?;
            replace_host_apply::apply_provider_owned_replace_to_target(
                &identity,
                &journal.session_id,
                &accepted,
                &map_provider_owned_journal_db_target(&journal),
                &admissible,
            )
            .map_err(|error| map_provider_owned_reconciliation_error(path, error))
            .map_err(|error| {
                retain_provider_owned_failure(path, &journal, lease, format!("{error:?}"))
            })?;
            mark_provider_owned_journal_path(
                path,
                "applied",
                journal.recovery_id.as_deref(),
                &journal.operation_id,
                lease,
            )
            .map_err(|error| {
                retain_provider_owned_failure(path, &journal, lease, format!("{error:?}"))
            })?;
            journal.db_apply_marker = "applied".to_string();
            retire_provider_owned_journal(path, &journal, lease)?;
        }
        _ => unreachable!("query state was recognized before reconciliation"),
    }
    Ok(())
}

struct ProviderOwnedRecoveryJournalContext<'a, 'lease> {
    journal: &'a ProviderOwnedReplaceJournal,
    path: &'a Path,
    lease: &'a ProviderOwnedReplaceLease<'lease>,
}

fn send_provider_owned_recovery_action(
    client: &oulipoly_provider::client::ProviderClient,
    identity: &identity::ExternalSessionIdentity,
    context: ProviderOwnedRecoveryJournalContext<'_, '_>,
    action: &str,
    input: Option<&replace_input_mapper::PreparedReplaceInput>,
    host_options: &crate::provider_registry::DescribeHostOptions,
) -> Result<(), ReplaceError> {
    let ProviderOwnedRecoveryJournalContext {
        journal,
        path,
        lease,
    } = context;
    let recovery_id = journal.recovery_id.as_deref().ok_or_else(|| {
        operator_recovery_required(
            path,
            format!(
                "provider-owned operation {:?} has no recovery identity for corrective {action}",
                journal.operation_id
            ),
        )
    })?;
    let request = request_builder::build_recovery_replace_request(
        identity,
        &journal.session_id,
        request_builder::RecoveryReplaceRequest {
            operation_id: &journal.operation_id,
            recovery_id: Some(recovery_id),
            action,
            input,
        },
        host_options,
        request_id_formatter::session_request_id("replace"),
    )
    .map_err(service_error_mapper::replace_adapter_error);
    let request = match request {
        Ok(request) => request,
        Err(error) => {
            return Err(retain_provider_owned_failure(
                path,
                journal,
                lease,
                format!("failed to build provider corrective {action}: {error:?}"),
            ));
        }
    };
    lease.require_current_journal(path, &journal.operation_id)?;
    let result = match client_invoker::invoke_replace(client, request) {
        Ok(result) => result,
        Err(error) => {
            let journal = lease.require_current_journal(path, &journal.operation_id)?;
            return Err(retain_provider_owned_failure(
                path,
                &journal,
                lease,
                format!("provider corrective {action} failed: {error}"),
            ));
        }
    };
    if result.operation_id.as_deref() != Some(journal.operation_id.as_str()) {
        let current_journal = lease.require_current_journal(path, &journal.operation_id)?;
        return Err(retain_provider_owned_failure(
            path,
            &current_journal,
            lease,
            format!(
                "corrective_operation_id_mismatch: expected {:?}, observed {:?}",
                journal.operation_id, result.operation_id
            ),
        ));
    }
    if result.recovery_id.as_deref() != Some(recovery_id) {
        let current_journal = lease.require_current_journal(path, &journal.operation_id)?;
        return Err(retain_provider_owned_failure(
            path,
            &current_journal,
            lease,
            format!(
                "corrective_recovery_id_mismatch: expected {recovery_id:?}, observed {:?}",
                result.recovery_id
            ),
        ));
    }
    let terminal = match action {
        "commit" => matches!(
            result.operation_state.as_deref(),
            Some("committed" | "atomic_committed")
        ),
        "rollback" => result.operation_state.as_deref() == Some("rolled_back"),
        _ => false,
    };
    if !terminal {
        let current_journal = lease.require_current_journal(path, &journal.operation_id)?;
        return Err(retain_provider_owned_failure(
            path,
            &current_journal,
            lease,
            format!(
                "corrective_state_mismatch: action {action:?} observed nonterminal state {:?}",
                result.operation_state
            ),
        ));
    }
    Ok(())
}

fn debug_recovery_canonical_input(
    session_id: &str,
) -> Result<Option<replace_input_mapper::PreparedReplaceInput>, ReplaceError> {
    let Some(bytes) = read_debug_recovery_canonical_input()? else {
        return Ok(None);
    };
    let records = parse_debug_recovery_canonical_records(session_id, &bytes)?;
    let (data_base64, records_sha256) = format_debug_recovery_canonical_facts(&bytes);
    Ok(Some(map_debug_recovery_prepared_input(
        bytes,
        data_base64,
        records_sha256,
        records.len() as u64,
    )))
}

fn recovery_input_from_result(
    journal: &ProviderOwnedReplaceJournal,
    result: &oulipoly_provider::generated::SessionReplaceResult,
) -> replace_input_mapper::PreparedReplaceInput {
    let turn_count = result
        .canonical_postimage
        .as_ref()
        .map(|postimage| postimage.turn_count)
        .unwrap_or_default();
    let records_sha256 = result.postimage_sha256.clone().unwrap_or_default();
    replace_input_mapper::map_prepared_replace_input(
        Vec::new(),
        String::new(),
        records_sha256,
        turn_count,
        None,
        journal.operation_id.clone(),
    )
}

fn validate_provider_owned_recovery_evidence(
    identity: &identity::ExternalSessionIdentity,
    journal: &ProviderOwnedReplaceJournal,
    result: &oulipoly_provider::generated::SessionReplaceResult,
    recovery_input: Option<&replace_input_mapper::PreparedReplaceInput>,
) -> Result<replace_result_mapper::AcceptedProviderOwnedReplaceEvidence, ReplaceError> {
    let mut input = recovery_input
        .cloned()
        .unwrap_or_else(|| recovery_input_from_result(journal, result));
    input.operation_id = journal.operation_id.clone();
    replace_result_mapper::validate_recovery_replace_result(
        identity,
        &journal.session_id,
        &input,
        result,
    )
    .map_err(service_error_mapper::replace_adapter_error)
}

fn restore_provider_owned_db_from_journal(
    journal: &ProviderOwnedReplaceJournal,
    admissible_current: &[ProviderReplaceDbPreimage],
) -> Result<(), ReplaceError> {
    let target = map_provider_owned_journal_db_target(journal);
    crate::session_replace::restore_provider_owned_db_preimage_if_current(
        &target,
        &journal.db_preimage,
        admissible_current,
    )
}

fn validate_accepted_host_state_consistency(
    accepted: &replace_result_mapper::AcceptedProviderOwnedReplaceEvidence,
) -> Result<(), ReplaceError> {
    let expected_last_turn_id = accepted
        .records
        .last()
        .map(|record| record.turn_id.as_str())
        .unwrap_or_default();
    if accepted.last_turn_id != expected_last_turn_id {
        return Err(ReplaceError::OperationalError {
            message: "invalid_host_state_plan".to_string(),
        });
    }

    let expected_last_used_at = accepted
        .records
        .last()
        .map(|record| record.timestamp.as_str())
        .unwrap_or_default();
    if accepted.last_used_at != expected_last_used_at {
        return Err(ReplaceError::OperationalError {
            message: "invalid_host_state_plan".to_string(),
        });
    }

    Ok(())
}

fn provider_owned_journal_entry_paths(journal_root: &Path) -> Result<Vec<PathBuf>, ReplaceError> {
    fs::read_dir(journal_root)
        .map_err(|e| ReplaceError::OperationalError {
            message: format!("failed to scan provider-owned replace journal: {e}"),
        })?
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|e| ReplaceError::OperationalError {
                    message: format!("failed to read provider-owned replace journal entry: {e}"),
                })
        })
        .collect()
}

fn is_pending_provider_owned_journal_filename(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("session-") && name.ends_with(".pending"))
}

fn provider_owned_session_id_from_pending_path(path: &Path) -> Result<String, ReplaceError> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            operator_recovery_required(
                path,
                "pending replacement journal filename is not valid UTF-8".to_string(),
            )
        })?;
    let session_id = name
        .strip_prefix("session-")
        .and_then(|name| name.strip_suffix(".pending"))
        .and_then(|name| name.split('.').next())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            operator_recovery_required(
                path,
                "pending replacement journal filename has no session identity".to_string(),
            )
        })?;
    Uuid::try_parse(session_id).map_err(|_| {
        operator_recovery_required(
            path,
            format!(
                "pending replacement journal filename has invalid session identity {session_id:?}"
            ),
        )
    })?;
    Ok(session_id.to_string())
}

fn read_provider_owned_pending_bytes(path: &Path) -> Result<Vec<u8>, ReplaceError> {
    fs::read(path).map_err(|error| {
        operator_recovery_required(
            path,
            format!("matching pending replacement journal is unreadable: {error}"),
        )
    })
}

fn parse_provider_owned_journal_header(
    path: &Path,
    bytes: &[u8],
) -> Result<ProviderOwnedJournalHeader, ReplaceError> {
    serde_json::from_slice(bytes).map_err(|error| {
        operator_recovery_required(
            path,
            format!("matching pending replacement journal is malformed or partial: {error}"),
        )
    })
}

fn is_known_host_owned_pending_journal_header(header: &ProviderOwnedJournalHeader) -> bool {
    matches!(header.schema_version, 1 | 2) && header.operation == "import-replace"
}

fn parse_provider_owned_pending_journal(
    path: &Path,
    bytes: &[u8],
) -> Result<ProviderOwnedReplaceJournal, ReplaceError> {
    serde_json::from_slice(bytes).map_err(|error| {
        operator_recovery_required(
            path,
            format!("provider-owned replacement journal is incomplete: {error}"),
        )
    })
}

fn operator_recovery_required(path: &Path, reason: String) -> ReplaceError {
    ReplaceError::OperatorRecoveryRequired {
        journal_path: path.to_path_buf(),
        reason,
    }
}

fn map_provider_owned_journal_identity(
    journal: &ProviderOwnedReplaceJournal,
) -> identity::ExternalSessionIdentity {
    identity::ExternalSessionIdentity {
        model_name: journal.model_name.clone(),
        provider_name: journal.provider_name.clone(),
        provider_instance_id: Some(journal.provider_instance_id.clone()),
        settings_id: journal.settings_id.clone(),
    }
}

fn read_debug_recovery_canonical_input() -> Result<Option<Vec<u8>>, ReplaceError> {
    if !cfg!(debug_assertions) {
        return Ok(None);
    }
    let path = provider_owned_data_root()?.join("replacement-canonical.jsonl");
    if !path.exists() {
        return Ok(None);
    }
    fs::read(path)
        .map(Some)
        .map_err(|e| ReplaceError::OperationalError {
            message: format!("failed to read debug recovery canonical input: {e}"),
        })
}

fn parse_debug_recovery_canonical_records(
    session_id: &str,
    bytes: &[u8],
) -> Result<Vec<crate::session_replace::CanonicalRecord>, ReplaceError> {
    crate::session_replace::parse_provider_owned_canonical_input_for_session(session_id, bytes)
}

fn format_debug_recovery_canonical_facts(bytes: &[u8]) -> (String, String) {
    (
        replace_input_formatter::data_base64(bytes),
        replace_input_formatter::records_sha256(bytes),
    )
}

fn map_debug_recovery_prepared_input(
    bytes: Vec<u8>,
    data_base64: String,
    records_sha256: String,
    turn_count: u64,
) -> replace_input_mapper::PreparedReplaceInput {
    replace_input_mapper::map_prepared_replace_input(
        bytes,
        data_base64,
        records_sha256,
        turn_count,
        None,
        generate_provider_owned_operation_id(),
    )
}

fn map_provider_owned_journal_db_target(
    journal: &ProviderOwnedReplaceJournal,
) -> ProviderReplaceDbTarget {
    ProviderReplaceDbTarget {
        provider_name: journal.provider_name.clone(),
        session_id: journal.session_id.clone(),
        chain_id: journal.chain_id.clone(),
        active_segment_id: journal.active_segment_id,
        active_segment_started_at: journal.active_segment_started_at.clone(),
        source_file: String::new(),
    }
}

fn provider_owned_admissible_db_states(
    journal: &ProviderOwnedReplaceJournal,
) -> Vec<ProviderReplaceDbPreimage> {
    let mut states = vec![journal.db_preimage.clone()];
    if let Some(postimage) = &journal.db_postimage
        && postimage != &journal.db_preimage
    {
        states.push(postimage.clone());
    }
    states
}

fn map_provider_owned_reconciliation_error(path: &Path, error: ReplaceError) -> ReplaceError {
    if matches!(
        &error,
        ReplaceError::OperationalError { message }
            if message.contains("session_turn_reconciliation_precondition_mismatch")
    ) {
        return operator_recovery_required(
            path,
            format!("provider_owned_reconciliation_precondition_mismatch: {error:?}"),
        );
    }
    error
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProviderOwnedJournalHeader {
    schema_version: u32,
    operation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ProviderOwnedReplaceJournal {
    schema_version: u32,
    operation: String,
    operation_id: String,
    started_at: String,
    settings_id: String,
    model_name: String,
    provider_name: String,
    provider_instance_id: String,
    session_id: String,
    chain_id: String,
    active_segment_id: i64,
    active_segment_started_at: String,
    db_apply_marker: String,
    #[serde(default = "provider_owned_pending_reconciliation_state")]
    reconciliation_state: String,
    db_preimage: ProviderReplaceDbPreimage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    db_postimage: Option<ProviderReplaceDbPreimage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recovery_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    failure_context: Option<String>,
}

fn provider_owned_pending_reconciliation_state() -> String {
    PROVIDER_OWNED_RECONCILIATION_PENDING.to_string()
}

struct ProviderOwnedReplaceLease<'a> {
    lock: &'a SessionLock,
    lease: Option<Lease>,
    process_authority: Option<ProcessAuthority>,
}

impl<'a> ProviderOwnedReplaceLease<'a> {
    fn acquire(
        lock: &'a SessionLock,
        session_id: &str,
        provider_name: &str,
    ) -> Result<Self, ReplaceError> {
        let (process_authority, lease) = lock
            .acquire_with_process_authority(session_id, provider_name, provider_owned_lease_ttl()?)
            .map_err(map_provider_lock_error)?;
        Ok(Self {
            lock,
            lease: Some(lease),
            process_authority: Some(process_authority),
        })
    }

    fn require_active(&self) -> Result<(), ReplaceError> {
        let lease = self
            .lease
            .as_ref()
            .ok_or_else(|| ReplaceError::OperationalError {
                message: "provider-owned process authority was already released".to_string(),
            })?;
        self.process_authority
            .as_ref()
            .ok_or_else(|| ReplaceError::OperationalError {
                message: "provider-owned process authority is absent".to_string(),
            })?
            .require_session(&lease.session_id)
            .map_err(map_provider_lock_error)
    }

    fn require_current_journal(
        &self,
        path: &Path,
        operation_id: &str,
    ) -> Result<ProviderOwnedReplaceJournal, ReplaceError> {
        self.require_active()?;
        read_matching_provider_owned_journal(path, operation_id)
    }

    fn release(mut self) -> Result<(), ReplaceError> {
        if let Some(lease) = self.lease.as_ref() {
            self.lock
                .release(&lease.session_id, &lease.token)
                .map_err(map_provider_lock_error)?;
            self.lease = None;
        }
        self.process_authority = None;
        Ok(())
    }
}

impl Drop for ProviderOwnedReplaceLease<'_> {
    fn drop(&mut self) {
        if let Some(lease) = self.lease.take() {
            let _ = self.lock.release(&lease.session_id, &lease.token);
        }
        self.process_authority = None;
    }
}

struct ProviderOwnedJournalPublication<'a> {
    pending_path: PathBuf,
    db_target: ProviderReplaceDbTarget,
    operation_id: String,
    lease: Option<ProviderOwnedReplaceLease<'a>>,
}

impl<'a> ProviderOwnedJournalPublication<'a> {
    fn publish_initial(
        data_root: &Path,
        identity: &identity::ExternalSessionIdentity,
        session_id: &str,
        operation_id: &str,
        lease: ProviderOwnedReplaceLease<'a>,
    ) -> Result<Self, ReplaceError> {
        let journal_root = data_root.join("replace_journal");
        fs::create_dir_all(&journal_root).map_err(|e| ReplaceError::OperationalError {
            message: format!("failed to create provider-owned journal root: {e}"),
        })?;
        let pending_path = journal_root.join(format!("session-{session_id}.pending"));
        require_provider_owned_pending_path_absent(&pending_path)?;
        let db_target = crate::session_replace::strict_provider_replace_db_identity(
            &identity.provider_name,
            session_id,
            String::new(),
        )?;
        let db_preimage = crate::session_replace::provider_replace_db_preimage(&db_target)?;
        let journal = ProviderOwnedReplaceJournal {
            schema_version: 3,
            operation: PROVIDER_OWNED_JOURNAL_OPERATION.to_string(),
            operation_id: operation_id.to_string(),
            started_at: Utc::now().to_rfc3339(),
            settings_id: identity.settings_id.clone(),
            model_name: identity.model_name.clone(),
            provider_name: identity.provider_name.clone(),
            provider_instance_id: identity::provider_instance_id(identity),
            session_id: session_id.to_string(),
            chain_id: db_target.chain_id.clone(),
            active_segment_id: db_target.active_segment_id,
            active_segment_started_at: db_target.active_segment_started_at.clone(),
            db_apply_marker: "not_applied".to_string(),
            reconciliation_state: PROVIDER_OWNED_RECONCILIATION_PENDING.to_string(),
            db_preimage,
            db_postimage: None,
            recovery_id: None,
            failure_context: None,
        };
        write_provider_owned_journal(
            &pending_path,
            &journal,
            "fail-initial-journal-replace",
            None,
            &lease,
        )?;
        Ok(Self {
            pending_path,
            db_target,
            operation_id: operation_id.to_string(),
            lease: Some(lease),
        })
    }

    fn update_recovery_id(&self, recovery_id: &str) -> Result<(), ReplaceError> {
        let lease = self.authority()?;
        let mut journal =
            lease.require_current_journal(&self.pending_path, &self.operation_id()?)?;
        journal.recovery_id = Some(recovery_id.to_string());
        write_provider_owned_journal(
            &self.pending_path,
            &journal,
            "fail-recovery-id-journal-replace",
            Some(&journal.operation_id),
            lease,
        )
    }

    fn mark_db_applied(&self) -> Result<(), ReplaceError> {
        let lease = self.authority()?;
        let operation_id = self.operation_id()?;
        mark_provider_owned_journal_path(&self.pending_path, "applied", None, &operation_id, lease)
    }

    fn record_db_postimage(
        &self,
        records: &[crate::session_replace::CanonicalRecord],
        source_file: &str,
    ) -> Result<ProviderOwnedReplaceJournal, ReplaceError> {
        let postimage = crate::session_replace::provider_replace_db_postimage(
            &self.db_target,
            records,
            source_file,
        )?;
        let lease = self.authority()?;
        let journal = lease.require_current_journal(&self.pending_path, &self.operation_id()?)?;
        publish_provider_owned_db_postimage(&self.pending_path, &journal, postimage, lease)
    }

    fn retire(&self) -> Result<(), ReplaceError> {
        let lease = self.authority()?;
        let journal = lease.require_current_journal(&self.pending_path, &self.operation_id()?)?;
        retire_provider_owned_journal(&self.pending_path, &journal, lease)
    }

    fn retire_after_error(&self, original: ReplaceError) -> ReplaceError {
        match self.retire() {
            Ok(()) => original,
            Err(retirement) => operator_recovery_required(
                &self.pending_path,
                format!(
                    "original provider replace error: {original:?}; journal retirement failed: {retirement:?}"
                ),
            ),
        }
    }

    fn retain_after_error(&self, original: ReplaceError) -> ReplaceError {
        let lease = match self.authority() {
            Ok(lease) => lease,
            Err(error) => return error,
        };
        let journal = match lease.require_current_journal(&self.pending_path, &self.operation_id) {
            Ok(journal) => journal,
            Err(error) => return error,
        };
        retain_provider_owned_failure(&self.pending_path, &journal, lease, format!("{original:?}"))
    }

    fn require_current(&self) -> Result<ProviderOwnedReplaceJournal, ReplaceError> {
        let operation_id = self.operation_id()?;
        self.authority()?
            .require_current_journal(&self.pending_path, &operation_id)
    }

    fn operation_id(&self) -> Result<String, ReplaceError> {
        Ok(self.operation_id.clone())
    }

    fn authority(&self) -> Result<&ProviderOwnedReplaceLease<'a>, ReplaceError> {
        let lease = self
            .lease
            .as_ref()
            .ok_or_else(|| ReplaceError::OperationalError {
                message: "provider-owned journal authority was already released".to_string(),
            })?;
        lease.require_active()?;
        Ok(lease)
    }

    fn release_lease(mut self) -> Result<(), ReplaceError> {
        if let Some(lease) = self.lease.take() {
            lease.release()?;
        }
        Ok(())
    }
}

fn require_provider_owned_pending_path_absent(path: &Path) -> Result<(), ReplaceError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(operator_recovery_required(
            path,
            "retained pending replacement journal blocks new provider publication".to_string(),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(operator_recovery_required(
            path,
            format!("cannot prove pending replacement journal absence: {error}"),
        )),
    }
}

fn mark_provider_owned_journal_path(
    path: &Path,
    marker: &str,
    recovery_id: Option<&str>,
    operation_id: &str,
    lease: &ProviderOwnedReplaceLease<'_>,
) -> Result<(), ReplaceError> {
    let mut journal = lease.require_current_journal(path, operation_id)?;
    journal.db_apply_marker = marker.to_string();
    if let Some(recovery_id) = recovery_id {
        journal.recovery_id = Some(recovery_id.to_string());
    }
    write_provider_owned_journal(
        path,
        &journal,
        "fail-db-apply-journal-replace",
        Some(operation_id),
        lease,
    )
}

fn publish_provider_owned_db_postimage(
    path: &Path,
    journal: &ProviderOwnedReplaceJournal,
    postimage: ProviderReplaceDbPreimage,
    lease: &ProviderOwnedReplaceLease<'_>,
) -> Result<ProviderOwnedReplaceJournal, ReplaceError> {
    let mut next = journal.clone();
    next.db_postimage = Some(postimage);
    write_provider_owned_journal(
        path,
        &next,
        "fail-db-postimage-journal-replace",
        Some(&journal.operation_id),
        lease,
    )?;
    Ok(next)
}

fn read_provider_owned_journal(path: &Path) -> Result<ProviderOwnedReplaceJournal, ReplaceError> {
    let bytes = read_provider_owned_pending_bytes(path)?;
    parse_provider_owned_pending_journal(path, &bytes)
}

fn read_matching_provider_owned_journal(
    path: &Path,
    operation_id: &str,
) -> Result<ProviderOwnedReplaceJournal, ReplaceError> {
    let journal = read_provider_owned_journal(path)?;
    if journal.operation_id != operation_id {
        return Err(operator_recovery_required(
            path,
            format!(
                "provider-owned journal operation changed: expected {operation_id:?}, observed {:?}",
                journal.operation_id
            ),
        ));
    }
    Ok(journal)
}

fn retain_provider_owned_failure(
    path: &Path,
    journal: &ProviderOwnedReplaceJournal,
    lease: &ProviderOwnedReplaceLease<'_>,
    context: String,
) -> ReplaceError {
    let mut retained = match lease.require_current_journal(path, &journal.operation_id) {
        Ok(retained) => retained,
        Err(error) => {
            return operator_recovery_required(
                path,
                format!(
                    "provider-owned operation {:?} failed with {context}; current journal could not be joined: {error:?}",
                    journal.operation_id
                ),
            );
        }
    };
    if retained.failure_context.is_none() {
        retained.failure_context = Some(context.clone());
    }
    let persistence = write_provider_owned_journal(
        path,
        &retained,
        "fail-failure-context-journal-replace",
        Some(&journal.operation_id),
        lease,
    );
    operator_recovery_required(
        path,
        format!(
            "provider-owned operation {:?} remains pending after {context}; failure-context persistence: {persistence:?}",
            journal.operation_id
        ),
    )
}

fn write_provider_owned_journal(
    path: &Path,
    journal: &ProviderOwnedReplaceJournal,
    failure_hook: &str,
    expected_operation_id: Option<&str>,
    lease: &ProviderOwnedReplaceLease<'_>,
) -> Result<(), ReplaceError> {
    lease.require_active()?;
    let bytes = serde_json::to_vec_pretty(journal).map_err(|e| ReplaceError::OperationalError {
        message: format!("failed to serialize provider-owned journal: {e}"),
    })?;
    crate::session_replace::atomic_write_bytes_before_replace(path, &bytes, || {
        maybe_provider_owned_test_hook(failure_hook)?;
        match expected_operation_id {
            Some(operation_id) => {
                lease.require_current_journal(path, operation_id)?;
            }
            None => require_provider_owned_pending_path_absent(path)?,
        }
        Ok(())
    })
}

fn retire_provider_owned_journal(
    path: &Path,
    journal: &ProviderOwnedReplaceJournal,
    lease: &ProviderOwnedReplaceLease<'_>,
) -> Result<(), ReplaceError> {
    maybe_sleep_provider_owned_test_hook(TEST_SLEEP_BEFORE_RETIRE_PREFIX)?;
    lease.require_current_journal(path, &journal.operation_id)?;
    let mut terminal = journal.clone();
    terminal.reconciliation_state = PROVIDER_OWNED_RECONCILIATION_CLEANUP_ONLY.to_string();
    write_provider_owned_journal(
        path,
        &terminal,
        "fail-terminal-journal-replace",
        Some(&journal.operation_id),
        lease,
    )
    .map_err(|error| provider_owned_retirement_required(path, error))?;
    let observed = read_provider_owned_journal(path)
        .map_err(|error| provider_owned_retirement_required(path, error))?;
    if observed != terminal {
        return Err(operator_recovery_required(
            path,
            "provider-owned cleanup-only journal changed before checked removal".to_string(),
        ));
    }
    maybe_provider_owned_test_hook(TEST_FAIL_JOURNAL_REMOVE)
        .map_err(|error| provider_owned_retirement_required(path, error))?;
    lease.require_current_journal(path, &journal.operation_id)?;
    fs::remove_file(path).map_err(|error| {
        operator_recovery_required(
            path,
            format!("failed to remove cleanup-only provider-owned journal: {error}"),
        )
    })?;
    let parent = path.parent().ok_or_else(|| {
        operator_recovery_required(
            path,
            "provider-owned journal path has no parent directory".to_string(),
        )
    })?;
    if let Err(error) = sync_provider_owned_journal_directory(parent) {
        let republish = write_provider_owned_journal(path, &terminal, "", None, lease);
        return Err(operator_recovery_required(
            path,
            format!(
                "failed to durably retire provider-owned journal: {error:?}; cleanup-only identity republish: {republish:?}"
            ),
        ));
    }
    Ok(())
}

fn provider_owned_retirement_required(path: &Path, error: ReplaceError) -> ReplaceError {
    operator_recovery_required(
        path,
        format!("provider-owned journal retirement remains pending: {error:?}"),
    )
}

fn sync_provider_owned_journal_directory(path: &Path) -> Result<(), ReplaceError> {
    let directory = File::open(path).map_err(|error| ReplaceError::OperationalError {
        message: format!(
            "failed to open provider-owned journal directory {}: {error}",
            path.display()
        ),
    })?;
    directory
        .sync_all()
        .map_err(|error| ReplaceError::OperationalError {
            message: format!(
                "failed to sync provider-owned journal directory {}: {error}",
                path.display()
            ),
        })
}

fn provider_owned_data_root() -> Result<PathBuf, ReplaceError> {
    oulipoly_state::paths::data_dir().map_err(|_| ReplaceError::OperationalError {
        message: "could not determine data directory".to_string(),
    })
}

fn provider_owned_session_lock(data_root: &Path) -> Result<SessionLock, ReplaceError> {
    SessionLock::new(&data_root.join("locks")).map_err(|e| ReplaceError::OperationalError {
        message: format!("failed to initialize session lock: {e}"),
    })
}

fn map_provider_lock_error(error: LockError) -> ReplaceError {
    match error {
        LockError::Busy {
            expires_at,
            token_hash,
        } => ReplaceError::SessionBusy {
            token: token_hash.unwrap_or_default(),
            expires_at,
        },
        LockError::TokenInvalid | LockError::LockExpired => ReplaceError::OperationalError {
            message: "session lock token invalid".to_string(),
        },
        LockError::SentinelBusy { timeout_ms } => {
            ReplaceError::SessionLockSentinelBusy { timeout_ms }
        }
        LockError::Operational { message } => ReplaceError::OperationalError { message },
    }
}

fn provider_owned_lease_ttl() -> Result<Duration, ReplaceError> {
    if !cfg!(debug_assertions) {
        return Ok(Duration::from_secs(300));
    }
    let Some(value) = std::env::var_os(PROVIDER_OWNED_LEASE_TTL_ENV) else {
        return Ok(Duration::from_secs(300));
    };
    let milliseconds =
        value
            .to_string_lossy()
            .parse::<u64>()
            .map_err(|error| ReplaceError::OperationalError {
                message: format!("invalid provider-owned lease test TTL: {error}"),
            })?;
    Ok(Duration::from_millis(milliseconds))
}

fn generate_provider_owned_operation_id() -> String {
    if cfg!(debug_assertions) {
        "55555555-5555-4555-8555-555555555555".to_string()
    } else {
        Uuid::new_v4().to_string()
    }
}

fn maybe_provider_owned_test_hook(token: &str) -> Result<(), ReplaceError> {
    if cfg!(debug_assertions) && std::env::var(PROVIDER_OWNED_TEST_HOOK_ENV).as_deref() == Ok(token)
    {
        return Err(ReplaceError::OperationalError {
            message: token.to_string(),
        });
    }
    Ok(())
}

fn maybe_sleep_provider_owned_recovery_after_lease() -> Result<(), ReplaceError> {
    maybe_sleep_provider_owned_test_hook(TEST_SLEEP_RECOVERY_AFTER_LEASE_PREFIX)
}

fn maybe_sleep_provider_owned_test_hook(prefix: &str) -> Result<(), ReplaceError> {
    if !cfg!(debug_assertions) {
        return Ok(());
    }
    let Ok(value) = std::env::var(PROVIDER_OWNED_TEST_HOOK_ENV) else {
        return Ok(());
    };
    let Some(milliseconds) = value.strip_prefix(prefix) else {
        return Ok(());
    };
    let milliseconds =
        milliseconds
            .parse::<u64>()
            .map_err(|error| ReplaceError::OperationalError {
                message: format!("invalid provider-owned recovery sleep hook: {error}"),
            })?;
    std::thread::sleep(Duration::from_millis(milliseconds));
    Ok(())
}
