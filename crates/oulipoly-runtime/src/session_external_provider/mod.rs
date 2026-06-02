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
use crate::session_replace::{ReplaceError, ReplaceReceipt, ReplaceSource};

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
    crate::session_replace::validate_import_replace_bytes_for_session(session_id, &bytes)?;
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
    let transaction = crate::session_replace::begin_external_provider_replace(
        session_id,
        &bytes,
        preimage_sha256,
    )?;
    let input_bytes = transaction.canonical_bytes().to_vec();
    let data_base64 = replace_input_formatter::data_base64(&input_bytes);
    let records_sha256 = replace_input_formatter::records_sha256(&input_bytes);
    let input = replace_input_mapper::map_prepared_replace_input(
        input_bytes,
        data_base64,
        records_sha256,
        transaction.turn_count(),
        transaction.preimage_sha256().to_string(),
    );
    let request_id = request_id_formatter::session_request_id("replace");
    let request =
        match request_builder::build_replace_request(&identity, session_id, &input, request_id) {
            Ok(request) => request,
            Err(error) => {
                let mapped = service_error_mapper::replace_adapter_error(error);
                return Err(
                    crate::session_replace::rollback_external_provider_replace_after_error(
                        transaction,
                        mapped,
                    ),
                );
            }
        };
    let result = match client_invoker::invoke_replace(&client, request) {
        Ok(result) => result,
        Err(error) => {
            let mapped = service_error_mapper::replace_client_error(error);
            return Err(
                crate::session_replace::rollback_external_provider_replace_after_error(
                    transaction,
                    mapped,
                ),
            );
        }
    };
    if !result.changed {
        crate::session_replace::rollback_external_provider_replace(transaction)?;
        return Ok(replace_no_change_formatter::no_change_receipt(
            identity, session_id, input,
        ));
    }
    let accepted = match replace_result_mapper::validate_changed_replace_result(
        &identity, session_id, &input, &result,
    ) {
        Ok(accepted) => accepted,
        Err(error) => {
            let mapped = service_error_mapper::replace_adapter_error(error);
            return Err(
                crate::session_replace::rollback_external_provider_replace_after_error(
                    transaction,
                    mapped,
                ),
            );
        }
    };
    if let Err(error) =
        replace_host_apply::verify_provider_transformed_replace(session_id, &input, &accepted)
    {
        return Err(
            crate::session_replace::rollback_external_provider_replace_after_error(
                transaction,
                error,
            ),
        );
    }
    replace_host_apply::apply_verified_provider_transformed_replace(transaction, accepted)
}
