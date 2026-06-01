//! ## Declared roles
//! mapper

use super::types::{RotationJournalPhase, RotationJournalPreimage, RotationJournalRecord};
use crate::rotation_domain::ExternalRotationIdentity;
use oulipoly_provider::generated::RotationMaterializeResult;

pub(super) fn build_rotation_journal_record(
    phase: RotationJournalPhase,
    identity: &ExternalRotationIdentity,
    preimage: RotationJournalPreimage,
    result: &RotationMaterializeResult,
) -> RotationJournalRecord {
    RotationJournalRecord {
        schema_version: 1,
        phase,
        identity: identity.clone(),
        preimage,
        result: result.clone(),
    }
}
