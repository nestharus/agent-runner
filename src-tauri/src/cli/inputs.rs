//! ## Declared roles
//!
//! `parser`, `validator`, `accessor`, `formatter`, `mapper`, `predicate`, `orchestration`
//!
//! CLI prompt / stdin / answer resolution. Relocated byte-identical out of
//! `main.rs` (AGE-207, AGE-183 program slice B12); folds in the former thin
//! `cli_inputs` module's `--input` parsing.

use crate::usage::cli::Cli;
use oulipoly_config::AgentConfig;
use std::collections::{BTreeMap, HashMap};
use std::io::{IsTerminal, Read};
use std::path::Path;

/// Parse --input key=value flags into a map (repeated keys become arrays).
pub(crate) fn parse_inputs(raw: &[String]) -> Result<HashMap<String, Vec<String>>, String> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for entry in raw {
        let (key, value) = parse_key_value(entry)?;
        map.entry(key.to_string())
            .or_default()
            .push(value.to_string());
    }
    Ok(map)
}

fn parse_key_value(entry: &str) -> Result<(&str, &str), String> {
    entry
        .split_once('=')
        .ok_or_else(|| format_invalid_input_format(entry))
}

fn format_invalid_input_format(entry: &str) -> String {
    format!("Invalid input format '{}': expected KEY=VALUE", entry)
}

fn collect_positional_prompt(cli: &Cli, include_agent: bool) -> Option<String> {
    joined_prompt_parts(positional_prompt_parts(cli, include_agent))
}

fn positional_prompt_parts(cli: &Cli, include_agent: bool) -> Vec<&str> {
    let mut parts = Vec::new();
    if include_agent && let Some(ref a) = cli.agent {
        parts.push(a.as_str());
    }
    for arg in &cli.prompt_args {
        parts.push(arg.as_str());
    }
    parts
}

fn joined_prompt_parts(parts: Vec<&str>) -> Option<String> {
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

pub(crate) fn resolve_prompt(cli: &Cli, include_agent_as_prompt: bool) -> Result<String, String> {
    if let Some(ref path) = cli.file {
        return read_prompt_file(path);
    }

    if let Some(text) = collect_positional_prompt(cli, include_agent_as_prompt) {
        return Ok(text);
    }

    validate_required_prompt_stdin_available()?;
    let input = read_required_stdin_prompt()?;
    validate_nonempty_prompt_stdin(&input)?;
    Ok(input)
}

pub(crate) fn resolve_resume_answer(
    prompt: Option<&str>,
    file: Option<&Path>,
) -> Result<Option<String>, String> {
    if let Some(path) = file {
        return read_answer_file(path).map(Some);
    }
    if let Some(prompt) = prompt {
        return Ok(Some(prompt.to_string()));
    }
    if should_read_optional_stdin_answer() {
        optional_nonempty_text(read_optional_stdin_answer()?)
    } else {
        Ok(None)
    }
}

fn read_prompt_file(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(format_prompt_file_read_error)
}

fn read_answer_file(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(format_answer_file_read_error)
}

fn read_required_stdin_prompt() -> Result<String, String> {
    read_stdin_text()
}

fn read_stdin_text() -> Result<String, String> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(format_stdin_read_error)?;
    Ok(input)
}

fn validate_required_prompt_stdin_available() -> Result<(), String> {
    if std::io::stdin().is_terminal() {
        Err(format_missing_prompt_error())
    } else {
        Ok(())
    }
}

fn validate_nonempty_prompt_stdin(input: &str) -> Result<(), String> {
    if input.trim().is_empty() {
        Err(format_empty_prompt_error())
    } else {
        Ok(())
    }
}

fn format_prompt_file_read_error(error: std::io::Error) -> String {
    format!("Failed to read prompt file: {error}")
}

fn format_answer_file_read_error(error: std::io::Error) -> String {
    format!("Failed to read answer file: {error}")
}

fn format_stdin_read_error(error: std::io::Error) -> String {
    format!("Failed to read stdin: {error}")
}

fn format_missing_prompt_error() -> String {
    "No prompt provided. Pass as argument, --file, or pipe to stdin.".to_string()
}

fn format_empty_prompt_error() -> String {
    "Empty prompt from stdin.".to_string()
}

pub(crate) enum TopLevelResumePromptSource {
    Headless {
        positional_or_stdin_prompt: Option<String>,
    },
    Interactive,
}

pub(crate) fn resolve_top_level_resume_prompt_source(
    cli: &Cli,
) -> Result<TopLevelResumePromptSource, String> {
    top_level_resume_prompt_source(resume_prompt_source_parts(cli)?)
}

