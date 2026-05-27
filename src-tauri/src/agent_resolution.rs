//! Agent lookup / load / resolution.
//!
//! Relocated from `src-tauri/src/main.rs` by AGE-206 (slice B11 of the AGE-183 main.rs
//! decomposition program; map row H2). Output-preserving: bodies byte-identical to the
//! pre-AGE-206 main.rs definitions; only the `resolve_agent` entry point gains `pub(crate)`
//! visibility and the `resolve_agents_dir` reference is imported via `crate::`. Given the parsed
//! CLI, this module resolves the selected agent — an explicit `--agent-file` bypass, a named-agent
//! directory lookup, or the missing-agent error — and maps the unknown/missing cases to their
//! stable error strings.
//!
//! ## Declared roles
//!
//! `orchestration`, `accessor`, `formatter`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/agent_resolution.rs
//!     role: adapter
//!     Translates:
//!       - AGE-206 H2: CLI agent selection (--agent-file | --agent | none) -> AgentConfig via AgentConfigRepository
//!       - AGE-206 H2: unknown/missing agent selection -> stable error strings
//! ```

use crate::cli::paths::resolve_agents_dir;
use crate::usage::cli::Cli;
use oulipoly_config::AgentConfig;
use oulipoly_config::repositories::AgentConfigRepository;
use std::path::Path;

pub(crate) fn resolve_agent(
    cli: &Cli,
    agent_config: &dyn AgentConfigRepository,
) -> Result<AgentConfig, String> {
    if let Some(ref path) = cli.agent_file {
        return load_agent_by_path(agent_config, path);
    }

    if let Some(ref name) = cli.agent {
        return lookup_agent_by_name(cli, agent_config, name);
    }

    Err(format_missing_agent_error())
}

fn load_agent_by_path(
    agent_config: &dyn AgentConfigRepository,
    path: &Path,
) -> Result<AgentConfig, String> {
    agent_config.load_agent_file(path)
}

fn lookup_agent_by_name(
    cli: &Cli,
    agent_config: &dyn AgentConfigRepository,
    name: &str,
) -> Result<AgentConfig, String> {
    let agents_dir = resolve_agents_dir(cli);
    let agents = agent_config.load_agents(&agents_dir)?;
    agents
        .get(name)
        .cloned()
        .ok_or_else(|| format_unknown_agent_error(name))
}

fn format_unknown_agent_error(name: &str) -> String {
    format!("Unknown agent: {name}")
}

fn format_missing_agent_error() -> String {
    "No agent specified. Use a positional argument or --agent-file.".to_string()
}
