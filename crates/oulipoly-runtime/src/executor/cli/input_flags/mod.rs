//! ## Declared roles
//!
//! Roles: orchestration.
//!
//! - orchestration: [`resolve_input_flags`] sequences input validation and
//!   command-line flag formatting for model-declared inputs.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/input_flags/mod.rs
//!     role: adapter
//!     Translates:
//!       - oulipoly-config-model-input-schema
//!       - executor-extra-inputs-contract
//!       - executor-cli-argv-contract
//! ```

mod format;
mod messages;
mod parse;
mod predicates;
mod schema_access;
mod validate;

use oulipoly_config::ModelConfig;
use std::collections::HashMap;

/// Map user-provided inputs to CLI flag arguments based on the model's input
/// schema. Orchestrates validation and formatting; preserves unknown-input
/// passthrough behavior.
pub(super) fn resolve_input_flags(
    model: &ModelConfig,
    extra_inputs: &HashMap<String, Vec<String>>,
) -> Result<Vec<String>, String> {
    validate::validate_extra_inputs(model, extra_inputs)?;
    validate::validate_required_inputs(model, extra_inputs)?;
    Ok(format::format_input_flags(model, extra_inputs))
}
