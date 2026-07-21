//! formatter

use oulipoly_runtime::services::{RotationFailedReason, ServiceError};

pub(super) fn emit_stderr(message: &str) {
    eprintln!("{message}");
}

pub(super) fn repl_launch_failure_message(provider: &oulipoly_config::ProviderConfig) -> String {
    format!(
        "Provider {} has no interactive_args; cannot launch interactively",
        provider.name
    )
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

pub(super) fn migration_dependency_failure(message: &str) -> String {
    format!("migration failed: {message}")
}

pub(super) fn migration_service_failure(error: &ServiceError) -> String {
    format!("migration service failed: {error}")
}

pub(super) fn resume_service_failure(error: &str) -> String {
    format!("resume service failed: {error}")
}
