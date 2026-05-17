//! Claude tool-filter shape contract for AGE-103 proxy-mode validation.

use crate::InvocationMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeToolFilterShape {
    AllowedToolsCamel,
    AllowedToolsKebab,
    ToolsRestrict,
    NoFilter,
}

impl ClaudeToolFilterShape {
    pub fn is_unsafe_for_proxy_pty_mcp(self) -> bool {
        matches!(self, ClaudeToolFilterShape::ToolsRestrict)
    }

    pub fn detect_in_argv(argv: &[String]) -> Option<ClaudeToolFilterShape> {
        if argv.is_empty() {
            return None;
        }

        if argv_value_for_flag(argv, "--tools").is_some_and(|value| value.starts_with("mcp__")) {
            return Some(ClaudeToolFilterShape::ToolsRestrict);
        }
        if argv_value_for_flag(argv, "--allowedTools").is_some() {
            return Some(ClaudeToolFilterShape::AllowedToolsCamel);
        }
        if argv_value_for_flag(argv, "--allowed-tools").is_some() {
            return Some(ClaudeToolFilterShape::AllowedToolsKebab);
        }
        Some(ClaudeToolFilterShape::NoFilter)
    }
}

fn argv_value_for_flag<'a>(argv: &'a [String], flag: &str) -> Option<&'a str> {
    let mut i = 0;
    while i < argv.len() {
        let token = argv[i].as_str();
        if token == flag {
            return argv.get(i + 1).map(String::as_str);
        }
        if let Some((token_flag, value)) = parse_eq_pair_token(token)
            && token_flag == flag
        {
            return Some(value);
        }
        i += 1;
    }
    None
}

fn parse_eq_pair_token(token: &str) -> Option<(&str, &str)> {
    token.split_once('=')
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeToolFilterError {
    UnsafeToolsRestrictInProxy,
}

impl std::fmt::Display for ClaudeToolFilterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsafeToolsRestrictInProxy => f.write_str(
                "proxy-mode Claude must not use `--tools mcp__...` (AGE-104: this shape suppresses MCP tools/call dispatch in interactive PTY). Use `--allowedTools` or omit the filter.",
            ),
        }
    }
}

impl std::error::Error for ClaudeToolFilterError {}

pub fn validate_proxy_claude_filter_shape(
    mode: InvocationMode,
    shape: ClaudeToolFilterShape,
) -> Result<(), ClaudeToolFilterError> {
    if mode == InvocationMode::Proxy && shape.is_unsafe_for_proxy_pty_mcp() {
        return Err(ClaudeToolFilterError::UnsafeToolsRestrictInProxy);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_string()).collect()
    }

    #[test]
    fn claude_tool_filter_detect_unsafe_tools_mcp() {
        assert_eq!(
            ClaudeToolFilterShape::detect_in_argv(&argv(&["--tools", "mcp__age104p2__Task"])),
            Some(ClaudeToolFilterShape::ToolsRestrict)
        );
    }

    #[test]
    fn claude_tool_filter_detect_safe_allowed_tools_camel() {
        assert_eq!(
            ClaudeToolFilterShape::detect_in_argv(&argv(&[
                "--allowedTools",
                "mcp__age104p2__Task"
            ])),
            Some(ClaudeToolFilterShape::AllowedToolsCamel)
        );
    }

    #[test]
    fn claude_tool_filter_detect_safe_allowed_tools_kebab() {
        assert_eq!(
            ClaudeToolFilterShape::detect_in_argv(&argv(&[
                "--allowed-tools",
                "mcp__age104p2__Task"
            ])),
            Some(ClaudeToolFilterShape::AllowedToolsKebab)
        );
    }

    #[test]
    fn claude_tool_filter_detect_no_filter() {
        assert_eq!(
            ClaudeToolFilterShape::detect_in_argv(&argv(&["--model", "opus"])),
            Some(ClaudeToolFilterShape::NoFilter)
        );
    }

    #[test]
    fn validate_proxy_claude_filter_shape_rejects_tools_restrict_in_proxy() {
        assert_eq!(
            validate_proxy_claude_filter_shape(
                InvocationMode::Proxy,
                ClaudeToolFilterShape::ToolsRestrict
            ),
            Err(ClaudeToolFilterError::UnsafeToolsRestrictInProxy)
        );
    }

    #[test]
    fn validate_proxy_claude_filter_shape_accepts_allowed_tools_in_proxy() {
        for shape in [
            ClaudeToolFilterShape::AllowedToolsCamel,
            ClaudeToolFilterShape::AllowedToolsKebab,
            ClaudeToolFilterShape::NoFilter,
        ] {
            assert_eq!(
                validate_proxy_claude_filter_shape(InvocationMode::Proxy, shape),
                Ok(())
            );
        }
    }

    #[test]
    fn validate_proxy_claude_filter_shape_accepts_anything_in_headless() {
        for shape in [
            ClaudeToolFilterShape::AllowedToolsCamel,
            ClaudeToolFilterShape::AllowedToolsKebab,
            ClaudeToolFilterShape::ToolsRestrict,
            ClaudeToolFilterShape::NoFilter,
        ] {
            assert_eq!(
                validate_proxy_claude_filter_shape(InvocationMode::Headless, shape),
                Ok(())
            );
        }
    }
}