fn resume_prompt_source_parts(cli: &Cli) -> Result<ResumePromptSourceParts, String> {
    let prompt_text = collect_positional_prompt(cli, true);
    let stdin_prompt =
        if should_read_optional_stdin_prompt(prompt_text.is_none() && cli.file.is_none()) {
            optional_nonempty_text(read_optional_stdin_prompt()?)?
        } else {
            None
        };
    Ok(ResumePromptSourceParts {
        prompt_text,
        has_file: cli.file.is_some(),
        stdin_prompt,
    })
}

struct ResumePromptSourceParts {
    prompt_text: Option<String>,
    has_file: bool,
    stdin_prompt: Option<String>,
}

fn top_level_resume_prompt_source(
    parts: ResumePromptSourceParts,
) -> Result<TopLevelResumePromptSource, String> {
    if parts.prompt_text.is_some() || parts.has_file || parts.stdin_prompt.is_some() {
        return Ok(headless_resume_prompt_source(
            parts.prompt_text.or(parts.stdin_prompt),
        ));
    }
    Ok(interactive_resume_prompt_source())
}

fn headless_resume_prompt_source(
    positional_or_stdin_prompt: Option<String>,
) -> TopLevelResumePromptSource {
    TopLevelResumePromptSource::Headless {
        positional_or_stdin_prompt,
    }
}

fn interactive_resume_prompt_source() -> TopLevelResumePromptSource {
    TopLevelResumePromptSource::Interactive
}

fn read_optional_stdin_prompt() -> Result<String, String> {
    read_stdin_text()
}

fn read_optional_stdin_answer() -> Result<String, String> {
    read_stdin_text()
}

fn should_read_optional_stdin_prompt(enabled: bool) -> bool {
    enabled && !std::io::stdin().is_terminal()
}

fn should_read_optional_stdin_answer() -> bool {
    !std::io::stdin().is_terminal()
}

fn optional_nonempty_text(input: String) -> Result<Option<String>, String> {
    if input.trim().is_empty() {
        Ok(None)
    } else {
        Ok(Some(input))
    }
}

pub(crate) fn format_agent_prompt(agent: &AgentConfig, raw_prompt: String) -> String {
    if agent.instructions.is_empty() {
        raw_prompt
    } else {
        format!("{}\n\n{}", agent.instructions, raw_prompt)
    }
}

pub(crate) fn format_agent_prompt_with_inputs(
    agent: &AgentConfig,
    raw_prompt: String,
    inputs: &HashMap<String, Vec<String>>,
) -> Result<String, String> {
    let prompt = format_agent_prompt(agent, raw_prompt);
    if inputs.is_empty() {
        return Ok(prompt);
    }

    let input_block = render_agent_inputs(inputs)?;
    Ok(format!(
        "{prompt}\n\n## Operator Inputs\n\n```json\n{input_block}\n```"
    ))
}

fn render_agent_inputs(inputs: &HashMap<String, Vec<String>>) -> Result<String, String> {
    let ordered: BTreeMap<_, _> = inputs.iter().collect();
    serde_json::to_string_pretty(&ordered)
        .map_err(|e| format!("Failed to render agent inputs: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Characterization test for AGE-8 — pins current behavior of parse_inputs CLI adapter.
    #[test]
    fn parse_inputs_collects_repeated_keys_and_rejects_missing_separator() {
        let parsed = parse_inputs(&[
            "size=large".to_string(),
            "style=flat".to_string(),
            "size=small".to_string(),
            "empty=".to_string(),
        ])
        .unwrap();

        assert_eq!(parsed["size"], vec!["large", "small"]);
        assert_eq!(parsed["style"], vec!["flat"]);
        assert_eq!(parsed["empty"], vec![""]);

        let err = parse_inputs(&["not-key-value".to_string()]).unwrap_err();
        assert_eq!(
            err,
            "Invalid input format 'not-key-value': expected KEY=VALUE"
        );
    }

    #[test]
    fn format_agent_prompt_with_inputs_appends_sorted_json_contract_block() {
        let agent = AgentConfig {
            name: "linear-operator".to_string(),
            description: String::new(),
            model: "claude-opus".to_string(),
            output_format: String::new(),
            instructions: "# Linear Operator\n\nDo the task.".to_string(),
        };
        let mut inputs = HashMap::new();
        inputs.insert("task".to_string(), vec!["create".to_string()]);
        inputs.insert(
            "label".to_string(),
            vec!["bug".to_string(), "runtime".to_string()],
        );

        let prompt =
            format_agent_prompt_with_inputs(&agent, "User prompt".to_string(), &inputs).unwrap();

        assert!(prompt.starts_with("# Linear Operator\n\nDo the task.\n\nUser prompt"));
        assert!(prompt.contains("## Operator Inputs\n\n```json\n"));
        assert!(
            prompt.contains(
                "\"label\": [\n    \"bug\",\n    \"runtime\"\n  ],\n  \"task\": [\n    \"create\"\n  ]"
            ),
            "{prompt}"
        );
        assert!(prompt.ends_with("\n```"));
    }
}
