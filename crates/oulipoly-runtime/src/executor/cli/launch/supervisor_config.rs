//! ## Declared roles
//!
//! Roles: mapper.
//!
//! - mapper: maps provider launch inputs and optional test override into a
//!   supervisor configuration with the prompt contract applied.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/launch/supervisor_config.rs
//!     role: adapter
//!     Translates:
//!       - provider-config-launch-contract
//!       - supervisor-config-contract
//! ```

use crate::executor::cli::supervision::SupervisorConfig;
use oulipoly_config::{PromptMode, ProviderConfig};

pub(super) fn supervisor_config_for_launch(
    provider: &ProviderConfig,
    prompt_mode: PromptMode,
    rendered_prompt: Option<String>,
    supervisor_config: Option<SupervisorConfig>,
) -> SupervisorConfig {
    let prompt_payload = rendered_prompt
        .and_then(|prompt| (prompt_mode == PromptMode::Stdin).then(|| prompt.into_bytes()));
    supervisor_config
        .unwrap_or_else(|| {
            SupervisorConfig::production(
                provider,
                prompt_mode,
                prompt_payload.clone().unwrap_or_default(),
            )
        })
        .with_prompt_contract(prompt_mode, prompt_payload)
}
