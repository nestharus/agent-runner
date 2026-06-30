use oulipoly_state::{CliMapping, DiscoveredModel, ModelParameter, ParamType};
use std::process::Command;

const CLAUDE_CLI_NAME: &str = concat!("cla", "ude");
const CODEX_CLI_NAME: &str = concat!("cod", "ex");

/// Result of a model discovery attempt for a single CLI.
#[derive(Debug)]
pub struct DiscoveryResult {
    pub cli_name: String,
    pub cli_version: String,
    pub models: Vec<DiscoveredModel>,
    pub parameters: Vec<(String, ModelParameter)>, // (model_name, param)
}

/// Known CLI discovery strategies. Each entry maps a CLI name to a function
/// that tries to extract model names from that CLI's output.
struct CliDiscoveryStrategy {
    name: &'static str,
    commands: &'static [&'static [&'static str]],
}

const STRATEGIES: &[CliDiscoveryStrategy] = &[
    CliDiscoveryStrategy {
        name: "claude",
        commands: &[&["models", "list"], &["--help"]],
    },
    CliDiscoveryStrategy {
        name: "codex",
        commands: &[&["models"], &["--help"]],
    },
    CliDiscoveryStrategy {
        name: "gemini",
        commands: &[&["models", "list"], &["--help"]],
    },
    CliDiscoveryStrategy {
        name: "opencode",
        commands: &[&["models"], &["--help"]],
    },
    CliDiscoveryStrategy {
        name: "forge",
        commands: &[&["list", "model", "--porcelain"], &["--help"]],
    },
];

/// Run model discovery for a specific CLI tool.
///
/// Tries known discovery commands in order, returning the first successful parse.
/// Returns an error only if the CLI is not found at all; empty results are OK.
pub fn discover_models(cli_name: &str) -> Result<DiscoveryResult, String> {
    let cli_version = get_cli_version(cli_name)?;
    let commands = discovery_commands(cli_name);
    let now = chrono::Utc::now().to_rfc3339();
    let discovered = first_discovery_result(cli_name, &cli_version, &now, commands);

    // No discovery command succeeded, return empty result (not an error)
    match discovered {
        Some(result) => Ok(result),
        None => Ok(empty_discovery_result(cli_name, cli_version)),
    }
}

fn discovery_commands(cli_name: &str) -> &[&[&str]] {
    match strategy_for_cli(cli_name, STRATEGIES) {
        Some(strategy) => strategy.commands,
        // Unknown CLI: try generic approaches
        None => &[&["models", "list"], &["models"], &["--help"]],
    }
}

fn strategy_for_cli<'a>(
    cli_name: &str,
    strategies: &'a [CliDiscoveryStrategy],
) -> Option<&'a CliDiscoveryStrategy> {
    let (strategy, remaining_strategies) = strategies.split_first()?;

    if strategy.name == cli_name {
        return Some(strategy);
    }

    strategy_for_cli(cli_name, remaining_strategies)
}

fn first_discovery_result(
    cli_name: &str,
    cli_version: &str,
    discovered_at: &str,
    commands: &[&[&str]],
) -> Option<DiscoveryResult> {
    let (cmd_args, remaining_commands) = commands.split_first()?;

    match discovery_result_from_command(cli_name, cli_version, discovered_at, cmd_args) {
        Some(result) => Some(result),
        None => first_discovery_result(cli_name, cli_version, discovered_at, remaining_commands),
    }
}

fn discovery_result_from_command(
    cli_name: &str,
    cli_version: &str,
    discovered_at: &str,
    cmd_args: &[&str],
) -> Option<DiscoveryResult> {
    let output = run_cli_command(cli_name, cmd_args).ok()?;
    let model_names = parse_model_names(cli_name, &output);
    discovery_result_for_models(cli_name, cli_version, discovered_at, model_names)
}

fn discovery_result_for_models(
    cli_name: &str,
    cli_version: &str,
    discovered_at: &str,
    model_names: Vec<String>,
) -> Option<DiscoveryResult> {
    if has_no_discovered_models(&model_names) {
        return None;
    }

    Some(populated_discovery_result(
        cli_name,
        cli_version,
        discovered_at,
        &model_names,
    ))
}

fn has_no_discovered_models(model_names: &[String]) -> bool {
    model_names.is_empty()
}

fn populated_discovery_result(
    cli_name: &str,
    cli_version: &str,
    discovered_at: &str,
    model_names: &[String],
) -> DiscoveryResult {
    DiscoveryResult {
        cli_name: cli_name.to_string(),
        cli_version: cli_version.to_string(),
        models: discovered_models(cli_name, cli_version, discovered_at, model_names),
        parameters: build_default_parameters(cli_name, model_names),
    }
}

