//! ## Declared roles
//!
//! `formatter`

use crate::provider_implementation_ref::ProviderImplementationRef;

use super::super::{InputType, ModelConfig, PromptMode};

pub(in crate::model) fn emit_model_toml(model: &ModelConfig) -> String {
    model_to_toml(model)
}

fn model_to_toml(model: &ModelConfig) -> String {
    let mut out = String::new();

    let mode_str = match model.prompt_mode {
        PromptMode::Stdin => "stdin",
        PromptMode::Arg => "arg",
    };

    if let Some(provider) = model.provider.as_ref() {
        append_provider_implementation_ref(&mut out, provider);
    }

    if model.providers.len() == 1 {
        let p = &model.providers[0];
        out.push_str("[[providers]]\n");
        out.push_str(&format!("name = \"{}\"\n", p.name));
        out.push_str(&format!("args = [{}]\n", format_string_list(&p.args)));
        append_optional_string_list(&mut out, "interactive_args", p.interactive_args.as_deref());
    } else {
        let _ = mode_str;
        for p in &model.providers {
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
    for input in &model.inputs {
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
