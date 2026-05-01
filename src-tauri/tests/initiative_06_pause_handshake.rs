#![cfg(unix)]

mod fixtures;

use chrono::{DateTime, Duration, Utc};
use fixtures::initiative_06_pause_handshake::*;

/// Risk: T1 — resolver pass-through may lock the wrong mutable target.
/// Level: CLI integration.
/// Source: contract §6 and §8; proposal §9.1 Resolver pass-through; A1, A2.
/// Observable: exit 0; pause receipt uses resolved active session/provider and required JSON fields.
/// Residual: does not re-prove the shared resolver beyond this ownership fixture.
#[test]
fn pause_success_receipt_uses_resolved_active_session_and_provider() {
    let prepared = prepared_single_session();

    let output = prepared
        .fixture
        .run_pause(&prepared.session_id, Some(60_000));

    let json = assert_success(&output);
    for field in required_pause_fields() {
        assert!(json.get(field).is_some(), "missing {field} in {json}");
    }
    assert_eq!(json["session_id"], prepared.session_id);
    assert_eq!(json["provider_name"], prepared.provider_name);
    assert_pause_token(&json);
    assert!(
        json["lock_path"]
            .as_str()
            .unwrap()
            .ends_with(&format!("session-{}.lock", prepared.session_id)),
        "{json}"
    );
}

/// Risk: T2 — invalid UUID may create state or lock files before validation.
/// Level: end-to-end CLI.
/// Source: contract §6 and §8; proposal §9.1 Invalid UUID; A1.
/// Observable: exit 2; stderr JSON code invalid-session-id; no state dir or lock dir created.
/// Residual: clap structural usage text is not constrained here.
#[test]
fn pause_invalid_uuid_exits_two_before_state_or_lock_access() {
    let fixture = invalid_uuid_fixture();

    let output = fixture.run_pause("not-a-uuid", None);

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert_json_error(&output, "invalid-session-id");
    assert!(
        !fixture.data_home().join("oulipoly-agent-runner").exists(),
        "invalid UUID should not initialize default state"
    );
}

/// Risk: T3 — resolver failures may collapse into operational errors.
/// Level: CLI integration.
/// Source: contract §5, §6, and §8 T-resolver-error-mapping; proposal §9.1 Not found/Ambiguous.
/// Observable: not-found maps to 10, ambiguity maps to 11, model-resolution failure maps to 12.
/// Residual: the contract does not name every future resolver error variant.
#[test]
fn pause_resolver_error_mapping_covers_not_found_ambiguous_and_twelve() {
    let missing = missing_uuid_fixture();
    let missing_output = missing.run_pause("99999999-9999-4999-8999-999999999999", None);
    assert_eq!(missing_output.status.code(), Some(10), "{missing_output:?}");
    assert_json_error(&missing_output, "session-not-found");

    let ambiguous = ambiguous_session_fixture();
    let ambiguous_output = ambiguous.run_pause(SESSION_A, None);
    assert_eq!(
        ambiguous_output.status.code(),
        Some(11),
        "{ambiguous_output:?}"
    );
    assert_json_error(&ambiguous_output, "ambiguous-session");

    let unsupported = model_resolution_failure_fixture();
    let unsupported_output = unsupported.run_pause(SESSION_A, None);
    assert_eq!(
        unsupported_output.status.code(),
        Some(12),
        "{unsupported_output:?}"
    );
    let json = parse_stderr_json(&unsupported_output);
    assert!(json["error"]["code"].is_string(), "{json}");
}

/// Risk: T4 — concurrent harnesses may both receive valid leases.
/// Level: process integration.
/// Source: contract §8 T-concurrent-pause; proposal §9.1 Atomic acquire; A4, A5, A7, A8.
/// Observable: N subprocesses share XDG_DATA_HOME; exactly one exits 0 and the rest exit 13.
/// Residual: does not prove behavior on filesystems without working flock/rename semantics.
#[test]
fn concurrent_pause_only_one_subprocess_acquires_same_session() {
    let prepared = prepared_single_session();
    let children = (0..8)
        .map(|_| {
            prepared
                .fixture
                .spawn_pause(&prepared.session_id, Some(60_000))
        })
        .collect();

    let outputs = collect_outputs(children);

    let successes = outputs
        .iter()
        .filter(|output| output.status.code() == Some(0))
        .count();
    let busy = outputs
        .iter()
        .filter(|output| output.status.code() == Some(13))
        .count();
    assert_eq!(successes, 1, "{outputs:?}");
    assert_eq!(busy, outputs.len() - 1, "{outputs:?}");
    for output in outputs
        .iter()
        .filter(|output| output.status.code() == Some(13))
    {
        assert_json_error(output, "session-busy");
    }
}

