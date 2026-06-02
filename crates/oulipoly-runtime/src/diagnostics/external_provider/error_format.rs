//! Role: formatter.

use super::errors::TerminalClassifyError;

pub(crate) fn format_terminal_classify_error(error: &TerminalClassifyError) -> String {
    format!(
        "external provider terminal classify unavailable: {}",
        error.kind()
    )
}
