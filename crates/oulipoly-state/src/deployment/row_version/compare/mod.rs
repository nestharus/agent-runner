mod decide;
mod predicate;

pub use decide::decide_apply;
pub use predicate::same_or_higher;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowVersion(pub i64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowSnapshot {
    pub version: RowVersion,
    pub payload_hash: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyDecision {
    ApplySource,
    SkipSourceSameOrHigher,
    ConflictEqualVersionMismatch,
}
