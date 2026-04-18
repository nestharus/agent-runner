PASS

PR-B's trace tests are contract-complete on the current branch and the exercised subset passes. The suite is strongest on determinism and negative-path coverage. I graded contract coverage and test-as-spec one notch lower than perfect because a few contract points are proven indirectly across unit and integration tests instead of by one end-to-end serialized-output spec.

**Grades**
- Determinism: `A`
- Focus: `A`
- Contract coverage: `B`
- Negative-test discipline: `A`
- Test-as-spec discipline: `B`

**Executed**
- `cargo test --manifest-path src-tauri/Cargo.toml --lib`
- `cargo test --manifest-path src-tauri/Cargo.toml --test pr_b_trace_integration`
- `cargo test --manifest-path src-tauri/Cargo.toml --bins no_subcommand_still_parses_existing_model_flow`

**Contract Walk**
- CLI parsing: `agents trace <uuid>` dispatch is covered by [trace_ascii_dispatches_and_prints_seeded_root](/home/nes/projects/agent-runner/src-tauri/tests/pr_b_trace_integration.rs:135)
- CLI parsing: `agents trace <uuid> --json` is covered by [trace_json_dispatches_and_returns_structured_root](/home/nes/projects/agent-runner/src-tauri/tests/pr_b_trace_integration.rs:149)
- CLI parsing: `agents trace <uuid> --inline-transcript` rejection is covered by [trace_subcommand_rejects_inline_transcript_without_json](/home/nes/projects/agent-runner/src-tauri/src/main.rs:500) and [inline_transcript_requires_json](/home/nes/projects/agent-runner/src-tauri/tests/pr_b_trace_integration.rs:163)
- CLI parsing: `agents trace <uuid> --json --inline-transcript` is covered by [trace_subcommand_parses_json_and_inline_transcript_flags](/home/nes/projects/agent-runner/src-tauri/src/main.rs:469) and [inline_transcript_with_json_is_accepted_and_returns_null_payloads](/home/nes/projects/agent-runner/src-tauri/tests/pr_b_trace_integration.rs:176)
- CLI parsing: `agents trace <uuid> --max-depth 10` is covered by [trace_subcommand_parses_json_and_inline_transcript_flags](/home/nes/projects/agent-runner/src-tauri/src/main.rs:469) and [trace_accepts_max_depth_flag](/home/nes/projects/agent-runner/src-tauri/tests/pr_b_trace_integration.rs:190)
- CLI parsing: `agents -m model "prompt"` regression is covered by [no_subcommand_still_parses_existing_model_flow](/home/nes/projects/agent-runner/src-tauri/src/main.rs:516) and [default_cli_flow_still_runs_without_subcommand](/home/nes/projects/agent-runner/src-tauri/tests/pr_b_trace_integration.rs:202)
- `list_invocation_children`: unknown parent is covered by [list_invocation_children_returns_empty_for_unknown_parent](/home/nes/projects/agent-runner/src-tauri/src/state/db.rs:2285)
- `list_invocation_children`: sorted by `created_at, id` is covered by [list_invocation_children_orders_by_created_at_then_row_id](/home/nes/projects/agent-runner/src-tauri/src/state/db.rs:2294)
- `list_invocation_children`: direct children only is covered by [list_invocation_children_returns_only_direct_children](/home/nes/projects/agent-runner/src-tauri/src/state/db.rs:2338)
- Tree walk: single root is covered by [single_root_with_no_children_emits_one_node](/home/nes/projects/agent-runner/src-tauri/src/trace/mod.rs:465)
- Tree walk: root plus two children ordered by `created_at` is covered by [root_with_children_is_sorted_by_created_at_then_row_id](/home/nes/projects/agent-runner/src-tauri/src/trace/mod.rs:476)
- Tree walk: three-level nesting is covered by [three_level_tree_walk_nests_children_under_their_parent](/home/nes/projects/agent-runner/src-tauri/src/trace/mod.rs:492)
- Tree walk: cycle leaf with no infinite descent is covered by [cycle_leaf_is_emitted_without_descending_forever](/home/nes/projects/agent-runner/src-tauri/src/trace/mod.rs:508)
- Tree walk: depth-limit leaf is covered by [depth_limit_leaf_is_emitted_when_requested](/home/nes/projects/agent-runner/src-tauri/src/trace/mod.rs:527)
- ASCII output: single-node contract string is covered by [ascii_format_matches_single_node_contract](/home/nes/projects/agent-runner/src-tauri/src/trace/mod.rs:545)
- ASCII output: multi-level indentation is covered by [ascii_indents_nested_children](/home/nes/projects/agent-runner/src-tauri/src/trace/mod.rs:559)
- ASCII output: legacy provider renders as `—` is covered by [legacy_rows_render_provider_dash](/home/nes/projects/agent-runner/src-tauri/src/trace/mod.rs:578)
- JSON output: top-level shape and nested children are covered by [json_output_has_top_level_shape_and_nested_children](/home/nes/projects/agent-runner/src-tauri/src/trace/mod.rs:589)
- JSON output: running rows keep null terminal fields is covered by [json_running_row_uses_null_terminal_fields](/home/nes/projects/agent-runner/src-tauri/src/trace/mod.rs:620)
- JSON output: PR-B session placeholders are covered by [json_session_fields_are_null_or_unresolved_in_pr_b](/home/nes/projects/agent-runner/src-tauri/src/trace/mod.rs:645)
- Error paths: non-existent UUID exits 1 with no stdout is covered by [trace_nonexistent_uuid_exits_one_and_keeps_stdout_empty](/home/nes/projects/agent-runner/src-tauri/tests/pr_b_trace_integration.rs:217)
- Error paths: malformed UUID error is covered by [trace_malformed_uuid_prints_clear_error](/home/nes/projects/agent-runner/src-tauri/tests/pr_b_trace_integration.rs:234)
- `--transcript` human mode footer is covered by [human_mode_transcript_footer_uses_unresolved_placeholder](/home/nes/projects/agent-runner/src-tauri/src/trace/mod.rs:673)
- `--inline-transcript`: `transcript: null` per JSON node is covered by [inline_transcript_adds_null_field_to_each_json_node](/home/nes/projects/agent-runner/src-tauri/src/trace/mod.rs:698) and [inline_transcript_with_json_is_accepted_and_returns_null_payloads](/home/nes/projects/agent-runner/src-tauri/tests/pr_b_trace_integration.rs:176)

