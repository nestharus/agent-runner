use std::path::PathBuf;

pub struct CliPaths {
    pub skills_dir: Option<PathBuf>,
    pub mcp_config: Option<PathBuf>,
    pub plugins_dir: Option<PathBuf>,
}

#[derive(Clone)]
pub struct Extension {
    pub name: String,
    pub ext_type: ExtensionType,
    pub source_cli: String,
    pub installed_in: Vec<String>,
}

#[derive(Clone, PartialEq)]
pub enum ExtensionType {
    Skill,
    Mcp,
    Plugin,
}

pub fn resolve_cli_paths(cli_name: &str) -> CliPaths {
    let home = dirs::home_dir().unwrap_or_default();

    match cli_name {
        "claude" => CliPaths {
            skills_dir: Some(home.join(".claude").join("skills")),
            mcp_config: Some(home.join(".claude").join(".claude.json")),
            plugins_dir: None,
        },
        "codex" => CliPaths {
            skills_dir: Some(home.join(".codex").join("skills")),
            mcp_config: Some(home.join(".codex").join("config.toml")),
            plugins_dir: None,
        },
        "opencode" => CliPaths {
            skills_dir: None,
            mcp_config: Some(home.join(".opencode").join("config.json")),
            plugins_dir: None,
        },
        _ => CliPaths {
            skills_dir: None,
            mcp_config: None,
            plugins_dir: None,
        },
    }
}

pub fn copy_skill(source_cli: &str, target_cli: &str, skill_name: &str) -> Result<(), String> {
    let source = resolve_cli_paths(source_cli)
        .skills_dir
        .ok_or("Source CLI has no skills directory")?
        .join(skill_name);

    let target = resolve_cli_paths(target_cli)
        .skills_dir
        .ok_or("Target CLI has no skills directory")?
        .join(skill_name);

    if !source.exists() {
        return Err(format!("Skill '{}' not found in {}", skill_name, source_cli));
    }

    copy_dir_recursive(&source, &target)
}

pub fn install_mcp(target_cli: &str, mcp_name: &str, config_json: &str) -> Result<(), String> {
    let paths = resolve_cli_paths(target_cli);
    let config_path = paths
        .mcp_config
        .ok_or(format!("No MCP config path for {target_cli}"))?;

    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create config directory: {e}"))?;
    }

    let ext = config_path.extension().and_then(|e| e.to_str()).unwrap_or("");

    match ext {
        "json" => install_mcp_json(&config_path, mcp_name, config_json),
        "toml" => install_mcp_toml(&config_path, mcp_name, config_json),
        _ => Err(format!("Unknown config format: {ext}")),
    }
}

fn install_mcp_json(config_path: &PathBuf, mcp_name: &str, config_json: &str) -> Result<(), String> {
    let existing = if config_path.exists() {
        let content = std::fs::read_to_string(config_path)
            .map_err(|e| format!("Failed to read config: {e}"))?;
        serde_json::from_str::<serde_json::Value>(&content)
            .map_err(|e| format!("Failed to parse config JSON: {e}"))?
    } else {
        serde_json::json!({})
    };

    let mut config = existing;
    let mcp_value: serde_json::Value = serde_json::from_str(config_json)
        .map_err(|e| format!("Failed to parse MCP config: {e}"))?;

    if let Some(obj) = config.as_object_mut() {
        let mcps = obj.entry("mcpServers").or_insert_with(|| serde_json::json!({}));
        if let Some(mcps_obj) = mcps.as_object_mut() {
            mcps_obj.insert(mcp_name.to_string(), mcp_value);
        }
    }

    let output = serde_json::to_string_pretty(&config)
        .map_err(|e| format!("Failed to serialize config: {e}"))?;
    std::fs::write(config_path, output)
        .map_err(|e| format!("Failed to write config: {e}"))?;

    Ok(())
}