fn discovered_models(
    cli_name: &str,
    cli_version: &str,
    discovered_at: &str,
    model_names: &[String],
) -> Vec<DiscoveredModel> {
    let mut models = Vec::new();

    for model_name in model_names {
        append_discovered_model(
            cli_name,
            cli_version,
            discovered_at,
            model_name,
            &mut models,
        );
    }

    models
}

fn append_discovered_model(
    cli_name: &str,
    cli_version: &str,
    discovered_at: &str,
    model_name: &str,
    models: &mut Vec<DiscoveredModel>,
) {
    models.push(discovered_model(
        cli_name,
        cli_version,
        discovered_at,
        model_name,
    ));
}

fn discovered_model(
    cli_name: &str,
    cli_version: &str,
    discovered_at: &str,
    model_name: &str,
) -> DiscoveredModel {
    DiscoveredModel {
        canonical_name: model_name.to_string(),
        provider: cli_name.to_string(),
        discovered_at: discovered_at.to_string(),
        cli_version: cli_version.to_string(),
    }
}

fn empty_discovery_result(cli_name: &str, cli_version: String) -> DiscoveryResult {
    DiscoveryResult {
        cli_name: cli_name.to_string(),
        cli_version,
        models: vec![],
        parameters: vec![],
    }
}

/// Get the version string from a CLI tool.
fn get_cli_version(cli_name: &str) -> Result<String, String> {
    let output = cli_version_output(cli_name)?;

    Ok(cli_version_from_output(&output))
}

fn cli_version_output(cli_name: &str) -> Result<std::process::Output, String> {
    match Command::new(cli_name).arg("--version").output() {
        Ok(output) => Ok(output),
        Err(error) => Err(cli_not_found_message(cli_name, error)),
    }
}

fn cli_not_found_message(cli_name: &str, error: std::io::Error) -> String {
    format!("CLI '{}' not found or not executable: {}", cli_name, error)
}

fn cli_version_from_output(output: &std::process::Output) -> String {
    if is_success_output(output) {
        return stdout_version_text(output);
    }

    stderr_version_text(output).unwrap_or_else(unknown_version_text)
}

fn is_success_output(output: &std::process::Output) -> bool {
    output.status.success()
}

