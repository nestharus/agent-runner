//! ## Declared roles
//! mapper

use sha2::{Digest, Sha256};

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