fn install_mcp_toml(config_path: &PathBuf, mcp_name: &str, config_json: &str) -> Result<(), String> {
    let existing = if config_path.exists() {
        let content = std::fs::read_to_string(config_path)
            .map_err(|e| format!("Failed to read config: {e}"))?;
        content.parse::<toml::Table>()
            .map_err(|e| format!("Failed to parse TOML: {e}"))?
    } else {
        toml::Table::new()
    };

    let mut config = existing;
    let mcp_table = config.entry("mcp").or_insert_with(|| toml::Value::Table(toml::Table::new()));

    if let Some(mcp_obj) = mcp_table.as_table_mut() {
        // Store the config JSON as a string value under the MCP name
        mcp_obj.insert(
            mcp_name.to_string(),
            toml::Value::String(config_json.to_string()),
        );
    }

    let output = toml::to_string_pretty(&config)
        .map_err(|e| format!("Failed to serialize TOML: {e}"))?;
    std::fs::write(config_path, output)
        .map_err(|e| format!("Failed to write config: {e}"))?;

    Ok(())
}

pub fn discover_extensions(clis: &[super::detection::CliInfo]) -> Vec<Extension> {
    let mut extensions = Vec::new();

    for cli in clis {
        if !cli.installed {
            continue;
        }

        let paths = resolve_cli_paths(&cli.name);

        // Discover skills
        if let Some(ref skills_dir) = paths.skills_dir {
            if skills_dir.is_dir() {
                if let Ok(entries) = std::fs::read_dir(skills_dir) {
                    for entry in entries.flatten() {
                        if entry.path().is_dir() {
                            let name = entry.file_name().to_string_lossy().to_string();
                            // Check if already tracked
                            if let Some(existing) = extensions.iter_mut().find(|e: &&mut Extension| {
                                e.name == name && e.ext_type == ExtensionType::Skill
                            }) {
                                existing.installed_in.push(cli.name.clone());
                            } else {
                                extensions.push(Extension {
                                    name,
                                    ext_type: ExtensionType::Skill,
                                    source_cli: cli.name.clone(),
                                    installed_in: vec![cli.name.clone()],
                                });
                            }
                        }
                    }
                }
            }
        }

        // Discover MCPs from config
        if let Some(ref mcp_config) = paths.mcp_config {
            if mcp_config.exists() {
                if let Ok(content) = std::fs::read_to_string(mcp_config) {
                    let mcp_names = extract_mcp_names(&content, mcp_config);
                    for name in mcp_names {
                        if let Some(existing) = extensions.iter_mut().find(|e: &&mut Extension| {
                            e.name == name && e.ext_type == ExtensionType::Mcp
                        }) {
                            existing.installed_in.push(cli.name.clone());
                        } else {
                            extensions.push(Extension {
                                name,
                                ext_type: ExtensionType::Mcp,
                                source_cli: cli.name.clone(),
                                installed_in: vec![cli.name.clone()],
                            });
                        }
                    }
                }
            }
        }
    }

    extensions
}

fn extract_mcp_names(content: &str, path: &PathBuf) -> Vec<String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext {
        "json" => {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(content) {
                if let Some(servers) = val.get("mcpServers").and_then(|s| s.as_object()) {
                    return servers.keys().cloned().collect();
                }
            }
            vec![]
        }
        "toml" => {
            if let Ok(table) = content.parse::<toml::Table>() {
                if let Some(mcp) = table.get("mcp").and_then(|m| m.as_table()) {
                    return mcp.keys().cloned().collect();
                }
            }
            vec![]
        }
        _ => vec![],
    }
}

fn copy_dir_recursive(src: &PathBuf, dst: &PathBuf) -> Result<(), String> {
    if !src.is_dir() {
        return Err(format!("Source is not a directory: {}", src.display()));
    }

    std::fs::create_dir_all(dst)
        .map_err(|e| format!("Failed to create directory {}: {e}", dst.display()))?;

    let entries = std::fs::read_dir(src)
        .map_err(|e| format!("Failed to read directory {}: {e}", src.display()))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read entry: {e}"))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)
                .map_err(|e| format!("Failed to copy {}: {e}", src_path.display()))?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_claude_paths() {
        let paths = resolve_cli_paths("claude");
        assert!(paths.skills_dir.is_some());
        assert!(paths.mcp_config.is_some());
    }

    #[test]
    fn resolve_unknown_cli() {
        let paths = resolve_cli_paths("unknown");
        assert!(paths.skills_dir.is_none());
        assert!(paths.mcp_config.is_none());
    }

    #[test]
    fn extract_json_mcp_names() {
        let json = r#"{"mcpServers": {"firecrawl": {}, "github": {}}}"#;
        let names = extract_mcp_names(json, &PathBuf::from("config.json"));
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"firecrawl".to_string()));
    }
}
