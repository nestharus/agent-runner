//! Declared roles: mapper, predicate, formatter

use oulipoly_state::ReadOnlyOpenError;
use oulipoly_state::schema_probe::{ProbeError, SchemaProbeReport};

pub(super) fn schema_probe_report_is_incompatible(report: &SchemaProbeReport) -> bool {
    report.state_db.exists && !report.state_db.compatible
}

pub(super) fn format_schema_incompatible_message(report: &SchemaProbeReport) -> String {
    format!(
        "state database schema is incompatible: {}",
        report.state_db.path.display()
    )
}

pub(super) fn probe_error_message(error: ProbeError) -> String {
    match error {
        ProbeError::StatePath { message } | ProbeError::Inspect { message } => message,
        ProbeError::Open { error } => match error {
            ReadOnlyOpenError::Missing { path } => {
                format!("state database is missing: {}", path.display())
            }
            ReadOnlyOpenError::NotADatabase { path, message } => {
                format!(
                    "state database is not a SQLite database at {}: {message}",
                    path.display()
                )
            }
            ReadOnlyOpenError::PermissionDenied { path } => {
                format!(
                    "permission denied reading state database at {}",
                    path.display()
                )
            }
            ReadOnlyOpenError::WalSidecarError { path, message } => {
                format!(
                    "failed to read SQLite WAL sidecar for state database at {}: {message}",
                    path.display()
                )
            }
            ReadOnlyOpenError::Operational { message } => message,
        },
    }
}
