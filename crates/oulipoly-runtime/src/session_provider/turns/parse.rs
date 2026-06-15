use super::super::types::{SessionProviderError, SessionProviderTurn};
use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

pub(super) fn provider_turns_from_values(
    values: Vec<Value>,
) -> Result<Vec<SessionProviderTurn>, SessionProviderError> {
    values.into_iter().map(provider_turn_from_value).collect()
}

fn provider_turn_from_value(value: Value) -> Result<SessionProviderTurn, SessionProviderError> {
    let fields = validate_provider_turn_fields(parse_provider_turn_fields(value)?)?;
    Ok(session_provider_turn_from_fields(fields))
}

struct ParsedProviderTurnFields {
    session_id: String,
    turn_id: String,
    role: String,
    timestamp: String,
    parent_turn_id: Option<String>,
    is_sidechain: Option<bool>,
    is_compaction_boundary: Option<bool>,
    body: Option<Value>,
}

fn parse_provider_turn_fields(
    value: Value,
) -> Result<ParsedProviderTurnFields, SessionProviderError> {
    let object = provider_turn_object(&value)?;
    Ok(ParsedProviderTurnFields {
        session_id: required_string(object, "session_id", "provider_turn_missing_session_id")?,
        turn_id: required_string(object, "turn_id", "provider_turn_missing_turn_id")?,
        role: required_string(object, "role", "provider_turn_missing_role")?,
        timestamp: required_string(object, "timestamp", "provider_turn_missing_timestamp")?,
        parent_turn_id: optional_string(object, "parent_turn_id")?,
        is_sidechain: optional_bool(object, "is_sidechain")?,
        is_compaction_boundary: optional_bool(object, "is_compaction_boundary")?,
        body: optional_body(object)?,
    })
}

fn validate_provider_turn_fields(
    fields: ParsedProviderTurnFields,
) -> Result<ProviderTurnFields, SessionProviderError> {
    Ok(provider_turn_fields_from_validated_parts(
        fields.session_id,
        fields.turn_id,
        parse_turn_timestamp(fields.timestamp)?,
        fields.role,
        fields.parent_turn_id,
        fields.is_sidechain.unwrap_or(false),
        fields.is_compaction_boundary.unwrap_or(false),
        fields.body,
    ))
}

#[allow(clippy::too_many_arguments)]
fn provider_turn_fields_from_validated_parts(
    session_id: String,
    turn_id: String,
    timestamp: DateTime<Utc>,
    role: String,
    parent_turn_id: Option<String>,
    is_sidechain: bool,
    is_compaction_boundary: bool,
    body: Option<Value>,
) -> ProviderTurnFields {
    ProviderTurnFields {
        session_id,
        turn_id,
        timestamp,
        role,
        parent_turn_id,
        is_sidechain,
        is_compaction_boundary,
        body,
    }
}

fn provider_turn_object(value: &Value) -> Result<&Map<String, Value>, SessionProviderError> {
    value.as_object().ok_or_else(|| {
        SessionProviderError::new(
            "provider_turn_invalid_type",
            "provider turn was not an object",
        )
    })
}

struct ProviderTurnFields {
    session_id: String,
    turn_id: String,
    timestamp: DateTime<Utc>,
    role: String,
    parent_turn_id: Option<String>,
    is_sidechain: bool,
    is_compaction_boundary: bool,
    body: Option<Value>,
}

fn session_provider_turn_from_fields(fields: ProviderTurnFields) -> SessionProviderTurn {
    SessionProviderTurn {
        session_id: fields.session_id,
        turn_id: fields.turn_id,
        timestamp: fields.timestamp,
        role: fields.role,
        parent_turn_id: fields.parent_turn_id,
        is_sidechain: fields.is_sidechain,
        is_compaction_boundary: fields.is_compaction_boundary,
        body: fields.body,
    }
}

fn required_string(
    object: &Map<String, Value>,
    key: &str,
    missing_token: &str,
) -> Result<String, SessionProviderError> {
    let Some(value) = object.get(key) else {
        return Err(SessionProviderError::new(
            missing_token,
            format!("provider turn missing {key}"),
        ));
    };
    value.as_str().map(str::to_string).ok_or_else(|| {
        SessionProviderError::new(
            "provider_turn_invalid_type",
            format!("provider turn field {key} was not a string"),
        )
    })
}

fn optional_string(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<String>, SessionProviderError> {
    object
        .get(key)
        .map(|value| {
            value.as_str().map(str::to_string).ok_or_else(|| {
                SessionProviderError::new(
                    "provider_turn_invalid_type",
                    format!("provider turn field {key} was not a string"),
                )
            })
        })
        .transpose()
}

fn optional_bool(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<bool>, SessionProviderError> {
    object
        .get(key)
        .map(|value| {
            value.as_bool().ok_or_else(|| {
                SessionProviderError::new(
                    "provider_turn_invalid_type",
                    format!("provider turn field {key} was not a boolean"),
                )
            })
        })
        .transpose()
}

fn optional_body(object: &Map<String, Value>) -> Result<Option<Value>, SessionProviderError> {
    optional_body_value(object)
        .map(validate_provider_turn_body)
        .transpose()
        .map(map_optional_body_value)
}

fn optional_body_value(object: &Map<String, Value>) -> Option<&Value> {
    object.get("body")
}

fn validate_provider_turn_body(body: &Value) -> Result<&Value, SessionProviderError> {
    if crate::sessions::is_canonical_body_shape(body) {
        Ok(body)
    } else {
        Err(provider_turn_noncanonical_body())
    }
}

fn provider_turn_noncanonical_body() -> SessionProviderError {
    SessionProviderError::new(
        "provider_turn_noncanonical_body",
        "provider turn body was not a canonical content chunk array",
    )
}

fn map_optional_body_value(body: Option<&Value>) -> Option<Value> {
    body.cloned()
}

fn parse_turn_timestamp(input: String) -> Result<DateTime<Utc>, SessionProviderError> {
    DateTime::parse_from_rfc3339(&input)
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .map_err(|err| {
            SessionProviderError::new(
                "provider_turn_invalid_timestamp",
                format!("invalid provider turn timestamp {input}: {err}"),
            )
        })
}