/// Risk: T5 — per-session locking may over-block unrelated sessions.
/// Level: CLI integration.
/// Source: proposal §9.1 Per-session scope; contract §2 and §7; A2, A7, A8.
/// Observable: two different resolved sessions can both hold leases with distinct lock paths.
/// Residual: does not cover cross-user shared data directories, which are out of scope.
#[test]
fn pause_locks_are_scoped_per_session() {
    let fixture = prepared_two_sessions();

    let first = assert_success(&fixture.run_pause(SESSION_A, Some(60_000)));
    let second = assert_success(&fixture.run_pause(SESSION_B, Some(60_000)));

    assert_eq!(first["session_id"], SESSION_A);
    assert_eq!(second["session_id"], SESSION_B);
    assert_ne!(first["lock_path"], second["lock_path"]);
}

/// Risk: T6 — token generation may produce malformed or repeated release credentials.
/// Level: CLI integration.
/// Source: proposal §9.1 Token format; contract §1 and §4; A7.
/// Observable: token matches pause_[0-9a-f]{32}; a later acquisition gets a different token.
/// Residual: does not statistically certify OS CSPRNG quality.
#[test]
fn pause_tokens_have_required_format_and_change_between_acquisitions() {
    let prepared = prepared_single_session();

    let first = assert_success(
        &prepared
            .fixture
            .run_pause(&prepared.session_id, Some(60_000)),
    );
    assert_pause_token(&first);
    let token = first["token"].as_str().unwrap();
    assert_success(&prepared.fixture.run_resume(&prepared.session_id, token));

    let second = assert_success(
        &prepared
            .fixture
            .run_pause(&prepared.session_id, Some(60_000)),
    );
    assert_pause_token(&second);
    assert_ne!(first["token"], second["token"]);
}

/// Risk: T7 — TTL policy may be ignored or allow unbounded leases.
/// Level: CLI integration.
/// Source: contract §1 and §8 T-ttl-respected; proposal §9.1 TTL bounds; A5.
/// Observable: default expiry is about 60s, over-max TTL exits 2, and expired TTL permits reacquire.
/// Residual: short sleep does not model wall-clock skew across hosts.
#[test]
fn ttl_default_max_bound_and_expiry_are_respected() {
    let fixture = prepared_two_sessions();

    let before = Utc::now();
    let default_pause = assert_success(&fixture.run_pause(SESSION_A, None));
    let after = Utc::now();
    let expires_at = DateTime::parse_from_rfc3339(default_pause["expires_at"].as_str().unwrap())
        .unwrap()
        .with_timezone(&Utc);
    assert!(
        expires_at >= before + Duration::milliseconds(59_000),
        "{default_pause}"
    );
    assert!(
        expires_at <= after + Duration::milliseconds(61_000),
        "{default_pause}"
    );

    let over_max = fixture.run_pause(SESSION_B, Some(600_001));
    assert_eq!(over_max.status.code(), Some(2), "{over_max:?}");

    let short = assert_success(&fixture.run_pause(SESSION_B, Some(1)));
    std::thread::sleep(std::time::Duration::from_millis(25));
    let reacquire = assert_success(&fixture.run_pause(SESSION_B, Some(60_000)));
    assert_ne!(short["token"], reacquire["token"]);
}

/// Risk: T8 — crashed harnesses may leave stale lockfiles that block forever.
/// Level: process integration.
/// Source: contract §8 T-concurrent-stale; proposal §9.1 Stale acquire; A5, A7, A8.
/// Observable: N subprocesses race on a prewritten stale lock; exactly one replaces it and the rest see busy.
/// Residual: stale metadata shape is a fixture approximation of the implementation's persisted JSON.
#[test]
fn concurrent_stale_pause_only_one_subprocess_replaces_expired_lock() {
    let prepared = prepared_single_session();
    prepared.fixture.write_expired_lock(&prepared.session_id);

    let children = (0..8)
        .map(|_| {
            prepared
                .fixture
                .spawn_pause(&prepared.session_id, Some(60_000))
        })
        .collect();
    let outputs = collect_outputs(children);

    let successes = outputs
        .iter()
        .filter(|output| output.status.code() == Some(0))
        .count();
    let busy = outputs
        .iter()
        .filter(|output| output.status.code() == Some(13))
        .count();
    assert_eq!(successes, 1, "{outputs:?}");
    assert_eq!(busy, outputs.len() - 1, "{outputs:?}");
}

