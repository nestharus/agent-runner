//! Runner's admission of `oulipoly.provider/v1` wire values.
//!
//! The schemas, the operation table and the admission rules, including
//! `describe`'s `preferred_contract` rule, are the shared SDK contract's;
//! Runner defines none of its own. Runner adds only the host-selected
//! extension subcommands its provider client carries, whose envelopes are the
//! base contract's and whose payloads each extension admits itself.

use agent_provider_contract::resident_session;
use serde_json::{Value, json};

pub use agent_provider_contract::schemas::{
    LAUNCH_EVENT_SCHEMAS, LaunchEventSchema, SCHEMA_DRAFT_2020_12, SCHEMA_FILES,
    SUBCOMMAND_SCHEMAS, SchemaFile, SchemaValidationError, SubcommandSchema, schema_by_file,
};

const COMMON_SCHEMA_FILE: &str = "common.schema.json";

/// Host-selected extension subcommands that travel over the base envelopes.
pub const HOST_EXTENSION_SUBCOMMANDS: &[SubcommandSchema] = &[SubcommandSchema {
    subcommand: resident_session::PREPARE_SUBCOMMAND,
    schema_file: COMMON_SCHEMA_FILE,
    request_def: "RequestEnvelope",
    response_def: Some("SuccessResponseEnvelope"),
    error_response_def: Some("ErrorResponseEnvelope"),
}];

#[derive(Debug, Clone, Default)]
pub struct SchemaRegistry {
    contract: agent_provider_contract::SchemaRegistry,
}

impl SchemaRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn schema_by_file(&self, filename: &str) -> Option<&'static str> {
        self.contract.schema_by_file(filename)
    }

    pub fn schema_for_subcommand(&self, subcommand: &str) -> Option<SubcommandSchema> {
        self.contract
            .schema_for_subcommand(subcommand)
            .or_else(|| host_extension(subcommand))
    }

    pub fn schema_for_launch_event(&self, kind: &str) -> Option<LaunchEventSchema> {
        self.contract.schema_for_launch_event(kind)
    }

    pub fn validate_request(
        &self,
        subcommand: &str,
        instance: &Value,
    ) -> Result<(), SchemaValidationError> {
        match host_extension(subcommand) {
            Some(row) => validate_common_envelope(row.request_def, instance),
            None => self.contract.validate_request(subcommand, instance),
        }
    }

    pub fn validate_response(
        &self,
        subcommand: &str,
        instance: &Value,
    ) -> Result<(), SchemaValidationError> {
        match host_extension(subcommand).and_then(|row| row.response_def) {
            Some(definition) => validate_common_envelope(definition, instance),
            None => self.contract.validate_response(subcommand, instance),
        }
    }

    pub fn validate_error_response(
        &self,
        subcommand: &str,
        instance: &Value,
    ) -> Result<(), SchemaValidationError> {
        match host_extension(subcommand).and_then(|row| row.error_response_def) {
            Some(definition) => validate_common_envelope(definition, instance),
            None => self.contract.validate_error_response(subcommand, instance),
        }
    }

    pub fn validate_launch_event(
        &self,
        kind: &str,
        instance: &Value,
    ) -> Result<(), SchemaValidationError> {
        self.contract.validate_launch_event(kind, instance)
    }
}

fn host_extension(subcommand: &str) -> Option<SubcommandSchema> {
    HOST_EXTENSION_SUBCOMMANDS
        .iter()
        .find(|row| row.subcommand == subcommand)
        .copied()
}

/// Validates against one of the contract's own common envelope definitions.
fn validate_common_envelope(
    definition: &'static str,
    instance: &Value,
) -> Result<(), SchemaValidationError> {
    let contents = schema_by_file(COMMON_SCHEMA_FILE)
        .ok_or_else(|| SchemaValidationError::UnknownSchemaFile(COMMON_SCHEMA_FILE.to_owned()))?;
    let common: Value =
        serde_json::from_str(contents).map_err(|error| SchemaValidationError::SchemaParse {
            schema_file: COMMON_SCHEMA_FILE,
            message: error.to_string(),
        })?;
    let wrapper = json!({
        "$schema": SCHEMA_DRAFT_2020_12,
        "$defs": common.get("$defs").cloned().unwrap_or_else(|| json!({})),
        "$ref": format!("#/$defs/{definition}"),
    });
    let validator = jsonschema::validator_for(&wrapper).map_err(|error| {
        SchemaValidationError::SchemaCompile {
            schema_file: COMMON_SCHEMA_FILE,
            definition,
            message: error.to_string(),
        }
    })?;
    let mut errors = validator
        .iter_errors(instance)
        .map(|error| error.to_string())
        .collect::<Vec<_>>();
    if errors.is_empty() {
        return Ok(());
    }
    errors.sort();
    Err(SchemaValidationError::Validation {
        schema_file: COMMON_SCHEMA_FILE,
        definition,
        errors,
    })
}
