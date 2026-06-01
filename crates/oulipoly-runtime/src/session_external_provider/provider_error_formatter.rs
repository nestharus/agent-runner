//! Role: formatter.

use super::provider_error::ExternalSessionProviderError;
use super::provider_error_accessor::{provider_error_detail, provider_error_token};

pub(crate) fn provider_error_message(error: &ExternalSessionProviderError) -> String {
    format!(
        "{}: {}",
        provider_error_token(error),
        provider_error_detail(error)
    )
}
