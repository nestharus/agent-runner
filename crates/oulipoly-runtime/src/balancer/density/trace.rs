//! ## Declared roles
//!
//! `formatter`, `accessor`.

use super::ProviderEval;
use oulipoly_config::ModelConfig;

pub(super) fn trace_fanout_selection(
    model: &ModelConfig,
    band: &[&ProviderEval],
    selected: &ProviderEval,
) {
    let selected_provider_name = &model.providers[selected.index].name;
    let band_member_names = fanout_band_member_names(model, band);
    tracing::info!(
        selected_provider_name = selected_provider_name.as_str(),
        band_member_names = band_member_names.as_str(),
        selected_binding_score = selected.binding_score.unwrap(),
        "fanout selected"
    );
}

fn fanout_band_member_names(model: &ModelConfig, band: &[&ProviderEval]) -> String {
    band.iter()
        .map(|eval| model.providers[eval.index].name.as_str())
        .collect::<Vec<_>>()
        .join(",")
}
