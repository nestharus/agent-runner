//! Role: formatter.

pub(crate) fn provider_instance_id(provider_id: &str) -> String {
    format!("{provider_id}-instance")
}
