//! Role: mapper, accessor.

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TerminalClassifyError {
    kind: &'static str,
}

impl TerminalClassifyError {
    pub(crate) fn missing_capability() -> Self {
        Self {
            kind: "missing_terminal_capability",
        }
    }

    pub(crate) fn projection() -> Self {
        Self {
            kind: "projection_failed",
        }
    }

    pub(crate) fn provider_client() -> Self {
        Self {
            kind: "provider_client_failed",
        }
    }

    pub(crate) fn registry() -> Self {
        Self {
            kind: "registry_failed",
        }
    }

    pub(crate) fn kind(&self) -> &'static str {
        self.kind
    }
}
