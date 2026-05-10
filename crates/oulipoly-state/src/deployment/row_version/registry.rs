pub struct TableRegistration {
    pub table: &'static str,
    pub primary_key_columns: &'static [&'static str],
    pub payload_columns: &'static [&'static str],
    pub kind: RowKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    /// Pure UPDATE/INSERT row; trigger advances row_version on every write.
    Mutable,
}

/// Payload columns from PRAGMA order: profile_name, auth_method, auth_status, created_at.
const ACCOUNTS_PAYLOAD: &[&str] = &["profile_name", "auth_method", "auth_status", "created_at"];
/// Payload columns from PRAGMA order: display_name, installed, version, config_dir, last_synced.
const CLI_PROVIDERS_PAYLOAD: &[&str] = &[
    "display_name",
    "installed",
    "version",
    "config_dir",
    "last_synced",
];
/// Payload columns from PRAGMA order: discovered_at, cli_version.
const DISCOVERED_MODELS_PAYLOAD: &[&str] = &["discovered_at", "cli_version"];
/// Payload columns from PRAGMA order, excluding id and row_version.
const INVOCATION_RETURNED_ARTIFACTS_PAYLOAD: &[&str] = &[
    "invocation_id",
    "ordinal",
    "version_id",
    "name",
    "workflow_run_id",
    "artifact_name",
    "version",
    "sha256",
    "content_len",
    "format_hint",
    "verdict_line",
    "source_kind",
    "source_json",
    "returned_at",
];
/// Payload columns from PRAGMA order, excluding id and row_version.
const INVOCATIONS_PAYLOAD: &[&str] = &[
    "invocation_uuid",
    "model_name",
    "provider_name",
    "provider_index",
    "parent_invocation_id",
    "status",
    "success",
    "exit_code",
    "error_category",
    "terminal_reason",
    "session_id",
    "session_capture_method",
    "resume_acceptance_status",
    "resume_acceptance_evidence",
    "created_at",
    "finished_at",
    "provider_session_id",
    "resume_input_id",
    "provider_session_capture_method",
];
/// Payload columns from PRAGMA order: data, created_at.
const MEMORY_EDGES_PAYLOAD: &[&str] = &["data", "created_at"];
/// Payload columns from PRAGMA order: node_type, label, data, created_at, updated_at.
const MEMORY_NODES_PAYLOAD: &[&str] = &["node_type", "label", "data", "created_at", "updated_at"];
/// Payload columns from PRAGMA order: display_name, param_type, description, cli_mapping.
const MODEL_PARAMETERS_PAYLOAD: &[&str] =
    &["display_name", "param_type", "description", "cli_mapping"];
/// Payload columns from PRAGMA order: used_percent, resets_at, last_delta_percent, last_delta_calls.
const PROVIDER_QUOTA_WINDOWS_PAYLOAD: &[&str] = &[
    "used_percent",
    "resets_at",
    "last_delta_percent",
    "last_delta_calls",
];
/// Payload columns from PRAGMA order, excluding provider_name and row_version.
const PROVIDER_QUOTAS_PAYLOAD: &[&str] = &[
    "used_percent",
    "resets_at",
    "calls_since_refresh",
    "refreshed_at",
    "last_empty_refresh_at",
    "exhausted_at",
    "topology_peak_live_window_count",
    "last_topology_probe_at",
];
/// Payload columns from PRAGMA order: invocation_count, error_count, last_error, last_error_at, last_invoked_at.
const PROVIDERS_PAYLOAD: &[&str] = &[
    "invocation_count",
    "error_count",
    "last_error",
    "last_error_at",
    "last_invoked_at",
];
/// Payload columns from PRAGMA order, excluding id and row_version.
const SESSION_CHAIN_SEGMENTS_PAYLOAD: &[&str] = &[
    "chain_id",
    "provider_name",
    "session_id",
    "started_at",
    "ended_at",
    "last_turn_id",
    "transition_reason",
];
/// Payload columns from PRAGMA order: created_at, last_used_at, model_name.
const SESSION_CHAINS_PAYLOAD: &[&str] = &["created_at", "last_used_at", "model_name"];
/// Payload columns from PRAGMA order, excluding id and row_version.
const SESSION_TURNS_PAYLOAD: &[&str] = &[
    "provider_name",
    "session_id",
    "turn_id",
    "timestamp",
    "role",
    "parent_turn_id",
    "is_sidechain",
    "is_compaction_boundary",
    "source_file",
    "ingested_at",
    "body",
];
/// Payload columns from PRAGMA order: started_at, ended_at, outcome, turn_count.
const SETUP_SESSIONS_PAYLOAD: &[&str] = &["started_at", "ended_at", "outcome", "turn_count"];
/// Payload columns from PRAGMA order, excluding id and row_version.
const SETUP_TURNS_PAYLOAD: &[&str] = &[
    "session_id",
    "turn_number",
    "agent_prompt",
    "agent_response",
    "events_emitted",
    "created_at",
];

