//! ## Declared roles
//! accessor

use crate::services::MigrationServiceRequest;

pub(super) fn failed_candidate_provider_name(
    request: &MigrationServiceRequest<'_>,
    candidate_index: usize,
) -> String {
    request.migration_model.providers[candidate_index]
        .name
        .clone()
}
