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
use crate::session_lock::{Lease, LockError, SessionLock};
use crate::session_replace::{
    ProviderReplaceDbPreimage, ProviderReplaceDbTarget, ReplaceError, ReplaceReceipt, ReplaceSource,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use uuid::Uuid;

const PROVIDER_OWNED_JOURNAL_OPERATION: &str = "provider-owned-import-replace";
const PROVIDER_OWNED_TEST_HOOK_ENV: &str = "OULIPOLY_PROVIDER_OWNED_REPLACE_TEST_HOOK";
const TEST_STOP_AFTER_RECOVERY_ID: &str = "stop-after-recovery-id-journal-update";
const TEST_STOP_AFTER_DB_APPLY_MARKER: &str = "stop-after-db-apply-marker";

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
    let request = request_builder::build_export_request(&identity, session_id, request_id)
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
    let lease = acquire_provider_owned_lease(&lock, session_id, &identity.provider_name)?;
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
    )?;
    let request_id = request_id_formatter::session_request_id("replace");
    let request =
        match request_builder::build_replace_request(&identity, session_id, &input, request_id) {
            Ok(request) => request,
            Err(error) => {
                let mapped = service_error_mapper::replace_adapter_error(error);
                pending_journal.cleanup();
                release_provider_owned_lease(&lock, &lease).ok();
                return Err(mapped);
            }
        };
    let result = match client_invoker::invoke_replace(&client, request) {
        Ok(result) => result,
        Err(error) => {
            let mapped = service_error_mapper::replace_client_error(error);
            pending_journal.cleanup();
            release_provider_owned_lease(&lock, &lease).ok();
            return Err(mapped);
        }
    };
    if !result.changed {
        pending_journal.cleanup();
        release_provider_owned_lease(&lock, &lease)?;
        return Ok(replace_no_change_formatter::no_change_receipt(
            identity, session_id, input,
        ));
    }
    if let Some(recovery_id) = result.recovery_id.as_deref() {
        pending_journal.update_recovery_id(recovery_id)?;
        maybe_provider_owned_test_hook(TEST_STOP_AFTER_RECOVERY_ID)?;
    }
    let accepted = match replace_result_mapper::validate_changed_replace_result(
        &identity, session_id, &input, &result,
    ) {
        Ok(accepted) => accepted,
        Err(error) => {
            let lacks_recovery_identity = matches!(
                error.token,
                "missing_operation_id" | "operation_id_mismatch" | "missing_recovery_id"
            );
            let mapped = service_error_mapper::replace_adapter_error(error);
            if lacks_recovery_identity || result.recovery_id.is_none() {
                pending_journal.cleanup();
            }
            release_provider_owned_lease(&lock, &lease).ok();
            return Err(mapped);
        }
    };
    validate_accepted_host_state_consistency(&accepted)?;
    let receipt =
        replace_host_apply::apply_provider_owned_replace(&identity, session_id, &accepted)?;
    pending_journal.mark_db_applied()?;
    maybe_provider_owned_test_hook(TEST_STOP_AFTER_DB_APPLY_MARKER)?;
    if accepted.operation_state == "prepared" {
        let request = request_builder::build_recovery_replace_request_with_input(
            &identity,
            session_id,
            &accepted.operation_id,
            Some(&accepted.recovery_id),
            "commit",
            Some(&input),
            request_id_formatter::session_request_id("replace"),
        )
        .map_err(service_error_mapper::replace_adapter_error)?;
        client_invoker::invoke_replace(&client, request)
            .map_err(service_error_mapper::replace_client_error)?;
    }
    pending_journal.cleanup();
    release_provider_owned_lease(&lock, &lease)?;
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
    let mut pending = Vec::new();
    for path in provider_owned_journal_entry_paths(&journal_root)? {
        if !is_pending_provider_owned_journal_filename(&path) {
            continue;
        }
        let Some(bytes) = read_optional_provider_owned_journal_bytes(&path) else {
            continue;
        };
        let Some(header) = parse_provider_owned_journal_header(&bytes) else {
            continue;
        };
        if is_provider_owned_pending_journal_header(&header) {
            let journal = parse_provider_owned_pending_journal(&bytes)?;
            pending.push((path, journal));
        }
    }
    for (path, journal) in pending {
        recover_provider_owned_journal(registry.as_ref(), &path, journal)?;
    }
    Ok(())
}

