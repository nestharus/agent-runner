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
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = fs::read_to_string(path)
            .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
        let raw: HashMap<String, RawEntry> = toml::from_str(&content)
            .map_err(|e| format!("TOML parse error in {}: {e}", path.display()))?;
        let entries = raw
            .into_iter()
            .map(|(k, v)| {
                (
                    k,
                    SessionSourceEntry {
                        turn_script: v.turn_script,
                        transcript_locator: v.transcript_locator,
                        state_dir: v.state_dir.map(|s| expand_tilde(&s)),
                    },
                )
            })
            .collect();
        Ok(Self { entries })
    }

    pub fn get(&self, name: &str) -> Option<&SessionSourceEntry> {
        self.entries.get(name)
    }
}

fn expand_tilde(input: &str) -> PathBuf {
    if let Some(rest) = input.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
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
