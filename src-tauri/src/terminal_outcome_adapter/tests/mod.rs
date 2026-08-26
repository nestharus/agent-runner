use oulipoly_runtime::executor::terminal_signal::TerminalSignalKind;
use oulipoly_runtime::executor::{
    CapturedChildInvocation, ExecutionResult, SessionCaptureMethod, SessionCaptureResult,
    TerminalSignal,
};
use std::time::SystemTime;

mod category;
mod disposition;
mod fixture_override;
mod outcome;

fn result_with_signal(kind: Option<TerminalSignalKind>) -> ExecutionResult {
    ExecutionResult {
        stdout: Vec::new(),
        stderr: "legacy quota text".to_string(),
        exit_code: 1,
        provider_index: 0,
        session_capture: SessionCaptureResult {
            session_id: None,
            method: SessionCaptureMethod::None,
        },
        resume_acceptance: None,
        terminal_reason: Some("quota_exhausted_inband".to_string()),
        terminal_signal: kind.map(|kind| TerminalSignal {
            kind,
            provider_name: "provider-a".to_string(),
            evidence: "typed evidence".to_string(),
            observed_at: SystemTime::UNIX_EPOCH,
        }),
        produced_assistant_response: false,
        prompt_acceptance_attestation: None,
        captured_child_invocations: Vec::<CapturedChildInvocation>::new(),
        returned_artifacts: Vec::new(),
    }
}

fn signal(kind: TerminalSignalKind) -> TerminalSignal {
    TerminalSignal {
        kind,
        provider_name: "provider-a".to_string(),
        evidence: "provider_session_id=session-1 baseline_assistant_turns=0 current_assistant_turns=0 new_assistant_turns=0".to_string(),
        observed_at: SystemTime::UNIX_EPOCH,
    }
}

fn production_source() -> &'static str {
    concat!(
        include_str!("../../terminal_outcome_adapter.rs"),
        "\n",
        include_str!("../category.rs"),
        "\n",
        include_str!("../disposition.rs"),
        "\n",
        include_str!("../fixture_override.rs"),
        "\n",
        include_str!("../marker.rs"),
        "\n",
        include_str!("../outcome.rs"),
    )
}

fn production_block_after(start: &str) -> String {
    let source = production_source();
    let open_idx = production_block_open_index(source, start);
    let close_idx = production_block_close_index(source, open_idx, start);
    source[open_idx + 1..close_idx].to_string()
}

fn production_block_open_index(source: &str, start: &str) -> usize {
    let start_idx = production_block_start_index(source, start);
    source[start_idx..]
        .find('{')
        .map(|idx| start_idx + idx)
        .unwrap_or_else(|| panic!("missing opening brace after {start}"))
}

fn production_block_start_index(source: &str, start: &str) -> usize {
    source
        .find(start)
        .unwrap_or_else(|| panic!("missing {start}"))
}

fn production_block_close_index(source: &str, open_idx: usize, start: &str) -> usize {
    let mut depth = 1usize;
    let mut idx = open_idx + 1;
    let bytes = source.as_bytes();

    while idx < bytes.len() {
        match bytes[idx] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return idx;
                }
            }
            _ => {}
        }
        idx += 1;
    }

    panic!("missing closing brace after {start}");
}

fn assert_production_contains(fragment_parts: &[&str]) {
    let fragment = fragment_parts.concat();
    assert!(
        production_source().contains(&fragment),
        "production terminal_outcome_adapter surface must contain {fragment:?} per /home/nes/projects/agent-runner/planning/age-153-terminal-signal-wiring/contracts/age-153-terminal-signal-wiring.md"
    );
}
