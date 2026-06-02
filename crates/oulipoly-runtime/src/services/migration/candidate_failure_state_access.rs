//! ## Declared roles
//! accessor, mapper, orchestration

use crate::balancer::{FailureClass, apply_post_failure_forensics};
use crate::services::MigrationServiceRequest;

pub(super) fn record_candidate_failure_forensics(
    request: &MigrationServiceRequest<'_>,
    provider_name: &str,
) {
    let _ = apply_post_failure_forensics(
        request.state,
        provider_name,
        FailureClass::UpstreamApiDown,
        chrono::Utc::now(),
    );
}
