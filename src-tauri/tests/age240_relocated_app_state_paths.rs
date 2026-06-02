#[path = "age240_relocated_support.rs"]
mod support;

#[test]
fn age38_load_providers_for_models_dir_with_routes_through_stub_and_defaults_errors() {
    support::age38_load_providers_for_models_dir_with_routes_through_stub_and_defaults_errors();
}

#[test]
fn age38_open_state_db_routes_through_injected_state_db_opener() {
    support::age38_open_state_db_routes_through_injected_state_db_opener();
}

#[test]
fn age38_open_state_db_returns_injected_opener_error() {
    support::age38_open_state_db_returns_injected_opener_error();
}
