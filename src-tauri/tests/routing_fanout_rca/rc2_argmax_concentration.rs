use super::*;

/// RC-2 replacement: learned quota selection follows projected usage, not
/// local invocation-count drift.
///
/// Old mapping:
/// `rc2_repeated_learned_quota_selections_fan_out_across_eligible_providers`
/// asserted local-count-driven spreading. AGE-25 replaces that with the
/// projected-usage invariant below.
#[test]
fn rc2_learned_quota_selection_follows_projected_usage_not_local_counts() {
    let db = in_memory_state();
    let model = model_named("gpt-high", &["codex", "codex2"]);

    seed_learned_windows(&db, "codex", &[(0.40, 24, 0.01, 40)]);
    seed_learned_windows(&db, "codex2", &[(0.55, 24, 0.01, 40)]);

    for _ in 0..5 {
        record_successful_invocation(&db, &model.name, "codex", 0);
    }

    for _ in 0..8 {
        let selected_index = select_provider(&model, &db, None).unwrap();
        let selected_name = model.providers[selected_index].name.clone();

        assert_eq!(
            selected_name, "codex",
            "selection should stay on the lower projected-usage provider even after local counts diverge"
        );

        record_successful_invocation(&db, &model.name, &selected_name, selected_index);
    }
}