fn recover_provider_owned_journal(
    registry: &crate::provider_registry::ProviderRegistry,
    path: &Path,
    journal: ProviderOwnedReplaceJournal,
) -> Result<(), ReplaceError> {
    let identity = map_provider_owned_journal_identity(&journal);
    let client = provider_registry_accessor::provider_client_for_model(registry, &identity)
        .map_err(|error| ReplaceError::OperationalError {
            message: format!("provider_owned_recovery_unavailable: {error}"),
        })?;
    let recovery_input = debug_recovery_canonical_input(&journal.session_id)?;
    let query = request_builder::build_recovery_replace_request_with_input(
        &identity,
        &journal.session_id,
        &journal.operation_id,
        journal.recovery_id.as_deref(),
        "query",
        recovery_input.as_ref(),
        request_id_formatter::session_request_id("replace"),
    )
    .map_err(service_error_mapper::replace_adapter_error)?;
    let query_result = client_invoker::invoke_replace(&client, query).map_err(|error| {
        ReplaceError::OperationalError {
            message: format!("provider_owned_recovery_unavailable: {error}"),
        }
    })?;
    let state = query_result
        .operation_state
        .as_deref()
        .unwrap_or("prepared");
    match (journal.db_apply_marker.as_str(), state) {
        (_, "rolled_back") => {
            restore_provider_owned_db_from_journal(&journal)?;
            send_provider_owned_recovery_action(&client, &identity, &journal, "rollback", None)?;
            cleanup_provider_owned_journal(path)?;
        }
        ("not_applied", "prepared") => {
            let accepted = replace_result_mapper::validate_changed_replace_result(
                &identity,
                &journal.session_id,
                &recovery_input_from_result(&journal, &query_result),
                &query_result,
            )
            .map_err(service_error_mapper::replace_adapter_error)?;
            replace_host_apply::apply_provider_owned_replace(
                &identity,
                &journal.session_id,
                &accepted,
            )?;
            mark_provider_owned_journal_path(path, "applied", journal.recovery_id.as_deref())?;
            send_provider_owned_recovery_action(
                &client,
                &identity,
                &journal,
                "commit",
                recovery_input.as_ref(),
            )?;
            cleanup_provider_owned_journal(path)?;
        }
        ("applied", "prepared") => {
            if provider_owned_current_db_equals_journal_preimage(&journal)? {
                let accepted = replace_result_mapper::validate_changed_replace_result(
                    &identity,
                    &journal.session_id,
                    &recovery_input_from_result(&journal, &query_result),
                    &query_result,
                )
                .map_err(service_error_mapper::replace_adapter_error)?;
                replace_host_apply::apply_provider_owned_replace(
                    &identity,
                    &journal.session_id,
                    &accepted,
                )?;
                send_provider_owned_recovery_action(
                    &client,
                    &identity,
                    &journal,
                    "commit",
                    recovery_input.as_ref(),
                )?;
            } else {
                restore_provider_owned_db_from_journal(&journal)?;
                send_provider_owned_recovery_action(
                    &client, &identity, &journal, "rollback", None,
                )?;
            }
            cleanup_provider_owned_journal(path)?;
        }
        (_, "atomic_committed" | "committed") => {
            if journal.db_apply_marker != "applied"
                || provider_owned_current_db_equals_journal_preimage(&journal)?
            {
                let accepted = replace_result_mapper::validate_changed_replace_result(
                    &identity,
                    &journal.session_id,
                    &recovery_input_from_result(&journal, &query_result),
                    &query_result,
                )
                .map_err(service_error_mapper::replace_adapter_error)?;
                replace_host_apply::apply_provider_owned_replace(
                    &identity,
                    &journal.session_id,
                    &accepted,
                )?;
            }
            cleanup_provider_owned_journal(path)?;
        }
        (_, _) => {
            restore_provider_owned_db_from_journal(&journal)?;
            send_provider_owned_recovery_action(
                &client,
                &identity,
                &journal,
                "rollback",
                recovery_input.as_ref(),
            )?;
            cleanup_provider_owned_journal(path)?;
        }
    }
    Ok(())
}

