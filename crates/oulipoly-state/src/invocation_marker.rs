//! ## Declared roles
//!
//! Role set: { parser, formatter, validator, predicate }
//!
//! `CompositeInvocationId` is the canonical Rust grammar owner for Oulipoly
//! invocation marker payloads. It formats canonical compact JSON and validates
//! accepted canonical or legacy payloads before callers use them as process
//! boundary identifiers. It also owns the small legacy-shape predicate needed
//! to choose canonical-vs-legacy parse errors without exporting grammar details.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-state/src/invocation_marker.rs
//!     role: adapter
//!     Translates:
//!       - serde_json compact-JSON serialization/deserialization contract for `CompositeInvocationId`
//!       - Oulipoly `OULIPOLY_INVOCATION=` marker-line and `OULIPOLY_PARENT_INVOCATION` parent-env payload contract (canonical-JSON grammar declared in this module)
//!       - Legacy shell-mangled marker payload compatibility grammar contract (declared accept-only-no-emit)
//! ```

use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

pub const INVOCATION_MARKER_PREFIX: &str = "OULIPOLY_INVOCATION=";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(deny_unknown_fields)]
pub struct CompositeInvocationId {
    pub source: String,
    pub id: String,
}

impl CompositeInvocationId {
    pub fn stderr_line(&self) -> String {
        format!(
            "{}{}",
            INVOCATION_MARKER_PREFIX,
            serde_json::to_string(self).expect("CompositeInvocationId serializes")
        )
    }

    pub fn parse_env_value(s: &str) -> Result<Self, ParseMarkerError> {
        Self::parse_marker_payload(s)
    }

    pub fn parse_marker_payload(s: &str) -> Result<Self, ParseMarkerError> {
        reject_empty_payload(s)?;
        parse_canonical_json(s).or_else(|json_err| {
            legacy_shell_mangled::parse(s).map_err(|legacy_err| {
                if legacy_err.is_legacy_shape_error() {
                    json_err
                } else {
                    legacy_err
                }
            })
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseMarkerError {
    InvalidJson,
    InvalidUuid,
    MissingSource,
    MissingId,
    UnknownKey(String),
    Empty,
    Malformed(String),
}

impl ParseMarkerError {
    fn is_legacy_shape_error(&self) -> bool {
        matches!(self, Self::Malformed(_) | Self::Empty)
    }
}

impl fmt::Display for ParseMarkerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJson => write!(f, "invalid JSON marker payload"),
            Self::InvalidUuid => write!(f, "invalid invocation UUID"),
            Self::MissingSource => write!(f, "missing source"),
            Self::MissingId => write!(f, "missing id"),
            Self::UnknownKey(key) => write!(f, "unknown key: {key}"),
            Self::Empty => write!(f, "empty marker payload"),
            Self::Malformed(message) => write!(f, "malformed marker payload: {message}"),
        }
    }
}

impl std::error::Error for ParseMarkerError {}

fn reject_empty_payload(s: &str) -> Result<(), ParseMarkerError> {
    if s.trim().is_empty() {
        Err(ParseMarkerError::Empty)
    } else {
        Ok(())
    }
}

fn parse_canonical_json(s: &str) -> Result<CompositeInvocationId, ParseMarkerError> {
    let parsed = decode_canonical_json(s)?;
    validate_canonical_fields(parsed)
}

fn decode_canonical_json(s: &str) -> Result<CompositeInvocationId, ParseMarkerError> {
    serde_json::from_str(s).map_err(|_| ParseMarkerError::InvalidJson)
}

fn validate_canonical_fields(
    parsed: CompositeInvocationId,
) -> Result<CompositeInvocationId, ParseMarkerError> {
    validate_required_text(&parsed.source, ParseMarkerError::MissingSource)?;
    validate_required_text(&parsed.id, ParseMarkerError::MissingId)?;
    validate_marker_uuid(&parsed.id)?;
    Ok(parsed)
}

fn validate_required_text(value: &str, err: ParseMarkerError) -> Result<(), ParseMarkerError> {
    if value.is_empty() { Err(err) } else { Ok(()) }
}

fn validate_marker_uuid(id: &str) -> Result<(), ParseMarkerError> {
    Uuid::parse_str(id)
        .map(|_| ())
        .map_err(|_| ParseMarkerError::InvalidUuid)
}

pub(crate) mod legacy_shell_mangled {
    use super::{CompositeInvocationId, ParseMarkerError, validate_canonical_fields};

    struct LegacyFields {
        source: Option<String>,
        id: Option<String>,
    }

    pub(crate) fn parse(s: &str) -> Result<CompositeInvocationId, ParseMarkerError> {
        let inner = legacy_inner(s)?;
        parse_legacy_fields(inner)
    }

    fn legacy_inner(s: &str) -> Result<&str, ParseMarkerError> {
        let trimmed = s.trim();
        trimmed
            .strip_prefix('{')
            .and_then(|inner| inner.strip_suffix('}'))
            .ok_or_else(|| ParseMarkerError::Malformed("expected braced payload".to_string()))
    }

    fn parse_legacy_fields(inner: &str) -> Result<CompositeInvocationId, ParseMarkerError> {
        let parsed = parse_legacy_fields_pure(inner)?;
        validate_legacy_fields(parsed)
    }

    fn parse_legacy_fields_pure(inner: &str) -> Result<LegacyFields, ParseMarkerError> {
        let mut source = None;
        let mut id = None;
        for part in inner.split(',') {
            let (key, value) = legacy_field(part)?;
            match key {
                "source" => source = Some(value),
                "id" => id = Some(value),
                _ => {}
            }
        }
        Ok(LegacyFields { source, id })
    }

    fn validate_legacy_fields(
        parsed: LegacyFields,
    ) -> Result<CompositeInvocationId, ParseMarkerError> {
        validate_canonical_fields(CompositeInvocationId {
            source: parsed.source.ok_or(ParseMarkerError::MissingSource)?,
            id: parsed.id.ok_or(ParseMarkerError::MissingId)?,
        })
    }

    fn legacy_field(part: &str) -> Result<(&str, String), ParseMarkerError> {
        let (key, value) = part
            .split_once(':')
            .ok_or_else(|| ParseMarkerError::Malformed("field lacks ':'".to_string()))?;
        Ok((key.trim(), legacy_value(value)))
    }

    fn legacy_value(value: &str) -> String {
        value
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .to_string()
    }
}
