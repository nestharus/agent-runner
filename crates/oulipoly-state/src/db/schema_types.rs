#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProviderSessionProjection {
    DualId,
    LegacySessionId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InvocationDualIdProjection {
    Current,
    CurrentWithoutResolvedAccount,
    Legacy,
}

impl InvocationDualIdProjection {
    pub(super) fn select_columns(
        &self,
    ) -> (&'static str, &'static str, &'static str, &'static str) {
        match self {
            InvocationDualIdProjection::Current => (
                "provider_session_id",
                "resume_input_id",
                "provider_session_capture_method",
                "provider_session_resolved_account",
            ),
            InvocationDualIdProjection::CurrentWithoutResolvedAccount => (
                "provider_session_id",
                "resume_input_id",
                "provider_session_capture_method",
                "NULL AS provider_session_resolved_account",
            ),
            InvocationDualIdProjection::Legacy => (
                "NULL AS provider_session_id",
                "NULL AS resume_input_id",
                "NULL AS provider_session_capture_method",
                "NULL AS provider_session_resolved_account",
            ),
        }
    }
}

pub(super) enum InvocationsSchemaShape {
    Empty,
    Current,
    LegacyPreUuid,
    UnrecognizedPreUuid(Vec<String>),
}

pub(super) enum ProvidersSchemaShape {
    Empty,
    Current,
    LegacyIndexKeyed,
    Unexpected(String),
}

pub(super) struct ColumnRepair {
    pub(super) column_name: &'static str,
    pub(super) sql: &'static str,
    pub(super) error_context: &'static str,
}

pub(super) struct DropColumnRepair {
    pub(super) column_name: &'static str,
    pub(super) sql: &'static str,
    pub(super) error_context: &'static str,
}
