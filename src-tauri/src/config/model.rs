use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub command: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub name: String,
    pub prompt_mode: PromptMode,
    pub providers: Vec<ProviderConfig>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<InputDef>,
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

// --- Raw TOML structures for deserialization ---

#[derive(Deserialize)]
struct RawModelToml {
    command: Option<String>,
    args: Option<Vec<String>>,
    prompt_mode: Option<String>,
    providers: Option<Vec<RawProvider>>,
    inputs: Option<Vec<RawInput>>,
}

#[derive(Deserialize)]
struct RawProvider {
    command: String,
    args: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct RawInput {
    name: String,
    #[serde(rename = "type")]
    type_name: String,
    #[serde(default)]
    required: bool,
    #[serde(default)]
    default_input: bool,
    default: Option<toml::Value>,
    description: Option<String>,
    flag: Option<String>,
    // Type-specific fields
    options: Option<Vec<String>>,
    min: Option<f64>,
    max: Option<f64>,
    item_type: Option<String>,
    min_items: Option<usize>,
    max_items: Option<usize>,
}

fn parse_prompt_mode(s: &str) -> PromptMode {
    match s {
        "arg" => PromptMode::Arg,
        _ => PromptMode::Stdin,
    }
}

fn parse_input_type(raw: &RawInput) -> Result<InputType, String> {
    match raw.type_name.as_str() {
        "string" => Ok(InputType::String),
        "integer" => Ok(InputType::Integer {
            min: raw.min.map(|v| v as i64),
            max: raw.max.map(|v| v as i64),
        }),
        "number" => Ok(InputType::Number {
            min: raw.min,
            max: raw.max,
        }),
        "boolean" => Ok(InputType::Boolean),
        "enum" => {
            let options = raw
                .options
                .clone()
                .ok_or_else(|| format!("Input '{}': enum type requires 'options'", raw.name))?;
            Ok(InputType::Enum { options })
        }
        "array" => {
            let item_type = raw
                .item_type
                .clone()
                .unwrap_or_else(|| "string".to_string());
            Ok(InputType::Array {
                item_type,
                min_items: raw.min_items,
                max_items: raw.max_items,
            })
        }
        other => Err(format!("Input '{}': unknown type '{}'", raw.name, other)),
    }
}

fn parse_inputs(raw_inputs: Vec<RawInput>) -> Result<Vec<InputDef>, String> {
    let mut inputs = Vec::new();
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

        let input_type = parse_input_type(&raw)?;
        inputs.push(InputDef {
            name: raw.name,
            input_type,
            required: raw.required,
            default_input: raw.default_input,
            default: raw.default,
            description: raw.description,
            flag: raw.flag,
        });
    }

    Ok(inputs)
}

