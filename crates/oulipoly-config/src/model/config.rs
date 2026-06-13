//! ## Declared roles
//!
//! - accessor
//! - formatter
//! - mapper
//! - orchestration
//! - validator
//!
//! Role set: { accessor, formatter, mapper, orchestration, validator }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-config/src/model/config.rs
//!     role: intrinsic-surface
//!     Domain: model-config-and-input-schema
//!     Owns:
//!       - ModelConfig public model schema and prompt-mode contract
//!       - InputDef and InputType typed input-schema construction and TOML formatting
//!       - ProviderImplementationRef emission subordinate to model TOML rendering
//! ```
//!
use crate::provider_implementation_ref::ProviderImplementationRef;
use serde::{Deserialize, Serialize};

use super::provider_config::ProviderConfig;
use super::raw_toml::RawInput;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub name: String,
    pub prompt_mode: PromptMode,
    pub providers: Vec<ProviderConfig>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<InputDef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderImplementationRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptMode {
    Stdin,
    Arg,
}

// --- Input schema ---

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InputType {
    String,
    Integer {
        #[serde(skip_serializing_if = "Option::is_none")]
        min: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        max: Option<i64>,
    },
    Number {
        #[serde(skip_serializing_if = "Option::is_none")]
        min: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        max: Option<f64>,
    },
    Boolean,
    Enum {
        options: Vec<String>,
    },
    Array {
        item_type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        min_items: Option<usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        max_items: Option<usize>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputDef {
    pub name: String,
    pub input_type: InputType,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub default_input: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<toml::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// CLI flag to pass this input as (e.g. "--size", "--duration")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flag: Option<String>,
}

impl ModelConfig {
    pub fn default_input(&self) -> Option<&InputDef> {
        self.inputs.iter().find(|i| i.default_input)
    }
}

pub(super) fn parse_input_type(raw: &RawInput) -> Result<InputType, String> {
    validate_raw_input_type(raw)?;
    Ok(map_raw_input_type(raw))
}

fn validate_raw_input_type(raw: &RawInput) -> Result<(), String> {
    match raw.type_name.as_str() {
        "string" | "integer" | "number" | "boolean" | "array" => Ok(()),
        "enum" => {
            if raw.options.is_none() {
                return Err(format!(
                    "Input '{}': enum type requires 'options'",
                    raw.name
                ));
            }
            Ok(())
        }
        other => Err(format!("Input '{}': unknown type '{}'", raw.name, other)),
    }
}

fn map_raw_input_type(raw: &RawInput) -> InputType {
    match raw.type_name.as_str() {
        "string" => InputType::String,
        "integer" => InputType::Integer {
            min: raw.min.map(|v| v as i64),
            max: raw.max.map(|v| v as i64),
        },
        "number" => InputType::Number {
            min: raw.min,
            max: raw.max,
        },
        "boolean" => InputType::Boolean,
        "enum" => InputType::Enum {
            options: raw.options.clone().expect("validated enum options"),
        },
        "array" => InputType::Array {
            item_type: raw
                .item_type
                .clone()
                .unwrap_or_else(|| "string".to_string()),
            min_items: raw.min_items,
            max_items: raw.max_items,
        },
        _ => unreachable!("validated input type"),
    }
}

pub(super) fn parse_inputs(raw_inputs: Vec<RawInput>) -> Result<Vec<InputDef>, String> {
    validate_single_default_input(&raw_inputs)?;
    raw_inputs
        .into_iter()
        .map(|raw| {
            let input_type = parse_input_type(&raw)?;
            Ok(map_raw_input_to_def(raw, input_type))
        })
        .collect()
}

fn validate_single_default_input(raw_inputs: &[RawInput]) -> Result<(), String> {
    let mut has_default_input = false;

    for raw in raw_inputs {
        if raw.default_input {
            if has_default_input {
                return Err(format!(
                    "Input '{}': only one input can have default_input = true",
                    raw.name
                ));
            }
            has_default_input = true;
        }
    }
    Ok(())
}

fn map_raw_input_to_def(raw: RawInput, input_type: InputType) -> InputDef {
    InputDef {
        name: raw.name,
        input_type,
        required: raw.required,
        default_input: raw.default_input,
        default: raw.default,
        description: raw.description,
        flag: raw.flag,
    }
}

impl ModelConfig {
    pub fn to_toml(&self) -> String {
        let mut out = String::new();

        let mode_str = match self.prompt_mode {
            PromptMode::Stdin => "stdin",
            PromptMode::Arg => "arg",
        };

        if let Some(provider) = self.provider.as_ref() {
            append_provider_implementation_ref(&mut out, provider);
        }

        if self.providers.len() == 1 {
            let p = &self.providers[0];
            out.push_str("[[providers]]\n");
            out.push_str(&format!("name = \"{}\"\n", p.name));
            out.push_str(&format!("args = [{}]\n", format_string_list(&p.args)));
            append_optional_string_list(
                &mut out,
                "interactive_args",
                p.interactive_args.as_deref(),
            );
        } else {
            let _ = mode_str;
            for p in &self.providers {
                out.push_str("\n[[providers]]\n");
                out.push_str(&format!("name = \"{}\"\n", p.name));
                out.push_str(&format!("args = [{}]\n", format_string_list(&p.args)));
                append_optional_string_list(
                    &mut out,
                    "interactive_args",
                    p.interactive_args.as_deref(),
                );
            }
        }

        // Inputs
        for input in &self.inputs {
            out.push('\n');
            out.push_str("[[inputs]]\n");
            out.push_str(&format!("name = \"{}\"\n", input.name));
            out.push_str(&format!(
                "type = \"{}\"\n",
                input_type_name(&input.input_type)
            ));
            if input.required {
                out.push_str("required = true\n");
            }
            if input.default_input {
                out.push_str("default_input = true\n");
            }
            if let Some(ref flag) = input.flag {
                out.push_str(&format!("flag = \"{}\"\n", flag));
            }
            if let Some(ref desc) = input.description {
                out.push_str(&format!("description = \"{}\"\n", desc));
            }
            if let Some(ref default) = input.default {
                out.push_str(&format!("default = {}\n", format_toml_value(default)));
            }

            // Type-specific fields
            match &input.input_type {
                InputType::Integer { min, max } => {
                    if let Some(v) = min {
                        out.push_str(&format!("min = {}.0\n", v));
                    }
                    if let Some(v) = max {
                        out.push_str(&format!("max = {}.0\n", v));
                    }
                }
                InputType::Number { min, max } => {
                    if let Some(v) = min {
                        out.push_str(&format!("min = {}\n", v));
                    }
                    if let Some(v) = max {
                        out.push_str(&format!("max = {}\n", v));
                    }
                }
                InputType::Enum { options } => {
                    let opts: Vec<String> = options.iter().map(|o| format!("\"{}\"", o)).collect();
                    out.push_str(&format!("options = [{}]\n", opts.join(", ")));
                }
                InputType::Array {
                    item_type,
                    min_items,
                    max_items,
                } => {
                    out.push_str(&format!("item_type = \"{}\"\n", item_type));
                    if let Some(v) = min_items {
                        out.push_str(&format!("min_items = {}\n", v));
                    }
                    if let Some(v) = max_items {
                        out.push_str(&format!("max_items = {}\n", v));
                    }
                }
                _ => {}
            }
        }

        out
    }
}

/// Render a Rust string as a TOML basic-string literal, escaping
/// backslashes, quotes, and control characters per the TOML spec.
/// Defers to the `toml` crate's serializer to avoid hand-rolling
/// escape rules that drift from the spec.
fn toml_string_literal(value: &str) -> String {
    toml::Value::String(value.to_string()).to_string()
}

fn append_optional_string_list(out: &mut String, key: &str, values: Option<&[String]>) {
    if let Some(values) = values {
        out.push_str(&format!("{key} = [{}]\n", format_string_list(values)));
    }
}

fn append_provider_implementation_ref(out: &mut String, provider: &ProviderImplementationRef) {
    let mut fields = Vec::new();
    if let Some(path) = provider.path.as_deref() {
        fields.push(format!("path = {}", toml_string_literal(path)));
    }
    if let Some(crate_name) = provider.crate_name.as_deref() {
        fields.push(format!("crate = {}", toml_string_literal(crate_name)));
    }
    if let Some(version) = provider.version.as_deref() {
        fields.push(format!("version = {}", toml_string_literal(version)));
    }
    if let Some(binary) = provider.binary.as_deref() {
        fields.push(format!("binary = {}", toml_string_literal(binary)));
    }
    if let Some(script) = provider.script.as_deref() {
        fields.push(format!("script = {}", toml_string_literal(script)));
    }
    out.push_str(&format!("provider = {{ {} }}\n\n", fields.join(", ")));
}

fn format_string_list(values: &[String]) -> String {
    values
        .iter()
        .map(|value| toml_string_literal(value))
        .collect::<Vec<_>>()
        .join(", ")
}

fn input_type_name(t: &InputType) -> &'static str {
    match t {
        InputType::String => "string",
        InputType::Integer { .. } => "integer",
        InputType::Number { .. } => "number",
        InputType::Boolean => "boolean",
        InputType::Enum { .. } => "enum",
        InputType::Array { .. } => "array",
    }
}

fn format_toml_value(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => format!("\"{}\"", s),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => format!("{}", f),
        toml::Value::Boolean(b) => b.to_string(),
        other => format!("{}", other),
    }
}
