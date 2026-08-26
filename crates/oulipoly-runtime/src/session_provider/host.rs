use crate::provider_registry::DescribeHostOptions;
use oulipoly_provider::generated::HostContext;
use std::collections::BTreeMap;
use std::path::Path;

pub(super) fn host_context(
    effective_cwd: Option<&Path>,
    host_options: &DescribeHostOptions,
) -> HostContext {
    host_context_with_home(effective_cwd, host_options, std::env::var("HOME").ok())
}

fn host_context_with_home(
    effective_cwd: Option<&Path>,
    host_options: &DescribeHostOptions,
    home: Option<String>,
) -> HostContext {
    HostContext {
        app: "oulipoly-agent-runner".to_string(),
        app_version: None,
        platform: Some(std::env::consts::OS.to_string()),
        working_directory: effective_cwd.map(|path| path.display().to_string()),
        config_root: host_options
            .config_root
            .as_ref()
            .map(|path| path.display().to_string()),
        data_root: host_options
            .data_root
            .as_ref()
            .map(|path| path.display().to_string()),
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
        let host_options = DescribeHostOptions {
            config_root: Some(Path::new("/tmp/oulipoly-config").to_path_buf()),
            data_root: Some(Path::new("/tmp/oulipoly-data").to_path_buf()),
        };
        let context = host_context_with_home(
            Some(Path::new("/tmp/oulipoly-work")),
            &host_options,
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
        assert_eq!(context.config_root.as_deref(), Some("/tmp/oulipoly-config"));
        assert_eq!(context.data_root.as_deref(), Some("/tmp/oulipoly-data"));
        assert_eq!(context.env.len(), 1);
    }

    #[test]
    fn host_context_omits_empty_or_missing_home() {
        let host_options = DescribeHostOptions::default();
        assert!(
            host_context_with_home(None, &host_options, Some(String::new()))
                .env
                .is_empty()
        );
        assert!(
            host_context_with_home(None, &host_options, None)
                .env
                .is_empty()
        );
    }
}
