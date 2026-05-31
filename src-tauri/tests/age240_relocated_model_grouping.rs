#[path = "age240_relocated_support.rs"]
mod support;

#[test]
fn effective_provider_for_model_provider_rejects_out_of_range_index() {
    support::effective_provider_for_model_provider_rejects_out_of_range_index();
}

#[test]
fn effective_provider_for_model_provider_rejects_unresolved_empty_command() {
    support::effective_provider_for_model_provider_rejects_unresolved_empty_command();
}
