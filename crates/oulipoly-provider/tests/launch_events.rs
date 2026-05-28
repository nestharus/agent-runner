pub mod support {
    pub mod contract_matrix;
}

use serde_json::Value;
use support::contract_matrix::{EXPECTED_LAUNCH_EVENT_KINDS, fixtures, launch_event_fixture};

#[test]
fn launch_event_variants_and_terminal_exit_are_locked() {
    let fixtures = fixtures();
    let mut actual = EXPECTED_LAUNCH_EVENT_KINDS
        .iter()
        .map(|kind| {
            launch_event_fixture(&fixtures, kind)
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("{kind} launch event fixture must carry kind"))
        })
        .collect::<Vec<_>>();
    actual.sort();

    let mut expected = EXPECTED_LAUNCH_EVENT_KINDS.to_vec();
    expected.sort();
    assert_eq!(actual, expected);

    let exit = launch_event_fixture(&fixtures, "exit");
    assert!(exit.get("status").is_some(), "exit event must carry status");
    assert!(
        exit.get("terminal_signal").is_some(),
        "exit event must carry terminal_signal"
    );
}

#[test]
fn launch_stdout_stderr_payloads_remain_base64_strings() {
    let fixtures = fixtures();
    for kind in ["stdout", "stderr"] {
        let payload = launch_event_fixture(&fixtures, kind)
            .get("data_base64")
            .and_then(Value::as_str);
        assert!(
            payload.is_some(),
            "{kind} event must carry data_base64 string"
        );
    }
}

#[test]
fn launch_fixture_sequence_has_monotonic_seq_and_terminal_exit() {
    let fixtures = fixtures();
    let sequence = fixtures
        .pointer("/launch/sequence")
        .and_then(Value::as_array)
        .expect("launch sequence fixture must exist");

    let mut previous = 0;
    for kind_value in sequence {
        let kind = kind_value
            .as_str()
            .expect("launch sequence entries must be kind strings");
        let seq = launch_event_fixture(&fixtures, kind)
            .get("seq")
            .and_then(Value::as_u64)
            .unwrap_or_else(|| panic!("{kind} event must carry positive seq"));
        assert_eq!(seq, previous + 1, "{kind} event seq must be monotonic");
        previous = seq;
    }

    let last = sequence
        .last()
        .and_then(Value::as_str)
        .expect("launch sequence must not be empty");
    assert_eq!(
        last, "exit",
        "exit event must terminate launch fixture sequence"
    );
}
