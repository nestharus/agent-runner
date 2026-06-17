//! ## Declared roles
//!
//! Roles: formatter, orchestration.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/provider_specific/policy/host_policy.rs
//!     role: adapter
//!     Translates:
//!       - provider-policy-config-contract
//!       - host-policy-argv-contract
//! ```

use oulipoly_config::{ProviderConfig, ToolRestrictions};

pub(in crate::executor) fn append_host_provider_policy(
    provider: &ProviderConfig,
    restrictions: Option<&ToolRestrictions>,
    base_args: &mut Vec<String>,
) {
    if let Some(override_text) = &provider.system_prompt_override {
        base_args.push("--append-system-prompt".to_string());
        base_args.push(override_text.clone());
    }
    append_host_restriction_args(restrictions, base_args);
}

fn append_host_restriction_args(
    restrictions: Option<&ToolRestrictions>,
    base_args: &mut Vec<String>,
) {
    let Some(restrictions) = restrictions else {
        return;
    };
    append_host_disallowed_tools(restrictions, base_args);
    append_host_allowed_tools(restrictions, base_args);
    append_host_disabled_slash_commands(restrictions, base_args);
}

fn append_host_disallowed_tools(restrictions: &ToolRestrictions, base_args: &mut Vec<String>) {
    if !restrictions.claude.disallowed_tools.is_empty() {
        base_args.push("--disallowed-tools".to_string());
        base_args.push(restrictions.claude.disallowed_tools.join(","));
    }
}

fn append_host_allowed_tools(restrictions: &ToolRestrictions, base_args: &mut Vec<String>) {
    if !restrictions.claude.allowed_tools.is_empty() {
        base_args.push("--allowed-tools".to_string());
        base_args.push(restrictions.claude.allowed_tools.join(","));
    }
}

fn append_host_disabled_slash_commands(
    restrictions: &ToolRestrictions,
    base_args: &mut Vec<String>,
) {
    if restrictions.claude.disable_slash_commands {
        base_args.push("--disable-slash-commands".to_string());
    }
}
