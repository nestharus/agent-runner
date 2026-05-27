//! parser

use uuid::Uuid;

pub(super) fn parse_invocation_uuid(invocation_id: &str) -> Uuid {
    Uuid::parse_str(invocation_id).expect("generated invocation id must be a UUID")
}
