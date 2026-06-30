use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

pub const SCHEMA_DRAFT_2020_12: &str = "https://json-schema.org/draft/2020-12/schema";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchemaFile {
    pub filename: &'static str,
    pub contents: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubcommandSchema {
    pub subcommand: &'static str,
    pub schema_file: &'static str,
    pub request_def: &'static str,
    pub response_def: Option<&'static str>,
    pub error_response_def: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaunchEventSchema {
    pub kind: &'static str,
    pub schema_file: &'static str,
    pub event_def: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SchemaValidationError {
    #[error("unknown schema file: {0}")]
    UnknownSchemaFile(String),
    #[error("unknown subcommand: {0}")]
    UnknownSubcommand(String),
    #[error("unknown launch event kind: {0}")]
    UnknownLaunchEventKind(String),
    #[error("subcommand has no single response envelope: {0}")]
    MissingResponseEnvelope(String),
    #[error("schema parse failed for {schema_file}: {message}")]
    SchemaParse {
        schema_file: &'static str,
        message: String,
    },
    #[error("schema compile failed for {schema_file}#{definition}: {message}")]
    SchemaCompile {
        schema_file: &'static str,
        definition: &'static str,
        message: String,
    },
    #[error("schema validation failed for {schema_file}#{definition}: {errors:?}")]
    Validation {
        schema_file: &'static str,
        definition: &'static str,
        errors: Vec<String>,
    },
}

#[derive(Debug, Clone)]
pub struct SchemaRegistry {
    files: BTreeMap<&'static str, &'static str>,
    subcommands: BTreeMap<&'static str, SubcommandSchema>,
    launch_events: BTreeMap<&'static str, LaunchEventSchema>,
}

impl Default for SchemaRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SchemaRegistry {
    pub fn new() -> Self {
        Self {
            files: SCHEMA_FILES
                .iter()
                .map(|file| (file.filename, file.contents))
                .collect(),
            subcommands: SUBCOMMAND_SCHEMAS
                .iter()
                .map(|schema| (schema.subcommand, *schema))
                .collect(),
            launch_events: LAUNCH_EVENT_SCHEMAS
                .iter()
                .map(|schema| (schema.kind, *schema))
                .collect(),
        }
    }

    pub fn schema_by_file(&self, filename: &str) -> Option<&'static str> {
        let normalized = normalize_schema_filename(filename);
        self.files.get(normalized.as_str()).copied()
    }

    pub fn schema_for_subcommand(&self, subcommand: &str) -> Option<SubcommandSchema> {
        self.subcommands.get(subcommand).copied()
    }

    pub fn schema_for_launch_event(&self, kind: &str) -> Option<LaunchEventSchema> {
        self.launch_events.get(kind).copied()
    }

    pub fn validate_request(
        &self,
        subcommand: &str,
        instance: &Value,
    ) -> Result<(), SchemaValidationError> {
        let target = self
            .schema_for_subcommand(subcommand)
            .ok_or_else(|| SchemaValidationError::UnknownSubcommand(subcommand.to_owned()))?;
        self.validate_definition(target.schema_file, target.request_def, instance)
    }

    pub fn validate_response(
        &self,
        subcommand: &str,
        instance: &Value,
    ) -> Result<(), SchemaValidationError> {
        let target = self
            .schema_for_subcommand(subcommand)
            .ok_or_else(|| SchemaValidationError::UnknownSubcommand(subcommand.to_owned()))?;
        let definition = target
            .response_def
            .ok_or_else(|| SchemaValidationError::MissingResponseEnvelope(subcommand.to_owned()))?;
        self.validate_definition(target.schema_file, definition, instance)
    }

    pub fn validate_error_response(
        &self,
        subcommand: &str,
        instance: &Value,
    ) -> Result<(), SchemaValidationError> {
        let target = self
            .schema_for_subcommand(subcommand)
            .ok_or_else(|| SchemaValidationError::UnknownSubcommand(subcommand.to_owned()))?;
        let definition = target
            .error_response_def
            .ok_or_else(|| SchemaValidationError::MissingResponseEnvelope(subcommand.to_owned()))?;
        self.validate_definition(target.schema_file, definition, instance)
    }

    pub fn validate_launch_event(
        &self,
        kind: &str,
        instance: &Value,
    ) -> Result<(), SchemaValidationError> {
        let target = self
            .schema_for_launch_event(kind)
            .ok_or_else(|| SchemaValidationError::UnknownLaunchEventKind(kind.to_owned()))?;
        self.validate_definition(target.schema_file, target.event_def, instance)
    }

    fn validate_definition(
        &self,
        schema_file: &'static str,
        definition: &'static str,
        instance: &Value,
    ) -> Result<(), SchemaValidationError> {
        let mut schema = parse_named_schema(schema_file)?;
        let common = parse_schema("common.schema.json", COMMON_SCHEMA)?;
        merge_common_defs_and_rewrite_refs(&mut schema, &common);

        let wrapper = map_definition_wrapper(&schema, definition);
        validate_instance_with_wrapper(schema_file, definition, &wrapper, instance)
    }
}

pub fn schema_by_file(filename: &str) -> Option<&'static str> {
    let normalized = normalize_schema_filename(filename);
    SCHEMA_FILES
        .iter()
        .find(|file| file.filename == normalized)
        .map(|file| file.contents)
}

pub fn schema_for_subcommand(subcommand: &str) -> Option<SubcommandSchema> {
    SchemaRegistry::new().schema_for_subcommand(subcommand)
}

pub fn schema_for_launch_event(kind: &str) -> Option<LaunchEventSchema> {
    SchemaRegistry::new().schema_for_launch_event(kind)
}

pub fn validate_request(subcommand: &str, instance: &Value) -> Result<(), SchemaValidationError> {
    SchemaRegistry::new().validate_request(subcommand, instance)
}

pub fn validate_response(subcommand: &str, instance: &Value) -> Result<(), SchemaValidationError> {
    SchemaRegistry::new().validate_response(subcommand, instance)
}

pub fn validate_error_response(
    subcommand: &str,
    instance: &Value,
) -> Result<(), SchemaValidationError> {
    SchemaRegistry::new().validate_error_response(subcommand, instance)
}

pub fn validate_launch_event(kind: &str, instance: &Value) -> Result<(), SchemaValidationError> {
    SchemaRegistry::new().validate_launch_event(kind, instance)
}

macro_rules! row {
    ($subcommand:literal, $schema_file:literal, $stem:literal) => {
        SubcommandSchema {
            subcommand: $subcommand,
            schema_file: $schema_file,
            request_def: concat!($stem, "Request"),
            response_def: Some(concat!($stem, "Response")),
            error_response_def: Some(concat!($stem, "ErrorResponse")),
        }
    };
}

macro_rules! event_row {
    ($kind:literal, $definition:literal) => {
        LaunchEventSchema {
            kind: $kind,
            schema_file: "launch.schema.json",
            event_def: $definition,
        }
    };
}

const COMMON_SCHEMA: &str = include_str!("../../../contract/v1/common.schema.json");

pub const SCHEMA_FILES: &[SchemaFile] = &[
    SchemaFile {
        filename: "common.schema.json",
        contents: COMMON_SCHEMA,
    },
    SchemaFile {
        filename: "describe.schema.json",
        contents: include_str!("../../../contract/v1/describe.schema.json"),
    },
    SchemaFile {
        filename: "schema.schema.json",
        contents: include_str!("../../../contract/v1/schema.schema.json"),
    },
    SchemaFile {
        filename: "settings.schema.json",
        contents: include_str!("../../../contract/v1/settings.schema.json"),
    },
    SchemaFile {
        filename: "launch.schema.json",
        contents: include_str!("../../../contract/v1/launch.schema.json"),
    },
    SchemaFile {
        filename: "policy.schema.json",
        contents: include_str!("../../../contract/v1/policy.schema.json"),
    },
    SchemaFile {
        filename: "quota.schema.json",
        contents: include_str!("../../../contract/v1/quota.schema.json"),
    },
    SchemaFile {
        filename: "terminal.schema.json",
        contents: include_str!("../../../contract/v1/terminal.schema.json"),
    },
    SchemaFile {
        filename: "session.schema.json",
        contents: include_str!("../../../contract/v1/session.schema.json"),
    },
    SchemaFile {
        filename: "rotation.schema.json",
        contents: include_str!("../../../contract/v1/rotation.schema.json"),
    },
    SchemaFile {
        filename: "discovery.schema.json",
        contents: include_str!("../../../contract/v1/discovery.schema.json"),
    },
    SchemaFile {
        filename: "setup.schema.json",
        contents: include_str!("../../../contract/v1/setup.schema.json"),
    },
    SchemaFile {
        filename: "migration.schema.json",
        contents: include_str!("../../../contract/v1/migration.schema.json"),
    },
];

pub const SUBCOMMAND_SCHEMAS: &[SubcommandSchema] = &[
    row!("describe", "describe.schema.json", "Describe"),
    row!("schema", "schema.schema.json", "Schema"),
    row!("settings.list", "settings.schema.json", "SettingsList"),
    row!("settings.get", "settings.schema.json", "SettingsGet"),
    row!("settings.create", "settings.schema.json", "SettingsCreate"),
    row!("settings.update", "settings.schema.json", "SettingsUpdate"),
    row!("settings.delete", "settings.schema.json", "SettingsDelete"),
    row!(
        "settings.validate",
        "settings.schema.json",
        "SettingsValidate"
    ),
    row!(
        "settings.migrate",
        "settings.schema.json",
        "SettingsMigrate"
    ),
    row!("policy.evaluate", "policy.schema.json", "PolicyEvaluate"),
    SubcommandSchema {
        subcommand: "launch",
        schema_file: "launch.schema.json",
        request_def: "LaunchRequest",
        response_def: None,
        error_response_def: None,
    },
    row!(
        "terminal.classify",
        "terminal.schema.json",
        "TerminalClassify"
    ),
    row!("quota.source", "quota.schema.json", "QuotaSource"),
    row!("quota.probe", "quota.schema.json", "QuotaProbe"),
    row!(
        "quota.refresh_auth",
        "quota.schema.json",
        "QuotaRefreshAuth"
    ),
    row!(
        "session.locate_transcript",
        "session.schema.json",
        "SessionLocateTranscript"
    ),
    row!(
        "session.enumerate",
        "session.schema.json",
        "SessionEnumerate"
    ),
    row!(
        "session.read_turns",
        "session.schema.json",
        "SessionReadTurns"
    ),
    row!("session.capture", "session.schema.json", "SessionCapture"),
    row!("session.export", "session.schema.json", "SessionExport"),
    row!("session.replace", "session.schema.json", "SessionReplace"),
    row!("rotation.assess", "rotation.schema.json", "RotationAssess"),
    row!(
        "rotation.materialize",
        "rotation.schema.json",
        "RotationMaterialize"
    ),
    row!(
        "discovery.models",
        "discovery.schema.json",
        "DiscoveryModels"
    ),
    row!(
        "discovery.accounts",
        "discovery.schema.json",
        "DiscoveryAccounts"
    ),
    row!("setup.detect", "setup.schema.json", "SetupDetect"),
    row!(
        "setup.install_plan",
        "setup.schema.json",
        "SetupInstallPlan"
    ),
    row!("setup.sync_plan", "setup.schema.json", "SetupSyncPlan"),
    row!("setup_brain.turn", "setup.schema.json", "SetupBrainTurn"),
    row!("migration.plan", "migration.schema.json", "MigrationPlan"),
    row!("migration.apply", "migration.schema.json", "MigrationApply"),
];

pub const LAUNCH_EVENT_SCHEMAS: &[LaunchEventSchema] = &[
    event_row!("stdout", "LaunchStdoutEvent"),
    event_row!("stderr", "LaunchStderrEvent"),
    event_row!("marker", "LaunchMarkerEvent"),
    event_row!("heartbeat", "LaunchHeartbeatEvent"),
    event_row!("exit", "LaunchExitEvent"),
];

fn parse_schema(
    schema_file: &'static str,
    contents: &'static str,
) -> Result<Value, SchemaValidationError> {
    serde_json::from_str(contents).map_err(|err| SchemaValidationError::SchemaParse {
        schema_file,
        message: err.to_string(),
    })
}

fn parse_named_schema(schema_file: &'static str) -> Result<Value, SchemaValidationError> {
    let contents = schema_contents(schema_file)?;
    parse_schema(schema_file, contents)
}

fn schema_contents(schema_file: &'static str) -> Result<&'static str, SchemaValidationError> {
    schema_by_file(schema_file).ok_or_else(|| format_unknown_schema_file(schema_file))
}

fn map_definition_wrapper(schema: &Value, definition: &'static str) -> Value {
    format_definition_wrapper(definition_wrapper_defs(schema), definition)
}

fn definition_wrapper_defs(schema: &Value) -> Value {
    schema
        .get("$defs")
        .cloned()
        .unwrap_or_else(empty_defs_object)
}

fn format_definition_wrapper(defs: Value, definition: &'static str) -> Value {
    json!({
        "$schema": SCHEMA_DRAFT_2020_12,
        "$defs": defs,
        "$ref": format!("#/$defs/{definition}")
    })
}

fn validate_instance_with_wrapper(
    schema_file: &'static str,
    definition: &'static str,
    wrapper: &Value,
    instance: &Value,
) -> Result<(), SchemaValidationError> {
    let validator = compile_definition_validator(schema_file, definition, wrapper)?;
    let errors = format_validation_errors(&validator, instance);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format_validation_failure(schema_file, definition, errors))
    }
}

