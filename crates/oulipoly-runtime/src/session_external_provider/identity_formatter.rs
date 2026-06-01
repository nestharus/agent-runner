//! Role: formatter.

use oulipoly_provider::generated::DescribeResult;

pub(crate) fn provider_instance_id(provider_id: &str) -> String {
    format!("{provider_id}-instance")
}

pub(crate) fn settings_id(describe: &DescribeResult, fallback: &str) -> String {
    describe
        .settings_schema_id
        .clone()
        .unwrap_or_else(|| fallback.to_string())
}
