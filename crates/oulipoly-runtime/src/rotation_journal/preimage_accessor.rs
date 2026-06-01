//! ## Declared roles
//! accessor

use crate::rotation_domain::ExternalRotationError;
use crate::services::MigrationServiceRequest;

pub(super) fn capture_rotation_preimage(
    request: &MigrationServiceRequest<'_>,
) -> Result<super::types::RotationJournalPreimage, ExternalRotationError> {
    let snapshot = crate::rotation_host_apply::load_chain_segment_snapshot(request)?;
    Ok(super::preimage_mapper::map_rotation_preimage(snapshot))
}
