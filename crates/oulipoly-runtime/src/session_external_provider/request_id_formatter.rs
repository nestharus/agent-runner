//! Role: formatter.

pub(crate) fn session_request_id(label: &str) -> String {
    format!("session-{label}-{}", uuid::Uuid::new_v4())
}