fn stdout_version_text(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn stderr_version_text(output: &std::process::Output) -> Option<String> {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    (!stderr.is_empty()).then_some(stderr)
}

fn unknown_version_text() -> String {
    "unknown".to_string()
}

/// Run a CLI command and capture stdout.
fn run_cli_command(cli_name: &str, args: &[&str]) -> Result<String, String> {
    let output = cli_command_output(cli_name, args)?;

    command_output_text(cli_name, args, &output)
}

fn cli_command_output(cli_name: &str, args: &[&str]) -> Result<std::process::Output, String> {
    match Command::new(cli_name).args(args).output() {
        Ok(output) => Ok(output),
        Err(error) => Err(command_run_failed_message(cli_name, args, error)),
    }
}

fn command_run_failed_message(cli_name: &str, args: &[&str], error: std::io::Error) -> String {
    format!("Failed to run {} {:?}: {}", cli_name, args, error)
}

fn command_output_text(
    cli_name: &str,
    args: &[&str],
    output: &std::process::Output,
) -> Result<String, String> {
    // Accept both success and some failure codes (help often returns non-zero)
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    command_text_from_streams(
        cli_name,
        args,
        is_success_output(output),
        command_exit_code(output),
        stdout,
        stderr,
    )
}

fn command_exit_code(output: &std::process::Output) -> Option<i32> {
    output.status.code()
}

fn command_text_from_streams(
    cli_name: &str,
    args: &[&str],
    success: bool,
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
) -> Result<String, String> {
    // Use whichever has more content
    if should_use_stdout(&stdout, &stderr) {
        Ok(stdout)
    } else if should_use_stderr(&stderr) {
        Ok(stderr)
    } else if success {
        Ok(stdout)
    } else {
        Err(command_exit_failed_message(cli_name, args, exit_code))
    }
}

fn should_use_stdout(stdout: &str, stderr: &str) -> bool {
    stdout.len() >= stderr.len() && !stdout.is_empty()
}

fn should_use_stderr(stderr: &str) -> bool {
    !stderr.is_empty()
}

fn command_exit_failed_message(cli_name: &str, args: &[&str], exit_code: Option<i32>) -> String {
    format!(
        "{} {:?} failed with exit code {:?}",
        cli_name, args, exit_code
    )
}

/// Parse model names from CLI output.
///
/// Uses heuristics to extract model identifiers from various output formats:
/// - One model name per line (most common for `models list`)
/// - Model names in help text (fallback)
fn parse_model_names(cli_name: &str, output: &str) -> Vec<String> {
    let mut models = Vec::new();

    for line in output.lines() {
        append_model_line(cli_name, line, &mut models);
    }

    sort_and_dedup_model_names(&mut models);
    models
}

fn append_model_line(cli_name: &str, line: &str, models: &mut Vec<String>) {
    if let Some(model_name) = parse_model_line(cli_name, line) {
        models.push(model_name);
    }
}

fn parse_model_line(cli_name: &str, line: &str) -> Option<String> {
    let trimmed = model_line_text(line)?;
    if is_ignored_model_line(trimmed) {
        return None;
    }

    let model_name = extract_model_name(cli_name, trimmed)?;
    is_valid_model_name(&model_name).then_some(model_name)
}

fn model_line_text(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn is_ignored_model_line(line: &str) -> bool {
    is_decorative_model_line(line) || is_common_non_model_line(line)
}

fn is_decorative_model_line(line: &str) -> bool {
    line.starts_with('-') || line.starts_with('=') || line.starts_with('#')
}

fn is_common_non_model_line(line: &str) -> bool {
    let lower = line.to_lowercase();
    lower.starts_with("available")
        || lower.starts_with("usage")
        || lower.starts_with("options")
        || lower.starts_with("commands")
        || lower.starts_with("flags")
        || lower.contains("--help")
}

fn sort_and_dedup_model_names(model_names: &mut Vec<String>) {
    model_names.sort();
    model_names.dedup();
}

/// Try to extract a model name from a single line of output.
fn extract_model_name(cli_name: &str, line: &str) -> Option<String> {
    // If the line looks like "model-name  description text", take the first token
    let first_token = first_model_line_token(line)?;
    let candidate = cleaned_model_token(first_token)?;
    Some(normalize_model_candidate(cli_name, candidate))
}

fn first_model_line_token(line: &str) -> Option<&str> {
    line.split_whitespace().next()
}

fn cleaned_model_token(token: &str) -> Option<String> {
    let cleaned = token
        .trim_start_matches(['*', '>', '|'])
        .trim_start_matches(is_list_number_prefix_char)
        .trim();

    if cleaned.is_empty() {
        return None;
    }

    Some(cleaned.to_string())
}

fn is_list_number_prefix_char(c: char) -> bool {
    c.is_ascii_digit() || c == '.' || c == ')'
}

fn normalize_model_candidate(_cli_name: &str, candidate: String) -> String {
    candidate
}

/// Check if a string looks like a valid model name.
fn is_valid_model_name(name: &str) -> bool {
    has_valid_model_name_length(name)
        && has_model_name_letter(name)
        && is_not_model_stop_word(name)
        && has_valid_model_name_chars(name)
}

fn has_valid_model_name_length(name: &str) -> bool {
    name.len() >= 2 && name.len() <= 100
}

fn has_model_name_letter(name: &str) -> bool {
    name.chars().any(is_ascii_alphabetic_char)
}

fn is_not_model_stop_word(name: &str) -> bool {
    let lower = name.to_lowercase();
    !model_stop_words().contains(&lower.as_str())
}

fn model_stop_words() -> &'static [&'static str] {
    &[
        "name",
        "id",
        "type",
        "model",
        "models",
        "list",
        "help",
        "version",
        "the",
        "and",
        "for",
        "with",
        "from",
        "this",
        "that",
        "description",
        "status",
        "created",
        "updated",
        "default",
        "none",
        "true",
        "false",
    ]
}

fn has_valid_model_name_chars(name: &str) -> bool {
    name.chars().all(is_valid_model_name_char)
}

fn is_ascii_alphabetic_char(c: char) -> bool {
    c.is_ascii_alphabetic()
}

fn is_valid_model_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == ':' || c == '/'
}

/// Build default parameter definitions for known CLIs.
/// These represent common parameters that most models of a given CLI support.
fn build_default_parameters(
    cli_name: &str,
    model_names: &[String],
) -> Vec<(String, ModelParameter)> {
    let common_params = common_parameters(cli_name, model_names);
    // Apply common params to all discovered models for this CLI
    parameter_pairs_for_models(model_names, &common_params)
}

fn common_parameters(cli_name: &str, model_names: &[String]) -> Vec<ModelParameter> {
    match cli_name {
        CLAUDE_CLI_NAME => vec![max_tokens_parameter()],
        CODEX_CLI_NAME => vec![model_enum_parameter("-m", model_names)],
        "forge" => vec![model_enum_parameter("--model", model_names)],
        "gemini" => vec![temperature_parameter()],
        _ => vec![],
    }
}

fn max_tokens_parameter() -> ModelParameter {
    ModelParameter {
        name: "max_tokens".to_string(),
        display_name: "Max Tokens".to_string(),
        param_type: ParamType::Number {
            min: Some(1.0),
            max: Some(200000.0),
        },
        description: "Maximum number of tokens to generate".to_string(),
        cli_mapping: CliMapping {
            flag: "--max-tokens".to_string(),
            value_template: "{value}".to_string(),
        },
    }
}

