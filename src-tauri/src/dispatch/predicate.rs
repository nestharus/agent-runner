//! Declared roles: predicate, accessor

use oulipoly_config::ModelConfig;
use std::collections::HashMap;

pub(crate) fn diagnostics_model_configured(models: &HashMap<String, ModelConfig>) -> bool {
    agent_runner_lib::load_app_config()
        .diagnostics_model
        .as_ref()
        .is_some_and(|name| models.contains_key(name))
}

/// The short `[resume] -> <provider>` line is always emitted regardless of
/// TTY (per proposal §5: V10 wins over V15 here — even at a terminal, the
/// runner's selection must be visible). Factored as a helper so the
/// "always-on" semantic has an explicit, unit-testable surface that mirrors
/// `should_emit_invocation_line`.
pub(crate) fn should_emit_resume_short_line(_is_terminal: bool) -> bool {
    true
}

pub(crate) fn execution_succeeded(exit_code: i32) -> bool {
    exit_code == 0
}
