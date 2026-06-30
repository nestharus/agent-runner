use regex::Regex;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::LazyLock;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct AgentConfig {
    pub name: String,
    pub description: String,
    pub model: String,
    pub output_format: String,
    pub instructions: String,
}

#[derive(Deserialize)]
struct RawFrontmatter {
    description: Option<String>,
    model: Option<String>,
    output_format: Option<String>,
}

struct AgentFileParts<'a> {
    frontmatter: &'a str,
    instructions: &'a str,
}

static FRONTMATTER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)^---\n(.*?)\n---\n?(.*)").unwrap());

pub fn parse_agent_file(name: &str, content: &str) -> Result<AgentConfig, String> {
    let parts = validate_agent_file_parts(name, parse_agent_file_parts(content))?;
    let raw = parse_agent_frontmatter(parts.frontmatter)
        .map_err(|error| format_agent_yaml_parse_error(name, error))?;

    Ok(map_agent_config(name, raw, parts.instructions))
}

pub fn load_agent_file(path: &Path) -> Result<AgentConfig, String> {
    let name = validate_agent_file_name(path, agent_file_name(path))?;

    let content =
        read_agent_file(path).map_err(|error| format_agent_file_read_error(path, error))?;

    parse_agent_file(name, &content)
}

pub fn load_agents(agents_dir: &Path) -> Result<HashMap<String, AgentConfig>, String> {
    let mut agents = HashMap::new();

    if !agents_dir.is_dir() {
        return Ok(agents);
    }

    let entries = read_agents_directory(agents_dir).map_err(format_agents_directory_read_error)?;

    for entry in entries {
        let entry = validate_directory_entry(entry)?;
        let path = entry.path();

        if !is_agent_config_file(&path) {
            continue;
        }

        let (name, agent) = map_agent_config_file(&path)?;
        agents.insert(name, agent);
    }

    Ok(agents)
}

fn parse_agent_file_parts(content: &str) -> Option<AgentFileParts<'_>> {
    FRONTMATTER_RE.captures(content).map(|caps| AgentFileParts {
        frontmatter: caps
            .get(1)
            .map(|match_| match_.as_str())
            .unwrap_or_default(),
        instructions: caps
            .get(2)
            .map(|match_| match_.as_str())
            .unwrap_or_default(),
    })
}

fn validate_agent_file_parts<'a>(
    name: &str,
    parts: Option<AgentFileParts<'a>>,
) -> Result<AgentFileParts<'a>, String> {
    parts.ok_or_else(|| format_missing_frontmatter_error(name))
}

fn parse_agent_frontmatter(frontmatter: &str) -> Result<RawFrontmatter, serde_yml::Error> {
    serde_yml::from_str(frontmatter)
}

fn map_agent_config(name: &str, raw: RawFrontmatter, instructions: &str) -> AgentConfig {
    AgentConfig {
        name: name.to_string(),
        description: raw.description.unwrap_or_default(),
        model: raw.model.unwrap_or_default(),
        output_format: raw.output_format.unwrap_or_default(),
        instructions: instructions.to_string(),
    }
}

fn format_missing_frontmatter_error(name: &str) -> String {
    format!("Agent {name}: no YAML frontmatter found")
}

fn format_agent_yaml_parse_error(name: &str, error: serde_yml::Error) -> String {
    format!("Agent {name}: YAML parse error: {error}")
}

fn agent_file_name(path: &Path) -> Option<&str> {
    path.file_stem().and_then(|stem| stem.to_str())
}

fn validate_agent_file_name<'a>(path: &Path, name: Option<&'a str>) -> Result<&'a str, String> {
    name.ok_or_else(|| format_invalid_agent_filename_error(path))
}

fn read_agent_file(path: &Path) -> io::Result<String> {
    fs::read_to_string(path)
}

fn format_invalid_agent_filename_error(path: &Path) -> String {
    format!("Invalid agent filename: {}", path.display())
}

fn format_agent_file_read_error(path: &Path, error: io::Error) -> String {
    format!("Failed to read agent file {}: {error}", path.display())
}

fn read_agents_directory(agents_dir: &Path) -> io::Result<fs::ReadDir> {
    fs::read_dir(agents_dir)
}

fn validate_directory_entry(entry: io::Result<fs::DirEntry>) -> Result<fs::DirEntry, String> {
    entry.map_err(format_directory_entry_read_error)
}

fn format_agents_directory_read_error(error: io::Error) -> String {
    format!("Failed to read agents directory: {error}")
}

fn format_directory_entry_read_error(error: io::Error) -> String {
    format!("Failed to read directory entry: {error}")
}

fn is_agent_config_file(path: &Path) -> bool {
    path.extension().and_then(|extension| extension.to_str()) == Some("md")
}

fn map_agent_config_file(path: &Path) -> Result<(String, AgentConfig), String> {
    let agent = load_agent_file(path)?;

    Ok((agent.name.clone(), agent))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_frontmatter() {
        let content = "---\ndescription: 'Test agent'\nmodel: claude-opus\noutput_format: ''\n---\n\n# Instructions\n\nDo stuff.\n";
        let agent = parse_agent_file("test", content).unwrap();
        assert_eq!(agent.name, "test");
        assert_eq!(agent.description, "Test agent");
        assert_eq!(agent.model, "claude-opus");
        assert!(agent.instructions.contains("# Instructions"));
    }

    #[test]
    fn rejects_no_frontmatter() {
        let content = "# Just markdown\n\nNo frontmatter here.\n";
        let result = parse_agent_file("test", content);
        assert!(result.is_err());
    }
}
