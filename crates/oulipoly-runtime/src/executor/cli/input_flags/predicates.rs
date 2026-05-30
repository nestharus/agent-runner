//! ## Declared roles
//!
//! Roles: predicate.
//!
//! - predicate: answers required-input and default-emission decisions for the
//!   input schema flow.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/input_flags/predicates.rs
//!     role: adapter
//!     Translates:
//!       - oulipoly-config-model-input-schema
//!       - executor-extra-inputs-contract
//! ```

use oulipoly_config::InputDef;
use std::collections::HashMap;

pub(super) fn input_requires_user_value(
    input_def: &InputDef,
    extra_inputs: &HashMap<String, Vec<String>>,
) -> bool {
    !input_def.default_input
        && !extra_inputs.contains_key(&input_def.name)
        && input_def.default.is_none()
        && input_def.required
}

pub(super) fn default_input_should_be_appended(
    input_def: &InputDef,
    extra_inputs: &HashMap<String, Vec<String>>,
) -> bool {
    !input_def.default_input && !extra_inputs.contains_key(&input_def.name)
}
