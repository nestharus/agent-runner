//! Role: formatter.

use oulipoly_provider::error::ProviderClientError;

pub(crate) fn client_error_message(error: ProviderClientError) -> String {
    match error.provider_error_code() {
        Some(code) => format!("{code}: {error}"),
        None => format!("{}: {error}", error.transport_kind()),
    }
}
