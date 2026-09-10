//! Role: orchestration, mapper.

use super::context::ExternalProviderDispatchContext;
use crate::diagnostics::classify_terminal_with_client;
use crate::provider_registry::ProviderRegistry;
use crate::services::{TerminalClassification, TerminalClassifyServiceRequest};
use oulipoly_provider::client::ProviderClient;
use oulipoly_provider::generated::DescribeResult;
use oulipoly_provider::stream::LaunchResult;
use std::time::SystemTime;

pub(crate) fn classify_after_launch_success(
    registry: &ProviderRegistry,
    client: &ProviderClient,
    describe: &DescribeResult,
    context: &ExternalProviderDispatchContext,
    launch_result: &LaunchResult,
) -> Option<TerminalClassification> {
    classify_terminal_with_client(
        registry,
        client,
        describe,
        terminal_classify_request(context, launch_result),
    )
    .ok()
}

fn terminal_classify_request(
    context: &ExternalProviderDispatchContext,
    launch_result: &LaunchResult,
) -> TerminalClassifyServiceRequest {
    TerminalClassifyServiceRequest {
        model_name: context.model.name.clone(),
        provider_name: context.provider.name.clone(),
        settings_id: context.settings_id.clone(),
        stdout: launch_result.stdout_bytes(),
        stderr: launch_result.stderr_bytes(),
        status: launch_result.exit.status.clone(),
        observed_at: SystemTime::now(),
    }
}