fn compile_definition_validator(
    schema_file: &'static str,
    definition: &'static str,
    wrapper: &Value,
) -> Result<jsonschema::Validator, SchemaValidationError> {
    jsonschema::validator_for(wrapper)
        .map_err(|err| format_schema_compile_error(schema_file, definition, err.to_string()))
}

fn format_validation_errors(validator: &jsonschema::Validator, instance: &Value) -> Vec<String> {
    let errors = collect_validation_errors(validator, instance);
    let mut messages = format_validation_error_messages(errors);
    sort_validation_error_messages(&mut messages);
    messages
}

fn collect_validation_errors<'a>(
    validator: &'a jsonschema::Validator,
    instance: &'a Value,
) -> Vec<jsonschema::ValidationError<'a>> {
    validator.iter_errors(instance).collect()
}

fn format_validation_error_messages(errors: Vec<jsonschema::ValidationError<'_>>) -> Vec<String> {
    errors
        .into_iter()
        .map(format_validation_error_message)
        .collect()
}

fn format_validation_error_message(error: jsonschema::ValidationError<'_>) -> String {
    error.to_string()
}

fn sort_validation_error_messages(errors: &mut [String]) {
    errors.sort();
}

fn format_unknown_schema_file(schema_file: &'static str) -> SchemaValidationError {
    SchemaValidationError::UnknownSchemaFile(schema_file.to_owned())
}