pub const REGISTRY: &[TableRegistration] = &[
    TableRegistration {
        table: "accounts",
        primary_key_columns: &["id", "provider"],
        payload_columns: ACCOUNTS_PAYLOAD,
        kind: RowKind::Mutable,
    },
    TableRegistration {
        table: "cli_providers",
        primary_key_columns: &["cli_name"],
        payload_columns: CLI_PROVIDERS_PAYLOAD,
        kind: RowKind::Mutable,
    },
    TableRegistration {
        table: "discovered_models",
        primary_key_columns: &["canonical_name", "provider"],
        payload_columns: DISCOVERED_MODELS_PAYLOAD,
        kind: RowKind::Mutable,
    },
    TableRegistration {
        table: "invocation_returned_artifacts",
        primary_key_columns: &["id"],
        payload_columns: INVOCATION_RETURNED_ARTIFACTS_PAYLOAD,
        kind: RowKind::Mutable,
    },
    TableRegistration {
        table: "invocations",
        primary_key_columns: &["id"],
        payload_columns: INVOCATIONS_PAYLOAD,
        kind: RowKind::Mutable,
    },
    TableRegistration {
        table: "memory_edges",
        primary_key_columns: &["source_id", "target_id", "edge_type"],
        payload_columns: MEMORY_EDGES_PAYLOAD,
        kind: RowKind::Mutable,
    },
    TableRegistration {
        table: "memory_nodes",
        primary_key_columns: &["id"],
        payload_columns: MEMORY_NODES_PAYLOAD,
        kind: RowKind::Mutable,
    },
    TableRegistration {
        table: "model_parameters",
        primary_key_columns: &["model_name", "provider", "name"],
        payload_columns: MODEL_PARAMETERS_PAYLOAD,
        kind: RowKind::Mutable,
    },
    TableRegistration {
        table: "provider_quota_windows",
        primary_key_columns: &["provider_name", "window_id"],
        payload_columns: PROVIDER_QUOTA_WINDOWS_PAYLOAD,
        kind: RowKind::Mutable,
    },
    TableRegistration {
        table: "provider_quotas",
        primary_key_columns: &["provider_name"],
        payload_columns: PROVIDER_QUOTAS_PAYLOAD,
        kind: RowKind::Mutable,
    },
    TableRegistration {
        table: "providers",
        primary_key_columns: &["model_name", "provider_name"],
        payload_columns: PROVIDERS_PAYLOAD,
        kind: RowKind::Mutable,
    },
    TableRegistration {
        table: "session_chain_segments",
        primary_key_columns: &["id"],
        payload_columns: SESSION_CHAIN_SEGMENTS_PAYLOAD,
        kind: RowKind::Mutable,
    },
    TableRegistration {
        table: "session_chains",
        primary_key_columns: &["chain_id"],
        payload_columns: SESSION_CHAINS_PAYLOAD,
        kind: RowKind::Mutable,
    },
    TableRegistration {
        table: "session_turns",
        primary_key_columns: &["id"],
        payload_columns: SESSION_TURNS_PAYLOAD,
        kind: RowKind::Mutable,
    },
    TableRegistration {
        table: "setup_sessions",
        primary_key_columns: &["id"],
        payload_columns: SETUP_SESSIONS_PAYLOAD,
        kind: RowKind::Mutable,
    },
    TableRegistration {
        table: "setup_turns",
        primary_key_columns: &["id"],
        payload_columns: SETUP_TURNS_PAYLOAD,
        kind: RowKind::Mutable,
    },
];

pub fn lookup(table: &str) -> Option<&'static TableRegistration> {
    REGISTRY
        .iter()
        .find(|registration| registration.table == table)
}

pub fn iter() -> impl Iterator<Item = &'static TableRegistration> {
    REGISTRY.iter()
}
