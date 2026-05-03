use super::*;
use std::collections::BTreeSet;

/// RC-2 reproduction: learned quota scoring must not route every request in
/// a healthy two-provider pool to the same account for a modest score gap.
///
/// Test intent risk: verifies the eventual fanout contract across repeated
/// successful selections when more than one provider remains eligible.
#[test]
fn rc2_repeated_learned_quota_selections_fan_out_across_eligible_providers() {
    let db = in_memory_state();
    let model = model_named("gpt-high", &["codex", "codex2"]);

    seed_learned_windows(&db, "codex", &[(0.69, 54, 0.01, 40), (0.08, 2, 0.01, 40)]);
    seed_learned_windows(&db, "codex2", &[(0.44, 52, 0.01, 40), (0.06, 1, 0.01, 40)]);

    let mut seen = BTreeSet::new();
    for _ in 0..8 {
        let selected_index = select_provider(&model, &db, None);
        let selected_name = model.providers[selected_index].name.clone();
        seen.insert(selected_name.clone());
        record_successful_invocation(&db, &model.name, &selected_name, selected_index);
    }

    assert!(
        seen.len() > 1,
        "post-fix routing should fan out across eligible providers; selected only {seen:?}"
    );
}
