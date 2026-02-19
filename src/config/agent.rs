use regex::Regex;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
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

static FRONTMATTER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)^---\n(.*?)\n---\n?(.*)").unwrap());

pub fn parse_agent_file(name: &str, content: &str) -> Result<AgentConfig, String> {
    let caps = FRONTMATTER_RE
        .captures(content)
        .ok_or_else(|| format!("Agent {name}: no YAML frontmatter found"))?;

    let yaml_str = &caps[1];
    let instructions = caps[2].to_string();

    let raw: RawFrontmatter = serde_yml::from_str(yaml_str)
        .map_err(|e| format!("Agent {name}: YAML parse error: {e}"))?;

    Ok(AgentConfig {
        name: name.to_string(),
        description: raw.description.unwrap_or_default(),
        model: raw.model.unwrap_or_default(),
        output_format: raw.output_format.unwrap_or_default(),
        instructions,
    })
}

pub fn load_agent_file(path: &Path) -> Result<AgentConfig, String> {
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| format!("Invalid agent filename: {}", path.display()))?;

    let content = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read agent file {}: {e}", path.display()))?;

    parse_agent_file(name, &content)
}

pub fn load_agents(agents_dir: &Path) -> Result<HashMap<String, AgentConfig>, String> {
    let mut agents = HashMap::new();

    if !agents_dir.is_dir() {
        return Ok(agents);
    }

    let entries = fs::read_dir(agents_dir)
        .map_err(|e| format!("Failed to read agents directory: {e}"))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read directory entry: {e}"))?;
        let path = entry.path();

        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }

        let agent = load_agent_file(&path)?;
        agents.insert(agent.name.clone(), agent);
    }

    Ok(agents)
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
