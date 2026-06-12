use oulipoly_state::ProviderSessionBinding;

pub(in crate::run::balancing) fn provider_session_binding(
    provider_session_id: &str,
) -> ProviderSessionBinding {
    ProviderSessionBinding {
        provider_session_id: provider_session_id.to_string(),
        capture_method: "forced_flag_verified",
        resume_input_id: None,
        provider_session_resolved_account: None,
    }
}

pub(in crate::run::balancing) fn pending_same_provider_verification_session_id(
    provider_session_id: Option<&str>,
) -> Option<String> {
    provider_session_id.map(str::to_string)
}

pub(in crate::run::balancing) fn pending_same_provider_verification(
    provider_index: usize,
    provider_session_id: Option<&str>,
) -> (usize, Option<String>) {
    (
        provider_index,
        pending_same_provider_verification_session_id(provider_session_id),
    )
}
