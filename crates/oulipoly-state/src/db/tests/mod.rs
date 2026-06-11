//! ## Declared roles
//!
//! - orchestration
//!
//! Role set: { orchestration }

use super::opening_read_only::{classify_read_only_open_error, shm_path, wal_path};
use super::*;
use crate::test_support::env_lock;
use tempfile::TempDir;
use uuid::Uuid;

mod failing_migration {
    include!("../../../tests/fixtures/failing_migration.rs");
}

mod age132_opening_tests;
mod age132_preview_tests;
mod age132_resume_tests_1;
mod age132_resume_tests_2;
mod age132_session_turn_tests;
mod age160_lifecycle_tests;
mod age160_marker_tests;
mod age160_read_only_tests;
mod artifact_tests;
mod chain_backfill_tests_1;
mod chain_backfill_tests_2;
mod common;
mod invocation_lifecycle_tests_1;
mod invocation_lifecycle_tests_2;
mod invocation_records_tests;
mod invocation_schema_tests;
mod legacy_invocation_migration_tests;
mod migration_failure_tests;
mod migration_returning_clause_tests;
mod opening_core_tests;
mod provider_aggregate_tests;
mod provider_migration_tests_1;
mod provider_migration_tests_2;
mod quota_exhaustion_tests;
mod quota_refresh_tests_1;
mod quota_refresh_tests_2;
mod quota_refresh_tests_3;
mod resume_resolution_tests_1;
mod resume_resolution_tests_2;
mod resume_window_tests;
mod session_capture_tests;
mod session_turn_schema_tests;
mod session_turn_tests_1;
mod session_turn_tests_2;
mod setup_counter_tests;
mod setup_crud_tests_1;
mod setup_crud_tests_2;
