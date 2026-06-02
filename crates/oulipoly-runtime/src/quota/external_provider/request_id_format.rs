//! ## Declared roles
//!
//! `formatter`

pub(crate) fn quota_request_id(label: &str) -> String {
    format!("external-quota-{label}-{}", uuid::Uuid::new_v4())
}
