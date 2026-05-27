//! Declared roles: formatter

use oulipoly_state::schema_probe::{ProbeError, SchemaProbeReport};

use crate::json_error::write_json_error;

#[cfg(test)]
const _: fn(&str, String) = crate::json_error::emit_json_error;

pub(super) fn render_schema_probe_report(report: &SchemaProbeReport) -> Result<i32, String> {
    if super::mapper::schema_probe_report_is_incompatible(report) {
        write_json_error(
            "schema-incompatible",
            &super::mapper::format_schema_incompatible_message(report),
        )?;
        return Ok(14);
    }
    let json = serde_json::to_string(report).map_err(format_schema_probe_serialize_error)?;
    println!("{json}");
    Ok(0)
}

fn format_schema_probe_serialize_error(error: serde_json::Error) -> String {
    format!("Failed to serialize schema probe report: {error}")
}

pub(super) fn render_schema_probe_error(error: ProbeError) -> Result<i32, String> {
    write_json_error(
        "operational-error",
        &super::mapper::probe_error_message(error),
    )?;
    Ok(1)
}
