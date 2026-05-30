//! ## Declared roles
//!
//! Roles: accessor, filter.
//!
//! - accessor: reads model input definitions and default flag/value pairs for
//!   validators and formatters.
//! - filter: selects only schema definitions that should emit default flags.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/input_flags/schema_access.rs
//!     role: adapter
//!     Translates:
//!       - oulipoly-config-model-input-schema
//!       - toml-default-value-contract
//! ```

use super::predicates::default_input_should_be_appended;
use oulipoly_config::{InputDef, ModelConfig};
use std::collections::HashMap;

pub(super) fn input_def_by_name<'a>(model: &'a ModelConfig, name: &str) -> Option<&'a InputDef> {
    model.inputs.iter().find(|input| input.name == name)
}

pub(super) fn default_input_defs<'a>(
    model: &'a ModelConfig,
    extra_inputs: &HashMap<String, Vec<String>>,
) -> Vec<&'a InputDef> {
    model
        .inputs
        .iter()
        .filter(|input_def| default_input_should_be_appended(input_def, extra_inputs))
        .collect()
}

pub(super) fn default_input_flag_and_value(input_def: &InputDef) -> Option<(&str, &toml::Value)> {
    input_def.flag.as_deref().zip(input_def.default.as_ref())
}