fn format_schema_compile_error(
    schema_file: &'static str,
    definition: &'static str,
    message: String,
) -> SchemaValidationError {
    SchemaValidationError::SchemaCompile {
        schema_file,
        definition,
        message,
    }
}

fn format_validation_failure(
    schema_file: &'static str,
    definition: &'static str,
    errors: Vec<String>,
) -> SchemaValidationError {
    SchemaValidationError::Validation {
        schema_file,
        definition,
        errors,
    }
}

fn empty_defs_object() -> Value {
    Value::Object(Map::new())
}

fn normalize_schema_filename(filename: &str) -> String {
    format_schema_filename(schema_filename_without_contract_prefix(filename))
}

fn schema_filename_without_contract_prefix(filename: &str) -> &str {
    filename.strip_prefix("contract/v1/").unwrap_or(filename)
}

fn format_schema_filename(filename: &str) -> String {
    if has_schema_filename_suffix(filename) {
        filename.to_owned()
    } else {
        format_schema_filename_with_suffix(filename)
    }
}

fn has_schema_filename_suffix(filename: &str) -> bool {
    filename.ends_with(".schema.json")
}

fn format_schema_filename_with_suffix(filename: &str) -> String {
    format!("{filename}.schema.json")
}

fn merge_common_defs_and_rewrite_refs(schema: &mut Value, common: &Value) {
    rewrite_common_refs(schema);

    let Some(common_defs) = common_schema_defs(common) else {
        return;
    };
    let schema_defs = validated_schema_defs_mut(schema);
    merge_missing_schema_defs(schema_defs, common_defs);
}

