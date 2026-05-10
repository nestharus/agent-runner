use super::{ApplyDecision, RowSnapshot};

pub fn decide_apply(source: &RowSnapshot, target: &RowSnapshot) -> ApplyDecision {
    if source.version.0 > target.version.0 {
        ApplyDecision::ApplySource
    } else if source.version.0 < target.version.0 || source.payload_hash == target.payload_hash {
        ApplyDecision::SkipSourceSameOrHigher
    } else {
        ApplyDecision::ConflictEqualVersionMismatch
    }
}
