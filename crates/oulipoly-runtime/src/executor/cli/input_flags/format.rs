//! ## Declared roles
//!
//! Roles: formatter.
//!
//! - formatter: emits CLI flags for explicit inputs, default values,
//!   fallback unknown-input flags, and repeated input values.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/input_flags/format.rs
//!     role: adapter
//!     Translates:
//!       - oulipoly-config-model-input-schema
//!       - toml-default-value-contract
//!       - executor-cli-argv-contract
//! ```

use super::schema_access::{default_input_defs, default_input_flag_and_value, input_def_by_name};
use oulipoly_config::{InputDef, ModelConfig};
use std::collections::HashMap;

pub(super) fn format_input_flags(
    model: &ModelConfig,
    extra_inputs: &HashMap<String, Vec<String>>,
) -> Vec<String> {
    let mut args = Vec::new();
    append_extra_input_flags(&mut args, model, extra_inputs);
    append_default_input_flags(&mut args, model, extra_inputs);
    args
}

fn append_extra_input_flags(
    args: &mut Vec<String>,
    model: &ModelConfig,
    extra_inputs: &HashMap<String, Vec<String>>,
) {
    for (key, values) in extra_inputs {
        let flag = input_def_by_name(model, key)
            .and_then(|def| def.flag.clone())
            .unwrap_or_else(|| fallback_input_flag(key));
        append_repeated_flag_values(args, &flag, values);
    }
}

fn append_default_input_flags(
    args: &mut Vec<String>,
    model: &ModelConfig,
    extra_inputs: &HashMap<String, Vec<String>>,
) {
    for input_def in default_input_defs(model, extra_inputs) {
        append_default_input_flag(args, input_def);
    }
}

fn append_default_input_flag(args: &mut Vec<String>, input_def: &InputDef) {
    if let Some((flag, default)) = default_input_flag_and_value(input_def) {
        append_default_flag_value(args, flag, default);
    }
}

fn append_default_flag_value(args: &mut Vec<String>, flag: &str, default: &toml::Value) {
    args.push(flag.to_string());
    args.push(toml_value_to_string(default));
}

fn fallback_input_flag(name: &str) -> String {
    format!("--{name}")
}

fn append_repeated_flag_values(args: &mut Vec<String>, flag: &str, values: &[String]) {
    for val in values {
        args.push(flag.to_string());
        args.push(val.clone());
    }
}

fn toml_value_to_string(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => s.clone(),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => f.to_string(),
        toml::Value::Boolean(b) => b.to_string(),
        other => other.to_string(),
    }
}
