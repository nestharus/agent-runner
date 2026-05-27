//! filter

pub(super) fn session_ingest_fallback_session_id(
    emitted: bool,
    session_id: Option<&str>,
) -> Option<&str> {
    (!emitted).then_some(session_id).flatten()
}
