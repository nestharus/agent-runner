//! Declared roles: accessor

pub(crate) fn utc_now() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now()
}
