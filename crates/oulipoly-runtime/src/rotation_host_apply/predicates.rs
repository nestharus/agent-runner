//! ## Declared roles
//! predicate

use serde_json::Value;

pub(super) fn find_plan_segment<'a>(
    segments: &'a [Value],
    provider: &str,
    session_id: &str,
) -> Option<&'a serde_json::Map<String, Value>> {
    segments
        .iter()
        .filter_map(Value::as_object)
        .find(|segment| {
            segment.get("provider").and_then(Value::as_str) == Some(provider)
                && segment.get("session_id").and_then(Value::as_str) == Some(session_id)
        })
}

pub(super) fn timestamps_equal(left: &str, right: &str) -> bool {
    match (
        chrono::DateTime::parse_from_rfc3339(left),
        chrono::DateTime::parse_from_rfc3339(right),
    ) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}