fn send_provider_owned_recovery_action(
    client: &oulipoly_provider::client::ProviderClient,
    identity: &identity::ExternalSessionIdentity,
    journal: &ProviderOwnedReplaceJournal,
    action: &str,
    input: Option<&replace_input_mapper::PreparedReplaceInput>,
) -> Result<(), ReplaceError> {
    let request = if let Some(input) = input {
        request_builder::build_recovery_replace_request_with_input(
            identity,
            &journal.session_id,
            &journal.operation_id,
            journal.recovery_id.as_deref(),
            action,
            Some(input),
            request_id_formatter::session_request_id("replace"),
        )
    } else {
        request_builder::build_recovery_replace_request(
            identity,
            &journal.session_id,
            &journal.operation_id,
            journal.recovery_id.as_deref(),
            action,
            request_id_formatter::session_request_id("replace"),
        )
    }
    .map_err(service_error_mapper::replace_adapter_error)?;
    client_invoker::invoke_replace(client, request)
        .map(|_| ())
        .map_err(service_error_mapper::replace_client_error)
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

fn restore_provider_owned_db_from_journal(
    journal: &ProviderOwnedReplaceJournal,
) -> Result<(), ReplaceError> {
    let target = map_provider_owned_journal_db_target(journal);
    crate::session_replace::restore_provider_owned_db_preimage(&target, &journal.db_preimage)
}

fn provider_owned_current_db_equals_journal_preimage(
    journal: &ProviderOwnedReplaceJournal,
) -> Result<bool, ReplaceError> {
    let target = map_provider_owned_journal_db_target(journal);
    let current = read_provider_owned_db_preimage(&target)?;
    Ok(provider_owned_db_preimage_matches(
        &current,
        &journal.db_preimage,
    ))
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

fn read_optional_provider_owned_journal_bytes(path: &Path) -> Option<Vec<u8>> {
    fs::read(path).ok()
}

fn parse_provider_owned_journal_header(bytes: &[u8]) -> Option<ProviderOwnedJournalHeader> {
    serde_json::from_slice(bytes).ok()
}

fn is_provider_owned_pending_journal_header(header: &ProviderOwnedJournalHeader) -> bool {
    header.schema_version == 2 && header.operation == PROVIDER_OWNED_JOURNAL_OPERATION
}

fn parse_provider_owned_pending_journal(
    bytes: &[u8],
) -> Result<ProviderOwnedReplaceJournal, ReplaceError> {
    serde_json::from_slice(bytes).map_err(|e| ReplaceError::OperationalError {
        message: format!("invalid provider-owned replace journal: {e}"),
    })
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
        source_file: String::new(),
    }
}

fn read_provider_owned_db_preimage(
    target: &ProviderReplaceDbTarget,
) -> Result<ProviderReplaceDbPreimage, ReplaceError> {
    crate::session_replace::provider_replace_db_preimage(target)
}

fn provider_owned_db_preimage_matches(
    current: &ProviderReplaceDbPreimage,
    expected: &ProviderReplaceDbPreimage,
) -> bool {
    current.session_turns == expected.session_turns
        && current.last_turn_id == expected.last_turn_id
        && current.last_used_at == expected.last_used_at
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProviderOwnedJournalHeader {
    schema_version: u32,
    operation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    db_apply_marker: String,
    db_preimage: ProviderReplaceDbPreimage,
    #[serde(skip_serializing_if = "Option::is_none")]
    recovery_id: Option<String>,
}

struct ProviderOwnedJournalPublication {
    pending_path: PathBuf,
}

impl ProviderOwnedJournalPublication {
    fn publish_initial(
        data_root: &Path,
        identity: &identity::ExternalSessionIdentity,
        session_id: &str,
        operation_id: &str,
    ) -> Result<Self, ReplaceError> {
        let identity_result = crate::session_replace::strict_provider_replace_db_identity(
            &identity.provider_name,
            session_id,
            String::new(),
        );
        let (chain_id, active_segment_id, db_preimage) = match identity_result {
            Ok(target) => {
                let preimage = crate::session_replace::provider_replace_db_preimage(&target)?;
                (target.chain_id, target.active_segment_id, preimage)
            }
            Err(_) => (
                String::new(),
                0,
                ProviderReplaceDbPreimage {
                    session_turns: Value::Array(Vec::new()),
                    last_turn_id: None,
                    last_used_at: String::new(),
                },
            ),
        };
        let journal = ProviderOwnedReplaceJournal {
            schema_version: 2,
            operation: PROVIDER_OWNED_JOURNAL_OPERATION.to_string(),
            operation_id: operation_id.to_string(),
            started_at: Utc::now().to_rfc3339(),
            settings_id: identity.settings_id.clone(),
            model_name: identity.model_name.clone(),
            provider_name: identity.provider_name.clone(),
            provider_instance_id: identity::provider_instance_id(identity),
            session_id: session_id.to_string(),
            chain_id,
            active_segment_id,
            db_apply_marker: "not_applied".to_string(),
            db_preimage,
            recovery_id: None,
        };
        let journal_root = data_root.join("replace_journal");
        fs::create_dir_all(&journal_root).map_err(|e| ReplaceError::OperationalError {
            message: format!("failed to create provider-owned journal root: {e}"),
        })?;
        let pending_path = journal_root.join(format!("session-{session_id}.pending"));
        write_provider_owned_journal(&pending_path, &journal)?;
        Ok(Self { pending_path })
    }

    fn update_recovery_id(&self, recovery_id: &str) -> Result<(), ReplaceError> {
        let mut journal = read_provider_owned_journal(&self.pending_path)?;
        journal.recovery_id = Some(recovery_id.to_string());
        write_provider_owned_journal(&self.pending_path, &journal)
    }

    fn mark_db_applied(&self) -> Result<(), ReplaceError> {
        mark_provider_owned_journal_path(&self.pending_path, "applied", None)
    }

    fn cleanup(&self) {
        fs::remove_file(&self.pending_path).ok();
    }
}

fn mark_provider_owned_journal_path(
    path: &Path,
    marker: &str,
    recovery_id: Option<&str>,
) -> Result<(), ReplaceError> {
    let mut journal = read_provider_owned_journal(path)?;
    journal.db_apply_marker = marker.to_string();
    if let Some(recovery_id) = recovery_id {
        journal.recovery_id = Some(recovery_id.to_string());
    }
    write_provider_owned_journal(path, &journal)
}

fn read_provider_owned_journal(path: &Path) -> Result<ProviderOwnedReplaceJournal, ReplaceError> {
    let bytes = fs::read(path).map_err(|e| ReplaceError::OperationalError {
        message: format!("failed to read provider-owned journal: {e}"),
    })?;
    serde_json::from_slice(&bytes).map_err(|e| ReplaceError::OperationalError {
        message: format!("failed to parse provider-owned journal: {e}"),
    })
}

fn write_provider_owned_journal(
    path: &Path,
    journal: &ProviderOwnedReplaceJournal,
) -> Result<(), ReplaceError> {
    let bytes = serde_json::to_vec_pretty(journal).map_err(|e| ReplaceError::OperationalError {
        message: format!("failed to serialize provider-owned journal: {e}"),
    })?;
    fs::write(path, bytes).map_err(|e| ReplaceError::OperationalError {
        message: format!("failed to write provider-owned journal: {e}"),
    })
}

fn cleanup_provider_owned_journal(path: &Path) -> Result<(), ReplaceError> {
    fs::remove_file(path).map_err(|e| ReplaceError::OperationalError {
        message: format!("failed to remove provider-owned journal: {e}"),
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

fn acquire_provider_owned_lease(
    lock: &SessionLock,
    session_id: &str,
    provider_name: &str,
) -> Result<Lease, ReplaceError> {
    lock.acquire(session_id, provider_name, Duration::from_secs(300))
        .map_err(map_provider_lock_error)
}

fn release_provider_owned_lease(lock: &SessionLock, lease: &Lease) -> Result<(), ReplaceError> {
    lock.release(&lease.session_id, &lease.token)
        .map(|_| ())
        .map_err(map_provider_lock_error)
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
        LockError::Operational { message } => ReplaceError::OperationalError { message },
    }
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