fn common_schema_defs(common: &Value) -> Option<&Map<String, Value>> {
    common.get("$defs").and_then(Value::as_object)
}

fn validated_schema_defs_mut(schema: &mut Value) -> &mut Map<String, Value> {
    schema
        .as_object_mut()
        .expect("schema root must be a JSON object")
        .entry("$defs")
        .or_insert_with(empty_defs_object)
        .as_object_mut()
        .expect("schema $defs must be a JSON object")
}

fn merge_missing_schema_defs(
    schema_defs: &mut Map<String, Value>,
    common_defs: &Map<String, Value>,
) {
    for (name, definition) in common_defs {
        schema_defs
            .entry(name.clone())
            .or_insert_with(|| definition.clone());
    }
}

fn rewrite_common_refs(value: &mut Value) {
    match value {
        Value::Object(object) => rewrite_common_refs_in_object(object),
        Value::Array(values) => rewrite_common_refs_in_array(values),
        _ => {}
    }
}

fn rewrite_common_refs_in_object(object: &mut Map<String, Value>) {
    rewrite_common_ref_field(object);
    rewrite_common_refs_in_children(object.values_mut());
}

fn rewrite_common_ref_field(object: &mut Map<String, Value>) {
    if let Some(replacement) = map_common_ref_replacement(object) {
        object.insert("$ref".to_owned(), replacement);
    }
}

fn map_common_ref_replacement(object: &Map<String, Value>) -> Option<Value> {
    parse_common_ref_definition(object).map(format_local_definition_ref)
}

fn parse_common_ref_definition(object: &Map<String, Value>) -> Option<String> {
    object
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|reference| reference.strip_prefix("common.schema.json#/$defs/"))
        .map(str::to_owned)
}

fn format_local_definition_ref(definition: String) -> Value {
    Value::String(format!("#/$defs/{definition}"))
}

fn rewrite_common_refs_in_array(values: &mut [Value]) {
    rewrite_common_refs_in_children(values.iter_mut());
}

fn rewrite_common_refs_in_children<'a, I>(values: I)
where
    I: IntoIterator<Item = &'a mut Value>,
{
    for child in values {
        rewrite_common_refs(child);
    }
}
