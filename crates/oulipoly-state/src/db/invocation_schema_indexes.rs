//! Invocation index schema ownership shared by fresh creation, repair, and migrations.

use super::*;

const INVOCATION_BASE_INDEX_SQL: &str = "CREATE INDEX IF NOT EXISTS idx_invocations_uuid
        ON invocations (invocation_uuid);
     CREATE INDEX IF NOT EXISTS idx_invocations_parent
        ON invocations (parent_invocation_id, created_at);
     CREATE INDEX IF NOT EXISTS idx_invocations_provider_created
        ON invocations (provider_name, created_at);
     CREATE INDEX IF NOT EXISTS idx_invocations_provider_session
        ON invocations (provider_name, session_id)
        WHERE session_id IS NOT NULL;";

const INVOCATION_RUNNING_PROJECTION_INDEX_SQL: &str =
    "CREATE INDEX IF NOT EXISTS idx_invocations_parent_running_created
        ON invocations (
            parent_invocation_id,
            (status = 'running') DESC,
            created_at,
            id
        );
     CREATE INDEX IF NOT EXISTS idx_invocations_running_parent
        ON invocations (parent_invocation_id, id)
        WHERE status = 'running';";

impl StateDb {
    pub(super) fn invocations_index_sql() -> &'static str {
        INVOCATION_BASE_INDEX_SQL
    }
}

pub(crate) fn invocation_running_projection_index_sql() -> &'static str {
    INVOCATION_RUNNING_PROJECTION_INDEX_SQL
}
