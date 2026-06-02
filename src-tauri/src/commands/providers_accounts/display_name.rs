//! Provider-specific display-name mapping residual island.
//!
//! This preserves the current mapping for L3 and remains an L6/S10/S11 residual.
//!
//! ## Declared roles
//!
//! `mapper`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/commands/providers_accounts/display_name.rs
//!     role: adapter
//!     Translates:
//!       - provider CLI name to display-name residual contract
//! ```

pub fn sync_provider_display_name(cli_name: &str) -> &str {
    // Provider-specific display-name residual for L6/S10/S11; preserve exactly.
    match cli_name {
        "claude" => "Anthropic",
        "codex" => "OpenAI",
        "gemini" => "Google",
        "opencode" => "OpenCode",
        _ => cli_name,
    }
}
