pub const FAILING_MIGRATION_ID: &str = "9999_failing_test_migration";
pub const FAILING_MIGRATION_TARGET_VERSION: i32 = 9999;
pub const FAILING_MIGRATION_SQL: &str = "
    CREATE TABLE age32_failure_marker (id INTEGER PRIMARY KEY);
    INSERT INTO definitely_missing_age32_table (id) VALUES (1);
";

pub fn failing_migration_parts() -> (i32, &'static str, &'static str) {
    (
        FAILING_MIGRATION_TARGET_VERSION,
        FAILING_MIGRATION_ID,
        FAILING_MIGRATION_SQL,
    )
}
