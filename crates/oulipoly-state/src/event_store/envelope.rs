use super::EVENT_SCHEMA_VERSION;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt;
use uuid::Uuid;

const MAX_KIND_BYTES: usize = 64;
/// Maximum normalized JSON payload accepted by version 1.
pub const MAX_EVENT_PAYLOAD_BYTES: usize = 1024 * 1024;
const DEFAULT_NORMALIZATION_DEPTH: usize = 16;
const DEFAULT_NORMALIZATION_FIELDS: usize = 256;
const DEFAULT_NORMALIZATION_STRING_BYTES: usize = 1024;
const MAX_LEGACY_SYNTHETIC_FIELDS: usize = 16;
const MAX_LEGACY_UNAVAILABLE_FIELDS: usize = 32;
const MAX_PROVENANCE_LABEL_BYTES: usize = 64;

macro_rules! id16 {
    ($name:ident) => {
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name([u8; 16]);

        impl $name {
            pub const fn from_bytes(bytes: [u8; 16]) -> Self {
                Self(bytes)
            }

            pub fn random() -> Self {
                Self(*Uuid::new_v4().as_bytes())
            }

            pub const fn as_bytes(&self) -> &[u8; 16] {
                &self.0
            }

            pub fn is_nil(&self) -> bool {
                self.0 == [0; 16]
            }

            pub fn from_slice(bytes: &[u8]) -> Result<Self, EnvelopeError> {
                let bytes: [u8; 16] = bytes.try_into().map_err(|_| {
                    EnvelopeError::InvalidField(concat!(stringify!($name), " must be 16 bytes"))
                })?;
                Ok(Self(bytes))
            }
        }

        impl From<Uuid> for $name {
            fn from(value: Uuid) -> Self {
                Self(*value.as_bytes())
            }
        }

        impl From<$name> for Uuid {
            fn from(value: $name) -> Self {
                Uuid::from_bytes(value.0)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                Uuid::from_bytes(self.0).fmt(formatter)
            }
        }
    };
}

