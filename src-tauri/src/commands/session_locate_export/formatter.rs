//! Declared roles: formatter

use super::mapper::{
    export_error_code, export_error_message, metadata_error_code, metadata_error_message,
};
use oulipoly_runtime::session_export::ExportError;
use oulipoly_runtime::session_metadata::MetadataError;

pub(crate) fn emit_metadata_error(err: &MetadataError) {
    emit_json_error_payload(crate::json_error_payload(
        metadata_error_code(err),
        metadata_error_message(err),
    ));
}

pub(super) fn emit_export_error(err: &ExportError) {
    emit_export_json_error(export_error_code(err), &export_error_message(err));
}

pub(super) fn emit_export_json_error(code: &str, message: &str) {
    emit_json_error_payload(crate::json_error_payload(code, message));
}

pub(super) fn emit_json_error_payload(payload: serde_json::Value) {
    eprintln!("{payload}");
}

#[cfg(test)]
mod tests {
    // Step 6b intentionally authors no direct formatter byte tests here.
    //
    // Reason: the current crate has no stderr-capture pattern such as
    // `gag::BufferRedirect`, and AGE-187 forbids inventing a new capture
    // harness for this relocation. Exact stderr byte obligations for
    // `emit_metadata_error`, `emit_export_json_error`, and
    // `emit_json_error_payload` are recorded as residuals and compensated by
    // the existing CLI integration oracle.
}
