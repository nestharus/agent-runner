//! Role: mapper.

#[derive(Debug, Clone)]
pub(crate) struct PreparedReplaceInput {
    pub(crate) bytes: Vec<u8>,
    pub(crate) data_base64: String,
    pub(crate) records_sha256: String,
    pub(crate) actual_preimage_sha256: String,
    pub(crate) turn_count: u64,
}

pub(crate) fn map_prepared_replace_input(
    bytes: Vec<u8>,
    data_base64: String,
    records_sha256: String,
    turn_count: u64,
    actual_preimage_sha256: String,
) -> PreparedReplaceInput {
    PreparedReplaceInput {
        data_base64,
        records_sha256,
        bytes,
        actual_preimage_sha256,
        turn_count,
    }
}