impl ModelConfig {
    pub fn to_toml(&self) -> String {
        let mut out = String::new();

        let mode_str = match self.prompt_mode {
            PromptMode::Stdin => "stdin",
            PromptMode::Arg => "arg",
        };

        if self.providers.len() == 1 {
            let p = &self.providers[0];
            let args_toml: Vec<String> = p.args.iter().map(|a| format!("\"{}\"", a)).collect();
            out.push_str(&format!(
                "command = \"{}\"\nargs = [{}]\nprompt_mode = \"{}\"\n",
                p.command,
                args_toml.join(", "),
                mode_str
            ));
        } else {
            out.push_str(&format!("prompt_mode = \"{}\"\n", mode_str));
            for p in &self.providers {
                let args_toml: Vec<String> = p.args.iter().map(|a| format!("\"{}\"", a)).collect();
                out.push_str(&format!(
                    "\n[[providers]]\ncommand = \"{}\"\nargs = [{}]\n",
                    p.command,
                    args_toml.join(", ")
                ));
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

    pub fn from_toml(name: &str, content: &str) -> Result<Self, String> {
        let raw: RawModelToml =
            toml::from_str(content).map_err(|e| format!("TOML parse error for {name}: {e}"))?;

        let prompt_mode = parse_prompt_mode(raw.prompt_mode.as_deref().unwrap_or("stdin"));

        let inputs = if let Some(raw_inputs) = raw.inputs {
            parse_inputs(raw_inputs)?
        } else {
            vec![]
        };

        let providers = if let Some(providers) = raw.providers {
            providers
                .into_iter()
                .map(|p| ProviderConfig {
                    command: p.command,
                    args: p.args.unwrap_or_default(),
                })
                .collect()
        } else if let Some(command) = raw.command {
            vec![ProviderConfig {
                command,
                args: raw.args.unwrap_or_default(),
            }]
        } else {
            return Err(format!(
                "Model {name}: must have either 'command' or '[[providers]]'"
            ));
        };

        if providers.is_empty() {
            return Err(format!("Model {name}: no providers defined"));
        }

        Ok(ModelConfig {
            name: name.to_string(),
            prompt_mode,
            providers,
            inputs,
        })
    }
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

pub fn load_models(models_dir: &Path) -> Result<HashMap<String, ModelConfig>, String> {
    let mut models = HashMap::new();

    if !models_dir.is_dir() {
        return Ok(models);
    }

    let entries =
        fs::read_dir(models_dir).map_err(|e| format!("Failed to read models directory: {e}"))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read directory entry: {e}"))?;
        let path = entry.path();

        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }

        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| format!("Invalid filename: {}", path.display()))?
            .to_string();

        let content = fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;

        let config = ModelConfig::from_toml(&name, &content)?;
        models.insert(name, config);
    }

    Ok(models)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_single_provider() {
        let toml = r#"
command = "codex"
args = ["exec", "-m", "gpt-5.3"]
prompt_mode = "arg"
"#;
        let config = ModelConfig::from_toml("test", toml).unwrap();
        assert_eq!(config.providers.len(), 1);
        assert_eq!(config.providers[0].command, "codex");
        assert_eq!(config.providers[0].args, vec!["exec", "-m", "gpt-5.3"]);
        assert_eq!(config.prompt_mode, PromptMode::Arg);
        assert!(config.inputs.is_empty());
    }

    #[test]
    fn parse_multi_provider() {
        let toml = r#"
prompt_mode = "arg"

[[providers]]
command = "codex"
args = ["exec", "-m", "gpt-5.3-codex"]

[[providers]]
command = "codex2"
args = ["exec", "-m", "gpt-5.3-codex"]
"#;
        let config = ModelConfig::from_toml("test", toml).unwrap();
        assert_eq!(config.providers.len(), 2);
    }

    #[test]
    fn parse_defaults_to_stdin() {
        let toml = r#"
command = "claude"
args = ["-p"]
"#;
        let config = ModelConfig::from_toml("test", toml).unwrap();
        assert_eq!(config.prompt_mode, PromptMode::Stdin);
    }

    #[test]
    fn rejects_no_providers() {
        let toml = r#"
prompt_mode = "arg"
"#;
        assert!(ModelConfig::from_toml("test", toml).is_err());
    }

    #[test]
    fn roundtrip_single_provider() {
        let original = r#"
command = "codex"
args = ["exec", "-m", "gpt-5.3"]
prompt_mode = "arg"
"#;
        let c1 = ModelConfig::from_toml("test", original).unwrap();
        let c2 = ModelConfig::from_toml("test", &c1.to_toml()).unwrap();
        assert_eq!(c1.providers[0].command, c2.providers[0].command);
        assert_eq!(c1.providers[0].args, c2.providers[0].args);
        assert_eq!(c1.prompt_mode, c2.prompt_mode);
    }

    #[test]
    fn roundtrip_multi_provider() {
        let original = r#"
prompt_mode = "stdin"

[[providers]]
command = "codex"
args = ["exec", "-m", "gpt-5.3-codex"]

[[providers]]
command = "codex2"
args = ["exec", "-m", "gpt-5.3-codex"]
"#;
        let c1 = ModelConfig::from_toml("test", original).unwrap();
        let c2 = ModelConfig::from_toml("test", &c1.to_toml()).unwrap();
        assert_eq!(c1.providers.len(), c2.providers.len());
        assert_eq!(c1.providers[0].command, c2.providers[0].command);
        assert_eq!(c1.providers[1].command, c2.providers[1].command);
    }

    #[test]
    fn parse_model_with_flagged_inputs() {
        let toml = r#"
command = "atlas-image"
prompt_mode = "stdin"

[[inputs]]
name = "prompt"
type = "string"
required = true
default_input = true

[[inputs]]
name = "size"
type = "enum"
flag = "--size"
options = ["2048*2048", "1024*1024"]
default = "2048*2048"
description = "Image size"

[[inputs]]
name = "duration"
type = "integer"
flag = "--duration"
min = 5.0
max = 10.0
default = 5
"#;
        let config = ModelConfig::from_toml("test", toml).unwrap();
        assert_eq!(config.inputs.len(), 3);
        assert!(config.default_input().is_some());

        let size = &config.inputs[1];
        assert_eq!(size.flag.as_deref(), Some("--size"));
        assert!(matches!(&size.input_type, InputType::Enum { options } if options.len() == 2));

        let duration = &config.inputs[2];
        assert_eq!(duration.flag.as_deref(), Some("--duration"));
        match &duration.input_type {
            InputType::Integer { min, max } => {
                assert_eq!(*min, Some(5));
                assert_eq!(*max, Some(10));
            }
            _ => panic!("Expected integer type"),
        }
    }

    #[test]
    fn parse_model_with_array_input() {
        let toml = r#"
command = "atlas-edit"
prompt_mode = "stdin"

[[inputs]]
name = "prompt"
type = "string"
required = true
default_input = true

[[inputs]]
name = "images"
type = "array"
flag = "--image"
item_type = "string"
required = true
min_items = 1
max_items = 14
"#;
        let config = ModelConfig::from_toml("test", toml).unwrap();
        let images = &config.inputs[1];
        assert_eq!(images.flag.as_deref(), Some("--image"));
        match &images.input_type {
            InputType::Array {
                min_items,
                max_items,
                ..
            } => {
                assert_eq!(*min_items, Some(1));
                assert_eq!(*max_items, Some(14));
            }
            _ => panic!("Expected array type"),
        }
    }

    #[test]
    fn rejects_duplicate_default_input() {
        let toml = r#"
command = "test"

[[inputs]]
name = "a"
type = "string"
default_input = true

[[inputs]]
name = "b"
type = "string"
default_input = true
"#;
        let result = ModelConfig::from_toml("test", toml);
        assert!(result.unwrap_err().contains("only one input"));
    }

    #[test]
    fn rejects_enum_without_options() {
        let toml = r#"
command = "test"

[[inputs]]
name = "format"
type = "enum"
"#;
        let result = ModelConfig::from_toml("test", toml);
        assert!(result.unwrap_err().contains("requires 'options'"));
    }

    #[test]
    fn roundtrip_model_with_inputs() {
        let toml = r#"
command = "atlas-image"
prompt_mode = "stdin"

[[inputs]]
name = "prompt"
type = "string"
required = true
default_input = true

[[inputs]]
name = "size"
type = "enum"
flag = "--size"
options = ["2048*2048", "1024*1024"]
default = "2048*2048"
"#;
        let c1 = ModelConfig::from_toml("test", toml).unwrap();
        let c2 = ModelConfig::from_toml("test", &c1.to_toml()).unwrap();
        assert_eq!(c1.inputs.len(), c2.inputs.len());
        assert_eq!(c1.inputs[0].name, c2.inputs[0].name);
        assert_eq!(c1.inputs[1].flag, c2.inputs[1].flag);
    }
}
