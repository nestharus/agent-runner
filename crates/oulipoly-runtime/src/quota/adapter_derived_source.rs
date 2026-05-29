//! S10/S11 provider-extraction move candidate for adapter-derived quota
//! credential script construction.

use oulipoly_config::{ProviderEntry, SessionStorage};
use std::path::{Path, PathBuf};

pub(super) fn derived_quota_script_from_provider_entry(entry: &ProviderEntry) -> Option<String> {
    let storage = entry.session_storage.as_ref()?;
    let script = session_storage_script(storage);
    derived_quota_script_from_adapter_command(&script)
}

fn session_storage_script(storage: &SessionStorage) -> String {
    match storage {
        SessionStorage::Script { cwd_script, .. } => cwd_script.clone(),
        _ => storage.cwd_script(),
    }
}

pub(super) fn derived_quota_script_from_adapter_command(command: &str) -> Option<String> {
    let parts = crate::executor::cli::shell_split(command);
    let adapter = adapter_parts(&parts)?;
    let source = quota_source_for_adapter(adapter.adapter_name)?;
    let account_root = Path::new(adapter.storage_root).parent()?;
    Some(source.script(account_root))
}

fn adapter_parts(parts: &[String]) -> Option<AdapterParts<'_>> {
    let adapter = parts.first()?;
    Some(AdapterParts {
        adapter_name: Path::new(adapter).file_name()?.to_str()?,
        storage_root: parts.get(1)?,
    })
}

struct AdapterParts<'a> {
    adapter_name: &'a str,
    storage_root: &'a str,
}

fn quota_source_for_adapter(adapter_name: &str) -> Option<QuotaSource> {
    match adapter_name {
        "claude-code-turns" | "claude-code-cwd" => Some(QuotaSource::Anthropic),
        "codex-turns" | "codex-cwd" => Some(QuotaSource::ChatGpt),
        _ => None,
    }
}

enum QuotaSource {
    Anthropic,
    ChatGpt,
}

impl QuotaSource {
    fn script(&self, account_root: &Path) -> String {
        match self {
            QuotaSource::Anthropic => self
                .script_with_credential("anthropic-usage", account_root.join(".credentials.json")),
            QuotaSource::ChatGpt => {
                self.script_with_credential("chatgpt-usage", account_root.join("auth.json"))
            }
        }
    }

    fn script_with_credential(&self, command: &str, credential: PathBuf) -> String {
        format!(
            "{command} {}",
            shell_word_arg(&credential.to_string_lossy())
        )
    }
}

fn shell_word_arg(input: &str) -> String {
    if is_bare_shell_word(input) {
        return input.to_string();
    }
    quote_shell_word(input)
}

fn is_bare_shell_word(input: &str) -> bool {
    input
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | '~'))
}

fn quote_shell_word(input: &str) -> String {
    format!("'{}'", input.replace('\'', r#"'\''"#))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_anthropic_quota_script_from_claude_storage_adapters() {
        assert_eq!(
            derived_quota_script_from_adapter_command("claude-code-turns ~/.claude3/projects")
                .as_deref(),
            Some("anthropic-usage ~/.claude3/.credentials.json")
        );
        assert_eq!(
            derived_quota_script_from_adapter_command("claude-code-cwd /home/nes/.claude/projects")
                .as_deref(),
            Some("anthropic-usage /home/nes/.claude/.credentials.json")
        );
    }

    #[test]
    fn derives_chatgpt_quota_script_from_codex_storage_adapters() {
        assert_eq!(
            derived_quota_script_from_adapter_command("codex-turns ~/.codex2/sessions").as_deref(),
            Some("chatgpt-usage ~/.codex2/auth.json")
        );
        assert_eq!(
            derived_quota_script_from_adapter_command("codex-cwd /home/nes/.codex/sessions")
                .as_deref(),
            Some("chatgpt-usage /home/nes/.codex/auth.json")
        );
    }
}
