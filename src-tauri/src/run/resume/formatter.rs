//! formatter

use std::fmt::Display;

use oulipoly_runtime::services::{RotationFailedReason, ServiceError};

pub(super) fn emit_stderr(message: &str) {
    eprintln!("{message}");
}

pub(super) fn emit_resume_short_line(selected_provider: &str) {
    eprintln!("[resume] -> {selected_provider}");
}

pub(super) fn emit_missing_resume_block(provider_name: &str) {
    eprintln!("provider {provider_name} has no [providers.resume] block; cannot resume");
}

pub(super) fn emit_migration_dependency_failure(message: &str) {
    eprintln!("migration failed: {message}");
}

pub(super) fn emit_migration_service_failure(error: impl Display) {
    eprintln!("migration service failed: {error}");
}

pub(super) fn emit_finalize_invocation_warning(error: impl Display) {
    eprintln!("Warning: Failed to finalize invocation: {error}");
}

pub(super) fn emit_returned_artifacts_error(error: impl Display) {
    eprintln!("Error: Failed to record returned artifacts: {error}");
}

pub(super) fn emit_routing_retry(provider_name: &str) {
    eprintln!("[routing] provider {provider_name} unavailable; rotating to another provider");
}

pub(super) fn emit_diagnostics_category(category: &str) {
    eprintln!("[diagnostics: {category}]");
}

pub(super) fn resume_provider_registry_failure(error: String) -> String {
    format!("failed to build resume provider registry: {error}")
}

pub(super) fn resume_acceptance_service_failure(error: ServiceError) -> String {
    format!("resume acceptance service failed: {error}")
}

pub(super) fn rotation_failed_reason(reason: &RotationFailedReason) -> String {
    match reason {
        RotationFailedReason::WorkingSetExhausted { candidates_tried } => format!(
            "migration failed: working set exhausted after trying providers [{}]",
            candidates_tried.join(", ")
        ),
        RotationFailedReason::ManualTargetNotInPool { target, pool } => format!(
            "cannot rotate: provider \"{target}\" is not in model pool [{}]",
            pool.join(", ")
        ),
        RotationFailedReason::ManualTargetNotMigratable { source, target } => {
            format!("cannot rotate: {source} -> {target} is not a migratable storage-class pair")
        }
        RotationFailedReason::ManualTargetIsSingleProviderPool { provider } => {
            format!("cannot rotate: model pool has only one provider ({provider})")
        }
        RotationFailedReason::ManualTargetActiveNotInPool { active } => {
            format!("cannot rotate: session-active provider \"{active}\" is not in the model pool")
        }
    }
}
