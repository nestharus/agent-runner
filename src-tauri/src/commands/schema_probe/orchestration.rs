//! Declared roles: orchestration

use oulipoly_state::schema_probe;

pub(crate) fn run_session_schema_probe() -> Result<i32, String> {
    render_schema_probe_result(schema_probe::run_schema_probe())
}

fn render_schema_probe_result(
    result: Result<schema_probe::SchemaProbeReport, schema_probe::ProbeError>,
) -> Result<i32, String> {
    match result {
        Ok(report) => super::formatter::render_schema_probe_report(&report),
        Err(error) => super::formatter::render_schema_probe_error(error),
    }
}