id16!(EventId);
id16!(GenerationId);
id16!(WriterInstanceId);
id16!(ProcessInstanceId);
id16!(CorrelationId);
id16!(SupervisorAuthorityId);
id16!(TraceId);
id16!(SpanId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Digest32([u8; 32]);

impl Digest32 {
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn from_slice(bytes: &[u8]) -> Result<Self, EnvelopeError> {
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| EnvelopeError::InvalidField("digest must be 32 bytes"))?;
        Ok(Self(bytes))
    }

    pub fn sha256(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionCorrelationDigest(Digest32);

impl SessionCorrelationDigest {
    pub const fn from_digest(digest: Digest32) -> Self {
        Self(digest)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        self.0.as_bytes()
    }

    pub fn from_slice(bytes: &[u8]) -> Result<Self, EnvelopeError> {
        Ok(Self(Digest32::from_slice(bytes)?))
    }
}

/// Hash a raw provider session identifier before it crosses the telemetry
/// boundary. The raw identifier must never be stored as a label or payload.
pub fn session_correlation_digest(raw_session_id: &str) -> SessionCorrelationDigest {
    let mut digest = Sha256::new();
    digest.update(b"oulipoly.session-correlation.v1\0");
    digest.update(raw_session_id.as_bytes());
    SessionCorrelationDigest(Digest32(digest.finalize().into()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(i64)]
pub enum EventFamily {
    Diagnostic = 1,
    Trace = 2,
    Metric = 3,
    Log = 4,
    Maintenance = 5,
}

impl EventFamily {
    pub(crate) fn from_i64(value: i64) -> Result<Self, EnvelopeError> {
        match value {
            1 => Ok(Self::Diagnostic),
            2 => Ok(Self::Trace),
            3 => Ok(Self::Metric),
            4 => Ok(Self::Log),
            5 => Ok(Self::Maintenance),
            _ => Err(EnvelopeError::InvalidField("unknown event family")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(i64)]
pub enum PayloadCodec {
    JsonUtf8V1 = 1,
}

impl PayloadCodec {
    pub(crate) fn from_i64(value: i64) -> Result<Self, EnvelopeError> {
        match value {
            1 => Ok(Self::JsonUtf8V1),
            _ => Err(EnvelopeError::InvalidField("unknown payload codec")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EventKind(String);

impl EventKind {
    /// Register a repository-owned label. Requiring a static string keeps raw
    /// provider/user text out of this indexed field.
    pub fn registered(value: &'static str) -> Result<Self, EnvelopeError> {
        Self::validate(value)?;
        Ok(Self(value.to_ascii_lowercase()))
    }

    pub(crate) fn from_stored(value: String) -> Result<Self, EnvelopeError> {
        Self::validate(&value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn validate(value: &str) -> Result<(), EnvelopeError> {
        if value.is_empty() || value.len() > MAX_KIND_BYTES {
            return Err(EnvelopeError::InvalidField(
                "kind must contain between 1 and 64 UTF-8 bytes",
            ));
        }
        if !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        }) {
            return Err(EnvelopeError::InvalidField(
                "kind must be a lowercase registered label",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeProcessIdentity {
    pub os_pid: i64,
    pub os_boot_id_sha256: Digest32,
    pub os_pid_starttime_ticks: i64,
}

impl NativeProcessIdentity {
    pub fn validate(&self) -> Result<(), EnvelopeError> {
        if self.os_pid <= 0 || self.os_pid_starttime_ticks <= 0 {
            return Err(EnvelopeError::InvalidField(
                "native PID identity values are invalid",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProducerIdentity {
    pub writer_instance_id: WriterInstanceId,
    pub process_instance_id: ProcessInstanceId,
    pub process_root_id: ProcessInstanceId,
    pub parent_process_instance_id: Option<ProcessInstanceId>,
    pub supervisor_authority_id: Option<SupervisorAuthorityId>,
    /// Present for native live writers and absent for deterministic legacy
    /// import partitions. Exact PID identity is metadata/evidence, not a lock.
    pub native_process: Option<NativeProcessIdentity>,
}

impl ProducerIdentity {
    pub fn validate(&self) -> Result<(), EnvelopeError> {
        if self.writer_instance_id.is_nil()
            || self.process_instance_id.is_nil()
            || self.process_root_id.is_nil()
            || self
                .parent_process_instance_id
                .is_some_and(|identity| identity.is_nil() || identity == self.process_instance_id)
            || self
                .supervisor_authority_id
                .is_some_and(|identity| identity.is_nil())
        {
            return Err(EnvelopeError::InvalidField(
                "producer identities must be non-nil and the parent must be distinct",
            ));
        }
        if let Some(native) = &self.native_process {
            native.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventCorrelations {
    pub trace_id: Option<TraceId>,
    pub span_id: Option<SpanId>,
    pub parent_span_id: Option<SpanId>,
    pub invocation_uuid: Option<CorrelationId>,
    pub session_correlation_sha256: Option<SessionCorrelationDigest>,
}

impl EventCorrelations {
    pub fn validate(&self) -> Result<(), EnvelopeError> {
        if self.trace_id.is_some_and(|identity| identity.is_nil())
            || self.span_id.is_some_and(|identity| identity.is_nil())
            || self
                .parent_span_id
                .is_some_and(|identity| identity.is_nil())
            || self
                .invocation_uuid
                .is_some_and(|identity| identity.is_nil())
        {
            return Err(EnvelopeError::InvalidField(
                "event correlations must not contain nil identities",
            ));
        }
        if self.parent_span_id.is_some() && (self.trace_id.is_none() || self.span_id.is_none()) {
            return Err(EnvelopeError::InvalidField(
                "a parent span requires trace and span identifiers",
            ));
        }
        if self.span_id.is_some() != self.trace_id.is_some() {
            return Err(EnvelopeError::InvalidField(
                "trace and span identifiers must be supplied together",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacySyntheticField {
    EventId,
    WriterInstance,
    ProcessInstance,
    SelfRoot,
    ProducerSequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyProvenance {
    pub legacy_source_id: Digest32,
    pub complete_line_byte_offset: u64,
    pub complete_record_ordinal: u64,
    pub line_sha256: Digest32,
    pub original_schema: String,
    pub synthetic_fields: Vec<LegacySyntheticField>,
    pub unavailable_fields: Vec<String>,
}

impl LegacyProvenance {
    fn validate(&self) -> Result<(), EnvelopeError> {
        if self.original_schema.is_empty()
            || self.original_schema.len() > MAX_PROVENANCE_LABEL_BYTES
        {
            return Err(EnvelopeError::InvalidField(
                "legacy original schema must be 1..=64 bytes",
            ));
        }
        if self.synthetic_fields.len() > MAX_LEGACY_SYNTHETIC_FIELDS
            || self.unavailable_fields.len() > MAX_LEGACY_UNAVAILABLE_FIELDS
        {
            return Err(EnvelopeError::InvalidField(
                "legacy provenance field bounds exceeded",
            ));
        }
        if self.unavailable_fields.iter().any(|field| {
            field.is_empty()
                || field.len() > MAX_PROVENANCE_LABEL_BYTES
                || !field
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        }) {
            return Err(EnvelopeError::InvalidField(
                "legacy unavailable-field label is invalid",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct PayloadNormalizationPolicy {
    allowed_root_fields: BTreeSet<String>,
    max_depth: usize,
    max_fields: usize,
    max_string_bytes: usize,
}

impl PayloadNormalizationPolicy {
    pub fn registered(allowed_root_fields: &[&'static str]) -> Result<Self, EnvelopeError> {
        let mut fields = BTreeSet::new();
        for field in allowed_root_fields {
            validate_payload_key(field)?;
            fields.insert((*field).to_string());
        }
        Ok(Self {
            allowed_root_fields: fields,
            max_depth: DEFAULT_NORMALIZATION_DEPTH,
            max_fields: DEFAULT_NORMALIZATION_FIELDS,
            max_string_bytes: DEFAULT_NORMALIZATION_STRING_BYTES,
        })
    }
}

#[derive(Debug, Clone)]
pub struct NewEventV1 {
    pub event_id: EventId,
    pub family: EventFamily,
    pub kind: EventKind,
    pub recorded_at_unix_micros: i64,
    pub producer_sequence: i64,
    pub producer: ProducerIdentity,
    pub correlations: EventCorrelations,
    pub payload: Value,
    pub legacy_provenance: Option<LegacyProvenance>,
    pub retry_of_generation_id: Option<GenerationId>,
}

/// Immutable version-1 logical event. It is fully normalized and identified
/// before any sink is selected, so SQLite, JSONL, import and retry all observe
/// byte-equivalent fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventEnvelopeV1 {
    pub event_id: EventId,
    pub schema_version: i64,
    pub family: EventFamily,
    pub kind: EventKind,
    pub recorded_at_unix_micros: i64,
    pub ingested_at_unix_micros: i64,
    pub producer_sequence: i64,
    pub producer: ProducerIdentity,
    pub correlations: EventCorrelations,
    pub payload_codec: PayloadCodec,
    /// Canonical compact UTF-8 JSON after normalization/redaction.
    pub payload: String,
    pub payload_sha256: Digest32,
    pub payload_bytes: i64,
    pub legacy_provenance: Option<LegacyProvenance>,
    pub retry_of_generation_id: Option<GenerationId>,
}

impl EventEnvelopeV1 {
    pub fn normalize(
        input: NewEventV1,
        policy: &PayloadNormalizationPolicy,
        ingested_at_unix_micros: i64,
    ) -> Result<Self, EnvelopeError> {
        if input.recorded_at_unix_micros < 0
            || ingested_at_unix_micros < 0
            || input.producer_sequence < 0
        {
            return Err(EnvelopeError::InvalidField(
                "timestamps and producer sequence must be non-negative",
            ));
        }
        input.correlations.validate()?;
        input.producer.validate()?;
        if let Some(provenance) = &input.legacy_provenance {
            provenance.validate()?;
        }
        if input.legacy_provenance.is_none() && input.producer.native_process.is_none() {
            return Err(EnvelopeError::InvalidField(
                "native event requires exact native process identity",
            ));
        }
        let payload = normalize_payload_v1(input.payload, policy)?;
        let payload = serde_json::to_string(&payload)
            .map_err(|error| EnvelopeError::Json(error.to_string()))?;
        if payload.len() > MAX_EVENT_PAYLOAD_BYTES {
            return Err(EnvelopeError::PayloadTooLarge {
                actual: payload.len(),
                maximum: MAX_EVENT_PAYLOAD_BYTES,
            });
        }
        let event = Self {
            event_id: input.event_id,
            schema_version: EVENT_SCHEMA_VERSION,
            family: input.family,
            kind: input.kind,
            recorded_at_unix_micros: input.recorded_at_unix_micros,
            ingested_at_unix_micros,
            producer_sequence: input.producer_sequence,
            producer: input.producer,
            correlations: input.correlations,
            payload_codec: PayloadCodec::JsonUtf8V1,
            payload_sha256: Digest32::sha256(payload.as_bytes()),
            payload_bytes: payload.len() as i64,
            payload,
            legacy_provenance: input.legacy_provenance,
            retry_of_generation_id: input.retry_of_generation_id,
        };
        event.validate()?;
        Ok(event)
    }

    pub fn validate(&self) -> Result<(), EnvelopeError> {
        if self.schema_version != EVENT_SCHEMA_VERSION {
            return Err(EnvelopeError::UnknownSchemaVersion(self.schema_version));
        }
        if self.event_id.is_nil() {
            return Err(EnvelopeError::InvalidField("event ID must be non-nil"));
        }
        EventKind::validate(self.kind.as_str())?;
        if self.recorded_at_unix_micros < 0
            || self.ingested_at_unix_micros < 0
            || self.producer_sequence < 0
        {
            return Err(EnvelopeError::InvalidField(
                "timestamps and producer sequence must be non-negative",
            ));
        }
        self.correlations.validate()?;
        self.producer.validate()?;
        if let Some(provenance) = &self.legacy_provenance {
            provenance.validate()?;
        }
        if self.legacy_provenance.is_none() && self.producer.native_process.is_none() {
            return Err(EnvelopeError::InvalidField(
                "native event requires exact native process identity",
            ));
        }
        let payload_bytes = usize::try_from(self.payload_bytes)
            .map_err(|_| EnvelopeError::InvalidField("payload byte count is negative"))?;
        if payload_bytes != self.payload.len() {
            return Err(EnvelopeError::InvalidField(
                "payload byte count does not match payload",
            ));
        }
        if payload_bytes > MAX_EVENT_PAYLOAD_BYTES {
            return Err(EnvelopeError::PayloadTooLarge {
                actual: payload_bytes,
                maximum: MAX_EVENT_PAYLOAD_BYTES,
            });
        }
        if Digest32::sha256(self.payload.as_bytes()) != self.payload_sha256 {
            return Err(EnvelopeError::InvalidField(
                "payload digest does not match payload",
            ));
        }
        let parsed: Value = serde_json::from_str(&self.payload)
            .map_err(|error| EnvelopeError::Json(error.to_string()))?;
        let allowed_root_fields = parsed
            .as_object()
            .ok_or(EnvelopeError::InvalidField(
                "event payload must be a JSON object",
            ))?
            .keys()
            .cloned()
            .collect();
        let policy = PayloadNormalizationPolicy {
            allowed_root_fields,
            max_depth: DEFAULT_NORMALIZATION_DEPTH,
            max_fields: DEFAULT_NORMALIZATION_FIELDS,
            max_string_bytes: DEFAULT_NORMALIZATION_STRING_BYTES,
        };
        let normalized = normalize_payload_v1(parsed.clone(), &policy)?;
        let canonical = serde_json::to_string(&normalized)
            .map_err(|error| EnvelopeError::Json(error.to_string()))?;
        if normalized != parsed || canonical != self.payload {
            return Err(EnvelopeError::InvalidField(
                "payload is not canonical normalized/redacted JSON",
            ));
        }
        Ok(())
    }

    /// Digest of immutable *logical* event fields. Writer instance,
    /// writer-wall ingestion time, physical generation/retry linkage, and
    /// legacy import provenance are deliberately absent. Producer sequence is
    /// assigned pre-fanout and remains part of logical identity. A fallback
    /// envelope imported into a deterministic source partition must remain the
    /// same logical event as its native pre-fanout copy.
    pub fn immutable_digest(&self) -> Result<Digest32, EnvelopeError> {
        #[derive(Serialize)]
        struct LogicalProducerIdentity<'a> {
            process_instance_id: ProcessInstanceId,
            process_root_id: ProcessInstanceId,
            parent_process_instance_id: Option<ProcessInstanceId>,
            supervisor_authority_id: Option<SupervisorAuthorityId>,
            native_process: &'a Option<NativeProcessIdentity>,
        }
        #[derive(Serialize)]
        struct LogicalIdentity<'a> {
            event_id: EventId,
            schema_version: i64,
            family: EventFamily,
            kind: &'a EventKind,
            recorded_at_unix_micros: i64,
            producer_sequence: i64,
            producer: LogicalProducerIdentity<'a>,
            correlations: &'a EventCorrelations,
            payload_codec: PayloadCodec,
            payload: &'a str,
            payload_sha256: Digest32,
            payload_bytes: i64,
        }
        let logical = LogicalIdentity {
            event_id: self.event_id,
            schema_version: self.schema_version,
            family: self.family,
            kind: &self.kind,
            recorded_at_unix_micros: self.recorded_at_unix_micros,
            producer_sequence: self.producer_sequence,
            producer: LogicalProducerIdentity {
                process_instance_id: self.producer.process_instance_id,
                process_root_id: self.producer.process_root_id,
                parent_process_instance_id: self.producer.parent_process_instance_id,
                supervisor_authority_id: self.producer.supervisor_authority_id,
                native_process: &self.producer.native_process,
            },
            correlations: &self.correlations,
            payload_codec: self.payload_codec,
            payload: &self.payload,
            payload_sha256: self.payload_sha256,
            payload_bytes: self.payload_bytes,
        };
        let bytes =
            serde_json::to_vec(&logical).map_err(|error| EnvelopeError::Json(error.to_string()))?;
        let mut digest = Sha256::new();
        digest.update(b"oulipoly.event-envelope.v1\0");
        digest.update(bytes);
        Ok(Digest32(digest.finalize().into()))
    }
}

pub fn normalize_payload_v1(
    payload: Value,
    policy: &PayloadNormalizationPolicy,
) -> Result<Value, EnvelopeError> {
    if !payload.is_object() {
        return Err(EnvelopeError::InvalidField(
            "event payload must be a JSON object",
        ));
    }
    let mut field_count = 0usize;
    normalize_value(payload, policy, 0, true, &mut field_count)
}

fn normalize_value(
    value: Value,
    policy: &PayloadNormalizationPolicy,
    depth: usize,
    root: bool,
    field_count: &mut usize,
) -> Result<Value, EnvelopeError> {
    if depth > policy.max_depth {
        return Err(EnvelopeError::NormalizationLimit("payload depth"));
    }
    match value {
        Value::Object(values) => {
            let mut normalized = Map::new();
            let mut sorted = values.into_iter().collect::<Vec<_>>();
            sorted.sort_by(|left, right| left.0.cmp(&right.0));
            for (key, value) in sorted {
                validate_payload_key(&key)?;
                if root && !policy.allowed_root_fields.contains(&key) {
                    return Err(EnvelopeError::UnknownPayloadField(key));
                }
                *field_count = field_count.saturating_add(1);
                if *field_count > policy.max_fields {
                    return Err(EnvelopeError::NormalizationLimit("payload field count"));
                }
                let normalized_value = if sensitive_key(&key) {
                    Value::String("[REDACTED]".to_string())
                } else if path_key(&key) {
                    Value::String("[PATH_REDACTED]".to_string())
                } else {
                    normalize_value(value, policy, depth + 1, false, field_count)?
                };
                normalized.insert(key, normalized_value);
            }
            Ok(Value::Object(normalized))
        }
        Value::Array(values) => {
            if values.len() > policy.max_fields {
                return Err(EnvelopeError::NormalizationLimit("payload array length"));
            }
            values
                .into_iter()
                .map(|value| normalize_value(value, policy, depth + 1, false, field_count))
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array)
        }
        Value::String(value) => Ok(Value::String(redact_text(&value, policy.max_string_bytes))),
        scalar => Ok(scalar),
    }
}

fn validate_payload_key(key: &str) -> Result<(), EnvelopeError> {
    if key.is_empty()
        || key.len() > MAX_PROVENANCE_LABEL_BYTES
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(EnvelopeError::InvalidField(
            "payload key is not a bounded label",
        ));
    }
    Ok(())
}

fn sensitive_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    [
        "token",
        "secret",
        "password",
        "credential",
        "authorization",
        "api_key",
        "apikey",
        "cursor",
        "prompt",
        "transcript",
        "environment",
        "claim",
        "sql",
        "bind",
        "session_id",
        "sessionid",
    ]
    .iter()
    .any(|sensitive| key.contains(sensitive))
}

fn path_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key == "path" || key.ends_with("_path") || key.ends_with("_paths")
}

fn redact_text(value: &str, max_bytes: usize) -> String {
    let flattened = value.replace(['\r', '\n'], " ");
    let mut redact_words = 0usize;
    let mut parts = Vec::new();
    for part in flattened.split_whitespace() {
        if redact_words > 0 {
            parts.push("[REDACTED]".to_string());
            redact_words -= 1;
            continue;
        }
        let lower = part.to_ascii_lowercase();
        if looks_like_path(part) {
            parts.push("[PATH_REDACTED]".to_string());
        } else if sensitive_key(&lower) {
            if let Some(delimiter) = part.find(['=', ':']) {
                parts.push(format!("{}=[REDACTED]", &part[..delimiter]));
                let supplied = &lower[delimiter + 1..];
                redact_words = usize::from(
                    delimiter + 1 == part.len() || supplied == "bearer" || supplied == "basic",
                );
            } else {
                parts.push(part.to_string());
                redact_words = if lower.contains("authorization") {
                    2
                } else {
                    1
                };
            }
        } else {
            parts.push(part.to_string());
        }
    }
    truncate_utf8(&parts.join(" "), max_bytes)
}

fn looks_like_path(value: &str) -> bool {
    value.contains('/')
        || value.contains('\\')
        || value.starts_with("~/")
        || (value.len() >= 3
            && value.as_bytes()[0].is_ascii_alphabetic()
            && value.as_bytes()[1] == b':'
            && matches!(value.as_bytes()[2], b'/' | b'\\'))
}

fn truncate_utf8(value: &str, maximum: usize) -> String {
    if value.len() <= maximum {
        return value.to_string();
    }
    let mut end = maximum;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvelopeError {
    InvalidField(&'static str),
    UnknownPayloadField(String),
    NormalizationLimit(&'static str),
    PayloadTooLarge { actual: usize, maximum: usize },
    UnknownSchemaVersion(i64),
    Json(String),
}

impl fmt::Display for EnvelopeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidField(reason) => write!(formatter, "invalid event envelope: {reason}"),
            Self::UnknownPayloadField(field) => {
                write!(formatter, "unregistered event payload field: {field}")
            }
            Self::NormalizationLimit(limit) => {
                write!(
                    formatter,
                    "event payload exceeded normalization {limit} limit"
                )
            }
            Self::PayloadTooLarge { actual, maximum } => write!(
                formatter,
                "event payload is {actual} bytes; maximum is {maximum} bytes"
            ),
            Self::UnknownSchemaVersion(version) => {
                write!(formatter, "unknown event schema version {version}")
            }
            Self::Json(error) => write!(formatter, "event JSON error: {error}"),
        }
    }
}

impl std::error::Error for EnvelopeError {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn identity() -> ProducerIdentity {
        ProducerIdentity {
            writer_instance_id: WriterInstanceId::from_bytes([1; 16]),
            process_instance_id: ProcessInstanceId::from_bytes([2; 16]),
            process_root_id: ProcessInstanceId::from_bytes([3; 16]),
            parent_process_instance_id: Some(ProcessInstanceId::from_bytes([4; 16])),
            supervisor_authority_id: Some(SupervisorAuthorityId::from_bytes([5; 16])),
            native_process: Some(NativeProcessIdentity {
                os_pid: 42,
                os_boot_id_sha256: Digest32::from_bytes([6; 32]),
                os_pid_starttime_ticks: 7,
            }),
        }
    }

    #[test]
    fn normalization_is_stable_and_redacts_sensitive_values_and_paths() {
        let policy = PayloadNormalizationPolicy::registered(&[
            "operation",
            "auth_token",
            "artifact_path",
            "cause",
        ])
        .unwrap();
        let input = NewEventV1 {
            event_id: EventId::from_bytes([9; 16]),
            family: EventFamily::Diagnostic,
            kind: EventKind::registered("sqlite.failed").unwrap(),
            recorded_at_unix_micros: 10,
            producer_sequence: 12,
            producer: identity(),
            correlations: EventCorrelations::default(),
            payload: json!({
                "operation": "open",
                "auth_token": "should-not-survive",
                "artifact_path": "/private/result.txt",
                "cause": "token=also-secret at /home/person/data"
            }),
            legacy_provenance: None,
            retry_of_generation_id: None,
        };
        let envelope = EventEnvelopeV1::normalize(input, &policy, 11).unwrap();
        assert_eq!(
            envelope.payload,
            r#"{"artifact_path":"[PATH_REDACTED]","auth_token":"[REDACTED]","cause":"token=[REDACTED] at [PATH_REDACTED]","operation":"open"}"#
        );
        assert!(!envelope.payload.contains("secret"));
        assert!(!envelope.payload.contains("/home"));
        envelope.validate().unwrap();
    }

    #[test]
    fn event_identity_is_preassigned_and_full_envelope_digest_is_stable() {
        let policy = PayloadNormalizationPolicy::registered(&["value"]).unwrap();
        let make = || {
            EventEnvelopeV1::normalize(
                NewEventV1 {
                    event_id: EventId::from_bytes([8; 16]),
                    family: EventFamily::Metric,
                    kind: EventKind::registered("queue.depth").unwrap(),
                    recorded_at_unix_micros: 20,
                    producer_sequence: 3,
                    producer: identity(),
                    correlations: EventCorrelations::default(),
                    payload: json!({"value": 4}),
                    legacy_provenance: None,
                    retry_of_generation_id: None,
                },
                &policy,
                21,
            )
            .unwrap()
        };
        assert_eq!(make().event_id, EventId::from_bytes([8; 16]));
        assert_eq!(
            make().immutable_digest().unwrap(),
            make().immutable_digest().unwrap()
        );
        let mut retried = make();
        retried.ingested_at_unix_micros = 999;
        retried.retry_of_generation_id = Some(GenerationId::from_bytes([4; 16]));
        retried.producer.writer_instance_id = WriterInstanceId::from_bytes([5; 16]);
        retried.legacy_provenance = Some(LegacyProvenance {
            legacy_source_id: Digest32::from_bytes([6; 32]),
            complete_line_byte_offset: 0,
            complete_record_ordinal: 0,
            line_sha256: Digest32::from_bytes([7; 32]),
            original_schema: "event-envelope-v1".to_string(),
            synthetic_fields: vec![LegacySyntheticField::WriterInstance],
            unavailable_fields: Vec::new(),
        });
        assert_eq!(
            make().immutable_digest().unwrap(),
            retried.immutable_digest().unwrap(),
            "physical retry fields are not logical identity"
        );
        retried.producer_sequence = 999;
        assert_ne!(
            make().immutable_digest().unwrap(),
            retried.immutable_digest().unwrap(),
            "producer sequence is assigned once before fanout"
        );
    }

    #[test]
    fn raw_session_is_domain_separated_and_unknown_payload_fields_are_refused() {
        assert_ne!(
            session_correlation_digest("same").as_bytes(),
            Digest32::sha256(b"same").as_bytes()
        );
        let policy = PayloadNormalizationPolicy::registered(&["allowed"]).unwrap();
        let result = EventEnvelopeV1::normalize(
            NewEventV1 {
                event_id: EventId::from_bytes([7; 16]),
                family: EventFamily::Log,
                kind: EventKind::registered("application.notice").unwrap(),
                recorded_at_unix_micros: 1,
                producer_sequence: 0,
                producer: identity(),
                correlations: EventCorrelations::default(),
                payload: json!({"not_allowed": "raw"}),
                legacy_provenance: None,
                retry_of_generation_id: None,
            },
            &policy,
            2,
        );
        assert!(matches!(result, Err(EnvelopeError::UnknownPayloadField(_))));
    }

    #[test]
    fn native_identity_requires_nonzero_starttime_and_legacy_provenance_is_explicit() {
        let mut invalid = identity();
        invalid
            .native_process
            .as_mut()
            .unwrap()
            .os_pid_starttime_ticks = 0;
        assert!(invalid.native_process.unwrap().validate().is_err());

        let policy = PayloadNormalizationPolicy::registered(&["value"]).unwrap();
        let result = EventEnvelopeV1::normalize(
            NewEventV1 {
                event_id: EventId::from_bytes([1; 16]),
                family: EventFamily::Diagnostic,
                kind: EventKind::registered("identity.missing").unwrap(),
                recorded_at_unix_micros: 1,
                producer_sequence: 1,
                producer: ProducerIdentity {
                    native_process: None,
                    ..identity()
                },
                correlations: EventCorrelations::default(),
                payload: json!({"value": 1}),
                legacy_provenance: None,
                retry_of_generation_id: None,
            },
            &policy,
            2,
        );
        assert!(matches!(result, Err(EnvelopeError::InvalidField(_))));
    }

    #[test]
    fn validation_refuses_nil_identity_and_noncanonical_or_unredacted_payload() {
        let policy = PayloadNormalizationPolicy::registered(&["message"]).unwrap();
        let mut envelope = EventEnvelopeV1::normalize(
            NewEventV1 {
                event_id: EventId::from_bytes([9; 16]),
                family: EventFamily::Log,
                kind: EventKind::registered("validation.test").unwrap(),
                recorded_at_unix_micros: 1,
                producer_sequence: 1,
                producer: identity(),
                correlations: EventCorrelations::default(),
                payload: json!({"message": "safe"}),
                legacy_provenance: None,
                retry_of_generation_id: None,
            },
            &policy,
            2,
        )
        .unwrap();
        envelope.event_id = EventId::from_bytes([0; 16]);
        assert!(envelope.validate().is_err());

        envelope.event_id = EventId::from_bytes([9; 16]);
        envelope.payload = r#"{ "message": "token=raw-secret" }"#.to_string();
        envelope.payload_bytes = envelope.payload.len() as i64;
        envelope.payload_sha256 = Digest32::sha256(envelope.payload.as_bytes());
        assert!(envelope.validate().is_err());
    }
}
