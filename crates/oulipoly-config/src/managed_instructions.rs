pub const DEFAULT_PROVIDER_SYSTEM_PROMPT_POLICY: &str = "Do not use the host CLI built-in sub-agent or Task tool to spawn child invocations.\nFor a defined operator, use: agents -a <agent.md> -p <worktree-path> -f <prompt-file>\nThe agent file's model frontmatter selects the model. Never combine -m with -a; -m shadows the operator frontmatter model selection.\nFor an ad-hoc child with no agent file, use: agents -m <model> -p <worktree-path> -f <prompt-file>\nNever call bare agents with no arguments because it launches an interactive UI on the user's machine.\n";

pub const MANAGED_SYSTEM_PROMPT_START: &str = "<!-- OULIPOLY_AGENT_RUNNER:BEGIN system-prompt -->";
pub const MANAGED_SYSTEM_PROMPT_END: &str = "<!-- OULIPOLY_AGENT_RUNNER:END system-prompt -->";

pub fn materialize_managed_system_prompt(existing: &str, prompt: &str) -> Result<String, String> {
    if prompt.contains(MANAGED_SYSTEM_PROMPT_START) || prompt.contains(MANAGED_SYSTEM_PROMPT_END) {
        return Err("managed system prompt must not contain block markers".to_string());
    }

    let starts = existing
        .match_indices(MANAGED_SYSTEM_PROMPT_START)
        .collect::<Vec<_>>();
    let ends = existing
        .match_indices(MANAGED_SYSTEM_PROMPT_END)
        .collect::<Vec<_>>();
    let block = format!(
        "{MANAGED_SYSTEM_PROMPT_START}\n{}\n{MANAGED_SYSTEM_PROMPT_END}",
        prompt.trim_end()
    );

    match (starts.as_slice(), ends.as_slice()) {
        ([], []) if existing.is_empty() => Ok(format!("{block}\n")),
        ([], []) => {
            let separator = if existing.ends_with("\n\n") {
                ""
            } else if existing.ends_with('\n') {
                "\n"
            } else {
                "\n\n"
            };
            Ok(format!("{existing}{separator}{block}\n"))
        }
        ([(start, _)], [(end, _)]) if start < end => {
            let suffix_start = end + MANAGED_SYSTEM_PROMPT_END.len();
            Ok(format!(
                "{}{block}{}",
                &existing[..*start],
                &existing[suffix_start..]
            ))
        }
        _ => Err(
            "managed system prompt markers are incomplete, duplicated, or out of order".to_string(),
        ),
    }
}
