//! ## Declared roles
//!
//! - accessor
//! - formatter
//! - mapper
//! - parser
//! - validator
//!
//! Returned-artifact address parser and formatter utilities.

use crate::MessengerError;
use crate::formatter::return_workflow;
use crate::validator::require_positive_version;
use uuid::Uuid;

struct PercentDecodeStep {
    byte: u8,
    next_index: usize,
}

struct ParsedVersionId<'a> {
    uuid_text: &'a str,
    name_text: &'a str,
    version_text: &'a str,
}

pub(crate) fn invocation_from_return_workflow(
    workflow_run_id: &str,
) -> Result<Uuid, MessengerError> {
    let value = return_workflow_uuid_text(workflow_run_id)?;
    Uuid::parse_str(value).map_err(|err| {
        MessengerError::InvalidInput(format!("invalid return invocation UUID {value}: {err}"))
    })
}

fn return_workflow_uuid_text(workflow_run_id: &str) -> Result<&str, MessengerError> {
    workflow_run_id.strip_prefix("return:").ok_or_else(|| {
        MessengerError::InvalidInput("version_id is not in return namespace".to_string())
    })
}

pub(crate) fn parse_version_id(value: &str) -> Result<(Uuid, String, u64), MessengerError> {
    let parsed = parse_version_id_parts(value)?;
    let invocation_uuid = parse_version_id_uuid(parsed.uuid_text)?;
    let name = percent_decode(parsed.name_text)?;
    let version = parse_version_id_version(parsed.version_text)?;
    Ok((invocation_uuid, name, version))
}

fn parse_version_id_parts(value: &str) -> Result<ParsedVersionId<'_>, MessengerError> {
    let rest = value.strip_prefix("store://return/").ok_or_else(|| {
        MessengerError::InvalidInput("version_id must start with store://return/".to_string())
    })?;
    let (uuid_text, rest) = version_id_uuid_and_rest(rest)?;
    let (name_text, version_text) = version_id_name_and_version(rest)?;
    Ok(ParsedVersionId {
        uuid_text,
        name_text,
        version_text,
    })
}

fn version_id_uuid_and_rest(value: &str) -> Result<(&str, &str), MessengerError> {
    value
        .split_once('/')
        .ok_or_else(|| MessengerError::InvalidInput("version_id is missing name".to_string()))
}

fn version_id_name_and_version(value: &str) -> Result<(&str, &str), MessengerError> {
    value
        .rsplit_once('/')
        .ok_or_else(|| MessengerError::InvalidInput("version_id is missing version".to_string()))
}

fn parse_version_id_uuid(value: &str) -> Result<Uuid, MessengerError> {
    Uuid::parse_str(value).map_err(|err| {
        MessengerError::InvalidInput(format!("invalid version_id invocation UUID: {err}"))
    })
}

fn parse_version_id_version(value: &str) -> Result<u64, MessengerError> {
    let version = value.parse::<u64>().map_err(|err| {
        MessengerError::InvalidInput(format!("invalid version_id version: {err}"))
    })?;
    require_positive_version(version)
}

fn percent_decode(value: &str) -> Result<String, MessengerError> {
    let bytes = percent_decoded_bytes(value)?;
    String::from_utf8(bytes)
        .map_err(|err| MessengerError::InvalidInput(format!("version_id name is not UTF-8: {err}")))
}

fn percent_decoded_bytes(value: &str) -> Result<Vec<u8>, MessengerError> {
    let input = value.as_bytes();
    let mut bytes = Vec::new();
    let mut index = 0;
    while index < input.len() {
        let step = percent_decode_step(input, index)?;
        bytes.push(step.byte);
        index = step.next_index;
    }
    Ok(bytes)
}

fn percent_decode_step(input: &[u8], index: usize) -> Result<PercentDecodeStep, MessengerError> {
    if input[index] == b'%' {
        return percent_escape_decode_step(input, index);
    }
    Ok(PercentDecodeStep {
        byte: input[index],
        next_index: index + 1,
    })
}

fn percent_escape_decode_step(
    input: &[u8],
    index: usize,
) -> Result<PercentDecodeStep, MessengerError> {
    require_percent_escape_len(input, index)?;
    let hex = percent_escape_hex(input, index)?;
    let byte = parse_percent_escape_byte(hex)?;
    Ok(PercentDecodeStep {
        byte,
        next_index: index + 3,
    })
}

fn require_percent_escape_len(input: &[u8], index: usize) -> Result<(), MessengerError> {
    if index + 2 >= input.len() {
        return Err(MessengerError::InvalidInput(
            "invalid percent escape in version_id".to_string(),
        ));
    }
    Ok(())
}

fn percent_escape_hex(input: &[u8], index: usize) -> Result<&str, MessengerError> {
    std::str::from_utf8(&input[index + 1..index + 3]).map_err(|err| {
        MessengerError::InvalidInput(format!("invalid percent escape in version_id: {err}"))
    })
}

fn parse_percent_escape_byte(value: &str) -> Result<u8, MessengerError> {
    u8::from_str_radix(value, 16).map_err(|err| {
        MessengerError::InvalidInput(format!("invalid percent escape in version_id: {err}"))
    })
}

pub(crate) fn return_lookup_key(
    invocation_uuid: Uuid,
    name: String,
) -> oulipoly_agent_store::ArtifactKey {
    oulipoly_agent_store::ArtifactKey {
        workflow_run_id: return_workflow(invocation_uuid),
        artifact_name: name,
    }
}
