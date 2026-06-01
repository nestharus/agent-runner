//! Role: accessor.

use super::provider_error::ExternalSessionProviderError;

pub(crate) fn provider_error_token(error: &ExternalSessionProviderError) -> &'static str {
    error.token
}

pub(crate) fn provider_error_detail(error: &ExternalSessionProviderError) -> &str {
    &error.detail
}