/// Risk: T9 — active leases may not refuse a second pause.
/// Level: CLI integration.
/// Source: contract §8 T-concurrent-pause and proposal §9.1 Busy lock; A4, A5, A7, A8.
/// Observable: second pause before expiry exits 13 with session-busy.
/// Residual: sibling writer paths are advisory until their observer PRs land.
#[test]
fn active_pause_blocks_second_pause_until_release_or_expiry() {
    let prepared = prepared_single_session();

    assert_success(
        &prepared
            .fixture
            .run_pause(&prepared.session_id, Some(60_000)),
    );
    let second = prepared
        .fixture
        .run_pause(&prepared.session_id, Some(60_000));

    assert_eq!(second.status.code(), Some(13), "{second:?}");
    assert_json_error(&second, "session-busy");
}

/// Risk: T10 — valid release may fail to clear the lease or may not be idempotent.
/// Level: CLI integration.
/// Source: contract §8 T-pause-release-cycle and T-release-after-expiry-with-marker; proposal §9.1 Correct release/Idempotent replay; A4, A7, A8.
/// Observable: pause -> resume succeeds, same-token replay succeeds with already_released, and later pause succeeds.
/// Residual: does not simulate crash during the release critical section.
#[test]
fn pause_release_cycle_is_idempotent_and_allows_future_pause() {
    let prepared = prepared_single_session();

    let pause = assert_success(&prepared.fixture.run_pause(&prepared.session_id, Some(1)));
    let token = pause["token"].as_str().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(25));

    let release = assert_success(&prepared.fixture.run_resume(&prepared.session_id, token));
    for field in required_resume_fields() {
        assert!(release.get(field).is_some(), "missing {field} in {release}");
    }
    assert_eq!(release["session_id"], prepared.session_id);
    assert_eq!(release["token"], token);
    assert_eq!(release["already_released"], false);

    let replay = assert_success(&prepared.fixture.run_resume(&prepared.session_id, token));
    assert_eq!(replay["already_released"], true);

    let next_pause = assert_success(
        &prepared
            .fixture
            .run_pause(&prepared.session_id, Some(60_000)),
    );
    assert_ne!(next_pause["token"], token);
}

/// Risk: T11 — wrong or malformed release tokens may release another caller's lease.
/// Level: CLI integration.
/// Source: contract §8 T-release-wrong-token and T-token-invalid-format; proposal §9.1 Wrong token/Marker token mismatch; A7.
/// Observable: wrong well-formed token exits 16 and lock remains; malformed token exits 16.
/// Residual: does not prove token secrecy outside stdout/filesystem handling.
#[test]
fn release_wrong_or_malformed_token_exits_16_and_preserves_lock() {
    let prepared = prepared_single_session();

    let pause = assert_success(
        &prepared
            .fixture
            .run_pause(&prepared.session_id, Some(60_000)),
    );
    let token = pause["token"].as_str().unwrap();
    let wrong = prepared.fixture.run_resume(
        &prepared.session_id,
        "pause_ffffffffffffffffffffffffffffffff",
    );
    assert_eq!(wrong.status.code(), Some(16), "{wrong:?}");
    assert_json_error(&wrong, "lock-token-invalid");

    let still_busy = prepared
        .fixture
        .run_pause(&prepared.session_id, Some(60_000));
    assert_eq!(still_busy.status.code(), Some(13), "{still_busy:?}");

    let correct = assert_success(&prepared.fixture.run_resume(&prepared.session_id, token));
    assert_eq!(correct["already_released"], false);

    let malformed = prepared
        .fixture
        .run_resume(&prepared.session_id, "not-a-pause-token");
    assert_eq!(malformed.status.code(), Some(16), "{malformed:?}");
    assert_json_error(&malformed, "lock-token-invalid");
}

/// Risk: T-release-after-expiry-no-marker — absent lock state may be mistaken for valid idempotent release.
/// Level: CLI integration.
/// Source: contract §8 T-release-after-expiry-no-marker; proposal §9.1 Missing lock no marker; A7.
/// Observable: valid session with no lockfile or marker exits 17 with lock-expired.
/// Residual: cannot distinguish manual lock deletion from never-acquired state.
#[test]
fn release_without_lock_or_marker_exits_lock_expired() {
    let prepared = prepared_single_session();

    let output = prepared.fixture.run_resume(
        &prepared.session_id,
        "pause_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    );

    assert_eq!(output.status.code(), Some(17), "{output:?}");
    assert_json_error(&output, "lock-expired");
}
