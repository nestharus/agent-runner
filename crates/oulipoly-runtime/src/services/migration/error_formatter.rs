//! ## Declared roles
//! formatter

use crate::services::ServiceError;

pub(super) fn construct_migration_service_error(
    error: crate::rotation_domain::ExternalRotationError,
) -> ServiceError {
    ServiceError::Dependency {
        message: error.to_string(),
    }
}

pub(super) fn migration_dependency_error(error: crate::migration::MigrationError) -> ServiceError {
    ServiceError::Dependency {
        message: format_migration_dependency_error(&error),
    }
}

fn format_migration_dependency_error(error: &crate::migration::MigrationError) -> String {
    format!("{error:?}")
}
