//! ## Declared roles
//! orchestration

use crate::services::MigrationServiceRequest;

pub(super) fn record_failed_candidate(
    request: &MigrationServiceRequest<'_>,
    candidate_index: usize,
    err: &crate::migration::MigrationError,
    candidates_tried: &mut Vec<String>,
) {
    let provider_name =
        super::provider_name_accessor::failed_candidate_provider_name(request, candidate_index);
    super::candidate_failure_state_access::record_candidate_failure_forensics(
        request,
        &provider_name,
    );
    candidates_tried.push(provider_name);
    let _ = err;
}
