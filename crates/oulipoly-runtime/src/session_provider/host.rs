use oulipoly_provider::generated::HostContext;
use std::collections::BTreeMap;
use std::path::Path;

pub(super) fn host_context(effective_cwd: Option<&Path>) -> HostContext {
    host_context_with_home(effective_cwd, std::env::var("HOME").ok())
}

fn host_context_with_home(effective_cwd: Option<&Path>, home: Option<String>) -> HostContext {
    HostContext {
        app: "oulipoly-agent-runner".to_string(),
        app_version: None,
        platform: Some(std::env::consts::OS.to_string()),
        working_directory: effective_cwd.map(|path| path.display().to_string()),
        config_root: None,
        data_root: None,
        env: host_env(home),
        deadline_unix_ms: None,
    }
}

fn host_env(home: Option<String>) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    if let Some(home) = home.filter(|value| !value.is_empty()) {
        env.insert("HOME".to_string(), home);
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_context_forwards_non_empty_home() {
        let context = host_context_with_home(
            Some(Path::new("/tmp/oulipoly-work")),
            Some("/home/example".to_string()),
        );

        assert_eq!(
            context.working_directory.as_deref(),
            Some("/tmp/oulipoly-work")
        );
        assert_eq!(
            context.env.get("HOME").map(String::as_str),
            Some("/home/example")
        );
        assert_eq!(context.env.len(), 1);
    }

    #[test]
    fn host_context_omits_empty_or_missing_home() {
        assert!(
            host_context_with_home(None, Some(String::new()))
                .env
                .is_empty()
        );
        assert!(host_context_with_home(None, None).env.is_empty());
    }
}
