use rusqlite::types::Value;
use sha2::{Digest, Sha256};

/// Hashes payload columns using a deterministic column-order encoding.
///
/// Each column starts with a tag: `0x00` for an absent outer `None`, `0x05`
/// for SQLite NULL, `0x01` plus little-endian `i64` for integers, `0x02` plus
/// little-endian `f64` bits for reals, `0x03` plus a little-endian `u32` byte
/// length and UTF-8 bytes for text, and `0x04` plus a little-endian `u32` byte
/// length and bytes for blobs. A `0xFF` separator is written between columns.
pub fn payload_hash_for_columns(values: &[Option<Value>]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for (idx, value) in values.iter().enumerate() {
        if idx > 0 {
            hasher.update([0xFF]);
        }
        match value {
            None => hasher.update([0x00]),
            Some(Value::Integer(value)) => {
                hasher.update([0x01]);
                hasher.update(value.to_le_bytes());
            }
            Some(Value::Real(value)) => {
                hasher.update([0x02]);
                hasher.update(value.to_bits().to_le_bytes());
            }
            Some(Value::Text(value)) => {
                hasher.update([0x03]);
                update_len_prefixed(&mut hasher, value.as_bytes());
            }
            Some(Value::Blob(value)) => {
                hasher.update([0x04]);
                update_len_prefixed(&mut hasher, value);
            }
            Some(Value::Null) => hasher.update([0x05]),
        }
    }
    hasher.finalize().into()
}

fn update_len_prefixed(hasher: &mut Sha256, bytes: &[u8]) {
    let len = u32::try_from(bytes.len()).expect("row-version payload exceeds u32 length prefix");
    hasher.update(len.to_le_bytes());
    hasher.update(bytes);
}
