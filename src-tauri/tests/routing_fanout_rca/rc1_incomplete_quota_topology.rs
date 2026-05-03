use super::*;
use chrono::{Duration, SecondsFormat, Utc};

/// RC-1 reproduction: a fresh cached single-window provider whose siblings
/// have the upstream's full window topology must not keep winning solely
/// because the missing short window removes the binding constraint.
///
/// Test intent risk: verifies the eventual routing guard/contract for
/// incomplete quota topology before density scoring chooses a provider.
#[test]
fn rc1_incomplete_cached_topology_does_not_dominate_pool_routing() {
    let db = in_memory_state();
    let model = model_named("claude-opus", &["claude", "claude2", "claude3"]);

    seed_learned_windows(&db, "claude", &[(0.02, 156, 0.01, 40)]);
    seed_learned_windows(&db, "claude2", &[(0.83, 84, 0.01, 40), (0.44, 2, 0.01, 40)]);
    seed_learned_windows(&db, "claude3", &[(0.66, 80, 0.01, 40), (0.16, 3, 0.01, 40)]);

    let long_resets = (Utc::now() + Duration::hours(80)).to_rfc3339_opts(SecondsFormat::Secs, true);
    let short_resets = (Utc::now() + Duration::hours(5)).to_rfc3339_opts(SecondsFormat::Secs, true);
    let fresh_two_window_claude_script = format!(
        r#"printf '%s' '{{"windows":[{{"used_percent":4,"resets_at":"{long_resets}"}},{{"used_percent":80,"resets_at":"{short_resets}"}}]}}'"#
    );
    let providers_cfg = provider_config_with_scripts(&[
        ("claude", fresh_two_window_claude_script.as_str()),
        ("claude2", "printf '%s' '{\"windows\":[]}'"),
        ("claude3", "printf '%s' '{\"windows\":[]}'"),
    ]);
    let sessions_cfg = SessionsConfig::default();
    let in_flight = InFlight::new();

    let selected =
        select_provider_name_with_ctx(&model, &db, &providers_cfg, &sessions_cfg, &in_flight);

    assert_eq!(
        selected, "claude3",
        "post-fix routing should not let claude's stale single-window cache dominate complete siblings"
    );
}
