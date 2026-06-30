//! ## Declared roles
//!
//! `parser`, `mapper`, `accessor`, `formatter`, `predicate`
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-config/src/sessions.rs
//!     role: intrinsic-surface
//!     Domain: sessions_config_contract
//!     Owns:
//!       - turn_script per-provider transcript adapter contract
//!       - transcript_locator per-provider script contract
//!       - adapter state_dir resolution
//!       - missing-config defaults
//! ```

use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// One entry in `sessions.toml`, keyed by the provider name.
///
/// Mirrors `providers.toml` / `quota_script`: the application stays ignorant
/// of any specific CLI's storage format. The user wires in an adapter script
/// that knows how to enumerate session turns for that CLI — files, SQLite,
/// remote API, anything — and emits a normalized JSONL stream.
#[derive(Debug, Clone)]
pub struct SessionSourceEntry {
    /// Shell command. Receives `STATE_DIR` as an env var (a writable dir
    /// the script can use for its own incremental cursor). Outputs JSONL on
    /// stdout, one line per turn:
    /// `{"session_id":"...","turn_id":"...","timestamp":"<ISO8601>","role":"user"|"assistant"}`
    pub turn_script: String,
    /// Optional adapter that resolves a session id to the raw transcript path.
    /// Same shell/env contract as `turn_script`, but stdout is a single path.
    pub transcript_locator: Option<String>,
    /// Optional override for where the script keeps its bookkeeping. If
    /// unset, defaults to `<data_dir>/sessions/<provider_name>`.
    pub state_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Default)]
pub struct SessionsConfig {
    pub entries: HashMap<String, SessionSourceEntry>,
}

#[derive(Deserialize)]
struct RawEntry {
    turn_script: String,
    #[serde(default)]
    transcript_locator: Option<String>,
    #[serde(default)]
    state_dir: Option<String>,
}

impl SessionsConfig {
    /// Parse a sessions.toml, returning an empty config if the file doesn't exist.
    pub fn load(path: &Path) -> Result<Self, String> {
        if is_missing_sessions_file(path) {
            return Ok(Self::default());
        }
        let content = read_sessions_file(path)?;
        let raw = parse_sessions_toml(path, &content)?;
        let entries = map_session_entries(raw);
        Ok(Self { entries })
    }

    pub fn get(&self, name: &str) -> Option<&SessionSourceEntry> {
        self.entries.get(name)
    }
}

fn is_missing_sessions_file(path: &Path) -> bool {
    !path.exists()
}

fn read_sessions_file(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|err| format_sessions_read_error(path, err))
}

fn format_sessions_read_error(path: &Path, err: std::io::Error) -> String {
    format!("Failed to read {}: {err}", path.display())
}

fn parse_sessions_toml(path: &Path, content: &str) -> Result<HashMap<String, RawEntry>, String> {
    toml::from_str(content).map_err(|err| format_sessions_parse_error(path, err))
}

fn format_sessions_parse_error(path: &Path, err: toml::de::Error) -> String {
    format!("TOML parse error in {}: {err}", path.display())
}

fn map_session_entries(raw: HashMap<String, RawEntry>) -> HashMap<String, SessionSourceEntry> {
    raw.into_iter().map(map_session_entry_pair).collect()
}

fn map_session_entry_pair((name, raw): (String, RawEntry)) -> (String, SessionSourceEntry) {
    (name, map_session_entry(raw))
}

fn map_session_entry(raw: RawEntry) -> SessionSourceEntry {
    SessionSourceEntry {
        turn_script: raw.turn_script,
        transcript_locator: raw.transcript_locator,
        state_dir: map_session_state_dir(raw.state_dir),
    }
}

fn map_session_state_dir(state_dir: Option<String>) -> Option<PathBuf> {
    state_dir.map(|path| expand_tilde(&path))
}

fn expand_tilde(input: &str) -> PathBuf {
    expanded_tilde_path(input).unwrap_or_else(|| literal_path(input))
}

fn expanded_tilde_path(input: &str) -> Option<PathBuf> {
    let rest = tilde_relative_path(input)?;
    let home = home_dir()?;
    Some(home.join(rest))
}

fn tilde_relative_path(input: &str) -> Option<&str> {
    input.strip_prefix("~/")
}

fn home_dir() -> Option<PathBuf> {
    dirs::home_dir()
}

fn literal_path(input: &str) -> PathBuf {
    PathBuf::from(input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parses_turn_scripts() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            f,
            r#"
[claude]
turn_script = "claude-code-turns ~/.claude/projects"
transcript_locator = "claude-code-locate-transcript ~/.claude/projects"

[codex]
turn_script = "codex-turns ~/.codex/sessions"
transcript_locator = "codex-locate-transcript ~/.codex/sessions"
state_dir = "~/.cache/oulipoly/codex-cursor"
"#
        )
        .unwrap();
        let cfg = SessionsConfig::load(f.path()).unwrap();
        assert_eq!(cfg.entries.len(), 2);
        assert!(
            cfg.get("claude")
                .unwrap()
                .turn_script
                .contains("claude-code-turns")
        );
        assert_eq!(
            cfg.get("claude").unwrap().transcript_locator.as_deref(),
            Some("claude-code-locate-transcript ~/.claude/projects")
        );
        assert!(cfg.get("claude").unwrap().state_dir.is_none());
        assert!(
            cfg.get("codex")
                .unwrap()
                .state_dir
                .as_ref()
                .unwrap()
                .to_string_lossy()
                .contains(".cache/oulipoly")
        );
        assert_eq!(
            cfg.get("codex").unwrap().transcript_locator.as_deref(),
            Some("codex-locate-transcript ~/.codex/sessions")
        );
    }

    #[test]
    fn missing_file_is_empty_config() {
        let cfg = SessionsConfig::load(Path::new("/nonexistent/sessions.toml")).unwrap();
        assert!(cfg.entries.is_empty());
    }

    #[test]
    fn missing_turn_script_is_error() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "[claude]\nstate_dir = \"~/x\"\n").unwrap();
        assert!(SessionsConfig::load(f.path()).is_err());
    }

    /// `transcript_locator` is OPTIONAL — a valid SessionsConfig entry
    /// without it must parse cleanly and present `transcript_locator =
    /// None`. Trace then maps that to `transcript_state = "no_locator"`.
    #[test]
    fn entry_without_transcript_locator_is_valid() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            f,
            r#"
[claude]
turn_script = "claude-code-turns ~/.claude/projects"
"#
        )
        .unwrap();
        let cfg = SessionsConfig::load(f.path()).unwrap();
        assert_eq!(cfg.entries.len(), 1);
        let entry = cfg.get("claude").unwrap();
        assert!(entry.transcript_locator.is_none());
        assert!(entry.state_dir.is_none());
    }
}
