//! validator

use super::formatter;

pub(super) fn validate_provider_repl_capability(
    provider: &oulipoly_config::ProviderConfig,
) -> Result<(), String> {
    if provider.interactive_args.is_some() {
        Ok(())
    } else {
        Err(formatter::repl_launch_failure_message(provider))
    }
}