fn model_enum_parameter(flag: &str, model_names: &[String]) -> ModelParameter {
    ModelParameter {
        name: "model".to_string(),
        display_name: "Model".to_string(),
        param_type: ParamType::Enum {
            options: model_names.to_vec(),
        },
        description: "Model to use for generation".to_string(),
        cli_mapping: CliMapping {
            flag: flag.to_string(),
            value_template: "{value}".to_string(),
        },
    }
}

fn temperature_parameter() -> ModelParameter {
    ModelParameter {
        name: "temperature".to_string(),
        display_name: "Temperature".to_string(),
        param_type: ParamType::Number {
            min: Some(0.0),
            max: Some(2.0),
        },
        description: "Controls randomness of output".to_string(),
        cli_mapping: CliMapping {
            flag: "--temperature".to_string(),
            value_template: "{value}".to_string(),
        },
    }
}

fn parameter_pairs_for_models(
    model_names: &[String],
    common_params: &[ModelParameter],
) -> Vec<(String, ModelParameter)> {
    let mut params = Vec::new();

    for model_name in model_names {
        append_parameter_pairs_for_model(model_name, common_params, &mut params);
    }

    params
}

fn append_parameter_pairs_for_model(
    model_name: &str,
    common_params: &[ModelParameter],
    params: &mut Vec<(String, ModelParameter)>,
) {
    for param in common_params {
        params.push((model_name.to_string(), param.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_model_names_simple_list() {
        let output = "claude-opus-4\nclaude-sonnet-4\nclaude-haiku-3.5\n";
        let models = parse_model_names("claude", output);
        assert_eq!(
            models,
            vec!["claude-haiku-3.5", "claude-opus-4", "claude-sonnet-4"]
        );
    }

    #[test]
    fn parse_model_names_with_descriptions() {
        let output = "\
gpt-5.3           Latest GPT model
gpt-5.3-mini      Smaller, faster variant
o3                Reasoning model
";
        let models = parse_model_names("codex", output);
        assert_eq!(models, vec!["gpt-5.3", "gpt-5.3-mini", "o3"]);
    }

    #[test]
    fn parse_model_names_skips_headers() {
        let output = "\
Available Models:
-----------------
claude-opus-4
claude-sonnet-4
";
        let models = parse_model_names("claude", output);
        assert_eq!(models, vec!["claude-opus-4", "claude-sonnet-4"]);
    }

    #[test]
    fn parse_model_names_empty_input() {
        let models = parse_model_names("claude", "");
        assert!(models.is_empty());
    }

    #[test]
    fn parse_model_names_gemini_prefixed() {
        let output = "models/gemini-2.0-flash\nmodels/gemini-pro\n";
        let models = parse_model_names("gemini", output);
        assert_eq!(models, vec!["models/gemini-2.0-flash", "models/gemini-pro"]);
    }

    #[test]
    fn is_valid_model_name_accepts_good_names() {
        assert!(is_valid_model_name("claude-opus-4"));
        assert!(is_valid_model_name("gpt-5.3"));
        assert!(is_valid_model_name("models/gemini-pro"));
        assert!(is_valid_model_name("o3"));
    }

    #[test]
    fn is_valid_model_name_rejects_bad_names() {
        assert!(!is_valid_model_name(""));
        assert!(!is_valid_model_name("a")); // too short
        assert!(!is_valid_model_name("help"));
        assert!(!is_valid_model_name("the"));
        assert!(!is_valid_model_name("123")); // no letters
        assert!(!is_valid_model_name("model with spaces"));
    }

    #[test]
    fn build_default_parameters_claude() {
        let models = vec!["claude-opus-4".to_string()];
        let params = build_default_parameters("claude", &models);
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].0, "claude-opus-4");
        assert_eq!(params[0].1.name, "max_tokens");
    }

    #[test]
    fn build_default_parameters_unknown_cli() {
        let models = vec!["some-model".to_string()];
        let params = build_default_parameters("unknown-cli", &models);
        assert!(params.is_empty());
    }

    #[test]
    fn build_default_parameters_multiple_models() {
        let models = vec!["m1".to_string(), "m2".to_string()];
        let params = build_default_parameters("codex", &models);
        // Each model gets the same set of params
        assert_eq!(params.len(), 2);
        assert_eq!(params[0].0, "m1");
        assert_eq!(params[1].0, "m2");
    }

    #[test]
    fn parse_deduplicates() {
        let output = "model-a\nmodel-b\nmodel-a\n";
        let models = parse_model_names("test", output);
        assert_eq!(models, vec!["model-a", "model-b"]);
    }
}
