//! ## Declared roles
//!
//! `orchestration`, `formatter`, `accessor`.

use super::ProviderEval;
use oulipoly_config::ModelConfig;

pub(super) fn trace_fanout_selection(
    model: &ModelConfig,
    band: &[&ProviderEval],
    selected: &ProviderEval,
) {
    let selected_provider_name = provider_name(model, selected);
    let band_provider_names = fanout_band_provider_names(model, band);
    let band_member_names = format_provider_names(&band_provider_names);
    emit_fanout_selection(
        selected_provider_name,
        band_member_names.as_str(),
        binding_score(selected),
    );
}

fn emit_fanout_selection(
    selected_provider_name: &str,
    band_member_names: &str,
    selected_binding_score: f64,
) {
    tracing::info!(
        selected_provider_name,
        band_member_names,
        selected_binding_score,
        "fanout selected"
    );
}

fn provider_name<'model>(model: &'model ModelConfig, eval: &ProviderEval) -> &'model str {
    model.providers[eval.index].name.as_str()
}

fn fanout_band_provider_names<'model>(
    model: &'model ModelConfig,
    band: &[&ProviderEval],
) -> Vec<&'model str> {
    band.iter().map(|eval| provider_name(model, eval)).collect()
}

fn format_provider_names(provider_names: &[&str]) -> String {
    provider_names.join(",")
}

fn binding_score(eval: &ProviderEval) -> f64 {
    eval.binding_score.unwrap()
}
