use oulipoly_config::ProviderEntry;

pub(crate) fn derive_vendor(provider_entry: &ProviderEntry) -> String {
    let Some(command) = provider_entry.command.as_deref() else {
        return "unknown".to_string();
    };
    let Some(first) = command.split_whitespace().next() else {
        return "unknown".to_string();
    };
    let vendor = first
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string();
    if vendor.is_empty() {
        "unknown".to_string()
    } else {
        vendor
    }
}
