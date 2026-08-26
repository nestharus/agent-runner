const BALANCING: &str = include_str!("../src/run/balancing/orchestration.rs");
const RESUME: &str = include_str!("../src/run/resume/orchestration.rs");
const REPL: &str = include_str!("../src/run/repl/orchestration.rs");
const DISPATCH: &str = include_str!("../src/dispatch.rs");
const DEFAULT_PROVIDER_REPL: &str =
    include_str!("../../crates/oulipoly-runtime/src/repl_default_provider.rs");
const SWEEP: &str = include_str!("../src/wake_coordinator/sweep/mod.rs");
const PROVIDER_PROCESS: &str = include_str!("../../crates/oulipoly-provider/src/process.rs");
const CLI_SPAWN_IDENTITY: &str =
    include_str!("../../crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs");
const HEADLESS_SUPERVISION: &str =
    include_str!("../../crates/oulipoly-runtime/src/executor/cli/supervision/mod.rs");

#[test]
fn initial_and_resume_production_paths_admit_before_provider_dispatch() {
    assert_before(
        function_body(BALANCING, "fn execute_balanced_attempt("),
        "admit_session_launch(",
        ".executor_service",
    );
    assert_before(
        function_body(RESUME, "fn run_resume_attempt("),
        "admit_session_launch(",
        "execute_resume_attempt_command(",
    );
    assert_before(
        function_body(REPL, "pub(crate) fn run_repl("),
        "admit_session_launch(",
        "execute_and_finalize_repl_attempt(",
    );
    assert_before(
        function_body(
            DEFAULT_PROVIDER_REPL,
            "fn run_registered_default_provider_repl<",
        ),
        "admit(&invocation)",
        "input.launcher.launch(",
    );
    assert!(
        function_body(DISPATCH, "fn run_default_provider_repl_for_project(")
            .contains("admit_session_launch(&invocation.id, None)")
    );
}

#[test]
fn wake_reclaim_selects_at_most_one_session_per_sweep() {
    let body = function_body(SWEEP, "fn run_wake_reclaim_sweep_with_owner(");
    assert!(body.contains("if let Some(candidate) = start"));
    assert!(!body.contains("for candidate in start"));
}

#[test]
fn rejected_sampled_rss_kill_policy_is_absent_from_provider_launches() {
    for source in [PROVIDER_PROCESS, CLI_SPAWN_IDENTITY] {
        assert!(!source.contains("ResourceContainment"));
        assert!(!source.contains("provider_memory_limit_exceeded"));
        assert!(!source.contains("OULIPOLY_PROVIDER_MEMORY_LIMIT_BYTES"));
    }
    assert!(CLI_SPAWN_IDENTITY.contains("libc::WNOWAIT"));
    assert!(PROVIDER_PROCESS.contains("libc::WNOWAIT"));
    assert!(!HEADLESS_SUPERVISION.contains("cleanup_process_group_after_child_exit"));
}

fn assert_before(source: &str, earlier: &str, later: &str) {
    let earlier = source
        .find(earlier)
        .expect("missing earlier production step");
    let later = source.find(later).expect("missing later production step");
    assert!(earlier < later, "{earlier} must precede {later}");
}

fn function_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source.find(signature).expect("missing production function");
    let source = &source[start..];
    let body_start = source.find('{').expect("missing function body");
    let mut depth = 0usize;
    for (offset, character) in source[body_start..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[..body_start + offset + 1];
                }
            }
            _ => {}
        }
    }
    panic!("unterminated production function")
}