**Specific Gap Check**
- Tree walk ordering uses deterministic fixture timestamps. [list_invocation_children_orders_by_created_at_then_row_id](/home/nes/projects/agent-runner/src-tauri/src/state/db.rs:2294) pins both timestamp order and row-id tie-break behavior.
- Cycle protection uses a real ancestor cycle. In [cycle_leaf_is_emitted_without_descending_forever](/home/nes/projects/agent-runner/src-tauri/src/trace/mod.rs:508), the fixture makes the root a child of the grandchild, so DFS genuinely revisits an ancestor.
- Depth limit actually triggers. [depth_limit_leaf_is_emitted_when_requested](/home/nes/projects/agent-runner/src-tauri/src/trace/mod.rs:527) runs with `max_depth = 1` against a three-level fixture and asserts the leaf marker.
- ASCII output has an exact-string assertion. [ascii_format_matches_single_node_contract](/home/nes/projects/agent-runner/src-tauri/src/trace/mod.rs:545) pins the full one-line rendering.
- JSON output is parsed through `serde_json::Value` at the CLI level. [trace_json_dispatches_and_returns_structured_root](/home/nes/projects/agent-runner/src-tauri/tests/pr_b_trace_integration.rs:149) and [inline_transcript_with_json_is_accepted_and_returns_null_payloads](/home/nes/projects/agent-runner/src-tauri/tests/pr_b_trace_integration.rs:176) both parse stdout and assert fields.
- Non-existent UUID error behavior is covered exactly by [trace_nonexistent_uuid_exits_one_and_keeps_stdout_empty](/home/nes/projects/agent-runner/src-tauri/tests/pr_b_trace_integration.rs:217).
- Clap rejects `--inline-transcript` without `--json` in both parser and binary tests: [trace_subcommand_rejects_inline_transcript_without_json](/home/nes/projects/agent-runner/src-tauri/src/main.rs:500), [inline_transcript_requires_json](/home/nes/projects/agent-runner/src-tauri/tests/pr_b_trace_integration.rs:163).
- Human-mode transcript placeholder footer is covered by [human_mode_transcript_footer_uses_unresolved_placeholder](/home/nes/projects/agent-runner/src-tauri/src/trace/mod.rs:673).

**Residual Notes**
- Coverage is broad enough for PASS; I did not find a missing contract bullet.
- The `B` grades are about sharpness, not holes. The JSON contract is asserted by selected fields rather than a full serialized golden payload, and a few CLI contract points are split across parser-level and integration-level tests instead of one single authoritative spec test.
