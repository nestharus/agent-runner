//! Opt-in offline pairing: AGE347_PROVIDER_BINARY must name a source-built
//! candidate, never an installed provider. Read-only describe/account discovery
//! bootstraps fixture identity; the page proxy admits only describe and
//! session.read_turns, with an empty environment and fixture HOME/data roots.
//! Every accepted page goes through the production Runner JSON/schema mapper.
use super::*;
use oulipoly_config::{ProviderEndpointConfig, ProviderEntry, ProvidersConfig};
use oulipoly_runtime::provider_registry::{ProviderRegistry, ProviderRegistryOptions};
use serde_json::json;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

// This fixture targets native storage with dot-account HOME directories and
// provider-id-scoped state. Identity comes from the real provider's read-only
// discovery contract, not a second Runner-maintained account/family catalog.
#[derive(serde::Deserialize)]
struct PairedProfile {
    family: String,
    settings_id: String,
    native_sessions: PathBuf,
}

fn profile(root: &Path) -> PairedProfile {
    serde_json::from_slice(&fs::read(root.join("paired-profile.json")).unwrap()).unwrap()
}

struct Paired {
    f: Fixture,
    transcript: PathBuf,
    mode: PathBuf,
    proxy: PathBuf,
    staging: PathBuf,
    registry: ProviderRegistry,
}

impl Paired {
    fn new() -> Self {
        let binary = std::env::var("AGE347_PROVIDER_BINARY")
            .expect("explicit source-built AGE347_PROVIDER_BINARY required");
        assert!(Path::new(&binary).is_absolute());
        assert!(Path::new(&binary).is_file());
        let mut f = Fixture::new();
        let proxy = write_proxy(f.root.path(), &binary);
        let discovered = profile(f.root.path());
        f.anchor.provider_instance_id = format!("{}-instance", discovered.family);
        f.anchor.settings_id = discovered.settings_id;
        let native = f
            .root
            .path()
            .join("home")
            .join(discovered.native_sessions)
            .join("2026/09/07");
        fs::create_dir_all(&native).unwrap();
        let transcript = native.join(format!("rollout-offline-{SESSION}.jsonl"));
        fs::write(&transcript, format!("{}\n", json!({"timestamp":"2026-09-07T12:00:00Z","type":"session_meta","payload":{"id":SESSION,"cwd":"/offline"}}))).unwrap();
        let mode = f.root.path().join("mode");
        fs::write(&mode, "normal").unwrap();
        let staging = f
            .root
            .path()
            .join("data/provider-state")
            .join(&discovered.family)
            .join("session-pages-v1");
        fs::create_dir_all(&staging).unwrap();
        fs::File::create(staging.join("unrelated-retained-sparse"))
            .unwrap()
            .set_len(536_870_912)
            .unwrap();
        let registry = registry(f.root.path(), &proxy);
        Self {
            f,
            transcript,
            mode,
            proxy,
            staging,
            registry,
        }
    }

    fn append(&self, role: &str, text: &str) {
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&self.transcript)
            .unwrap();
        writeln!(file, "{}", json!({"timestamp":"2026-09-07T12:00:01Z","type":"response_item","payload":{"type":"message","role":role,"content":[{"type":"input_text","text":text}]}})).unwrap();
    }

    fn anchor_and_submit(&mut self) {
        let page = self.read(SessionProviderPageCursor::Tail, 0, 0).unwrap();
        assert!(page.snapshot_complete && page.turns.is_empty());
        self.f.anchor.resume_token = page.resume_token;
        self.f.anchored_submit();
    }

    fn restart(&mut self) {
        self.f.restart();
        self.registry = registry(self.f.root.path(), &self.proxy);
    }

    fn read(
        &self,
        cursor: SessionProviderPageCursor,
        index: u64,
        sequence: u64,
    ) -> Result<SessionProviderReadPageResult, String> {
        read_paired(
            self.f.root.path(),
            &self.registry,
            &self.f.attempt,
            cursor,
            index,
            sequence,
        )
    }

    fn set_mode(&self, mode: &str) {
        fs::write(&self.mode, mode).unwrap();
    }

    fn assert_staging_untouched(&self) {
        assert_eq!(fs::read_dir(&self.staging).unwrap().count(), 1);
        assert_eq!(
            fs::metadata(self.staging.join("unrelated-retained-sparse"))
                .unwrap()
                .len(),
            536_870_912
        );
        let key_dir = self
            .f
            .root
            .path()
            .join("data/provider-state")
            .join(profile(self.f.root.path()).family)
            .join("observation-auth-v1");
        assert_eq!(fs::read_dir(&key_dir).unwrap().count(), 1);
        assert_eq!(fs::metadata(key_dir.join("key")).unwrap().len(), 32);
    }
}

fn identity(root: &Path) -> SessionProviderIdentity {
    let discovered = profile(root);
    SessionProviderIdentity {
        model_name: "offline-paired".into(),
        provider_name: "account".into(),
        provider_instance_id: Some(format!("{}-instance", discovered.family)),
        settings_id: discovered.settings_id,
    }
}

fn registry(root: &Path, proxy: &Path) -> ProviderRegistry {
    let discovered = profile(root);
    let providers = ProvidersConfig {
        entries: HashMap::from([(
            "account".into(),
            ProviderEntry {
                implementation: Some(ProviderEndpointConfig {
                    family: discovered.family,
                    executable: proxy.display().to_string(),
                }),
                settings_id: Some(discovered.settings_id),
                ..ProviderEntry::default()
            },
        )]),
    };
    ProviderRegistry::from_configs(
        &[],
        &providers,
        ProviderRegistryOptions::default()
            .with_config_root(root.join("config"))
            .with_data_root(root.join("data")),
    )
    .unwrap()
}

fn read_paired(
    root: &Path,
    registry: &ProviderRegistry,
    nonce: &str,
    cursor: SessionProviderPageCursor,
    index: u64,
    sequence: u64,
) -> Result<SessionProviderReadPageResult, String> {
    read_paired_typed(root, registry, nonce, cursor, index, sequence).map_err(|err| err.to_string())
}

fn read_paired_typed(
    root: &Path,
    registry: &ProviderRegistry,
    nonce: &str,
    cursor: SessionProviderPageCursor,
    index: u64,
    sequence: u64,
) -> Result<SessionProviderReadPageResult, oulipoly_runtime::session_provider::SessionProviderError>
{
    let cancellation = CancellationToken::new();
    read_turn_page(SessionProviderReadPageRequest {
        registry,
        identity: identity(root),
        session_id: SESSION,
        effective_cwd: None,
        projection: SessionProviderTurnProjection::UserObservation,
        expected_delivery_nonce: Some(nonce),
        cursor,
        expected_page_index: index,
        expected_turn_sequence: sequence,
        max_turns: 64,
        max_source_bytes: 512,
        max_response_bytes: 128 * 1024,
        max_inline_body_bytes: 65536,
        cancellation: &cancellation,
        timeout: Duration::from_secs(5),
    })
}

fn write_proxy(root: &Path, binary: &str) -> PathBuf {
    let proxy = root.join("offline-page-only.py");
    let script = include_str!("observation_paired_proxy.py")
        .replace("__FIXTURE_ROOT__", &serde_json::to_string(root).unwrap())
        .replace(
            "__PROVIDER_BINARY__",
            &serde_json::to_string(binary).unwrap(),
        );
    fs::write(&proxy, script).unwrap();
    fs::set_permissions(&proxy, fs::Permissions::from_mode(0o700)).unwrap();
    let bootstrap = std::process::Command::new(&proxy)
        .arg("--prepare-fixture")
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .output()
        .unwrap();
    assert!(
        bootstrap.status.success(),
        "{}",
        String::from_utf8_lossy(&bootstrap.stderr)
    );
    proxy
}

fn large_text() -> String {
    (0..60_000)
        .map(|i| char::from(b'a' + (i % 26) as u8))
        .collect()
}

#[test]
#[ignore = "requires explicit freshly source-built candidate provider; offline only"]
fn age347_paired_staging_full_large_inline_restart_and_exact_nonce_settlement() {
    let mut p = Paired::new();
    p.anchor_and_submit();
    let body = large_text();
    p.append("user", &body);
    p.append(
        "user",
        &format!("{}\n[OULIPOLY-DELIVERY {}]", p.f.envelope, p.f.attempt),
    );
    p.append("assistant", "offline notification accepted and answered");
    // Canonical still cannot allocate under the exact same full admission root.
    p.set_mode("canonical_request");
    let err = p
        .read(
            SessionProviderPageCursor::Beginning { after_token: None },
            0,
            0,
        )
        .unwrap_err();
    assert!(
        err.contains("session_turn_staging_capacity_exceeded"),
        "{err}"
    );
    p.set_mode("normal");
    for error in ["capacity_error", "transient_error"] {
        p.set_mode(error);
        let result = observe_delivery_with(&p.f.db, &p.f.attempt, &p.f.anchor, |c, i, s, _| {
            p.read(c, i, s)
        });
        assert!(result.is_err());
        p.restart();
        p.f.assert_pending_without_replay();
    }
    p.set_mode("normal");
    let mut calls = 0;
    let mut inline_seen = false;
    let mut expanded_total_seen = false;
    let mut done = false;
    for _ in 0..16 {
        let before = calls;
        done = observe_delivery_with(&p.f.db, &p.f.attempt, &p.f.anchor, |c, i, s, _| {
            let page = p.read(c.clone(), i, s)?;
            calls += 1;
            expanded_total_seen |= page.source_bytes_examined > 512;
            assert!(page.source_bytes_examined < 8_388_608 + 512);
            assert_eq!(
                page.warnings
                    .iter()
                    .filter(|w| w.starts_with("codex_observation_io_v1:"))
                    .count(),
                1
            );
            // Each read already launches a fresh provider process. Replay the same
            // opaque request through another fresh process and compare mapped bytes.
            if i == 17 {
                let replay = p.read(c, i, s)?;
                assert_eq!(page.page_digest, replay.page_digest);
            }
            for turn in &page.turns {
                if turn.body_bytes.is_some_and(|bytes| bytes > 60_000) {
                    assert_eq!(turn.body.as_ref().unwrap()[0]["text"], body);
                    assert!(turn.canonical_text_digest_verified);
                    inline_seen = true;
                }
            }
            Ok(page)
        })
        .unwrap();
        assert!(calls - before <= OBSERVATION_MAX_PAGES);
        if done {
            break;
        }
        p.restart();
        p.f.assert_pending_without_replay();
        if calls == OBSERVATION_MAX_PAGES {
            restart_in_fresh_runner_process(p.f.root.path());
            calls += OBSERVATION_MAX_PAGES;
            p.restart();
            p.f.assert_pending_without_replay();
        }
    }
    assert!(done && inline_seen && expanded_total_seen);
    assert!(calls > 100 && calls < 240, "calls={calls}");
    let settled = deliverable_pending_count_on(&mut p.f.db, &p.f.state, SESSION).unwrap();
    assert_eq!(settled, 0);
    let delivered = p.f.db.list_mailbox(SESSION, true).unwrap()[0]
        .delivered_at
        .clone();
    assert!(delivered.is_some());
    for _ in 0..3 {
        p.restart();
        assert!(
            observe_delivery_with(&p.f.db, &p.f.attempt, &p.f.anchor, |_, _, _, _| panic!(
                "confirmed attempt must not reread"
            ))
            .unwrap()
        );
        assert_eq!(
            deliverable_pending_count_on(&mut p.f.db, &p.f.state, SESSION).unwrap(),
            0
        );
        let rows = p.f.db.list_mailbox(SESSION, true).unwrap();
        assert_eq!(rows[0].delivered_at, delivered);
        assert_eq!(rows[0].delivery_attempts, 1);
        assert_eq!(p.f.submissions, 1);
    }
    p.assert_staging_untouched();
    println!(
        "paired real-provider pages={calls}; inline=60000; repeated provider process replay; source total > quantum accepted; canonical capacity refusal; synthetic submissions=1; durable settlement=1"
    );
}

#[test]
#[ignore = "requires explicit freshly source-built candidate provider; offline only"]
fn age347_paired_negative_accounting_preserves_uncertainty_and_checkpoint() {
    let mut p = Paired::new();
    p.anchor_and_submit();
    p.append("user", &large_text());
    p.append("user", &p.f.envelope);
    assert!(
        !observe_delivery_with(&p.f.db, &p.f.attempt, &p.f.anchor, |c, i, s, _| p
            .read(c, i, s))
        .unwrap()
    );
    let checkpoint = p.f.db.delivery_observation_progress(&p.f.attempt).unwrap();
    for mode in [
        "missing",
        "duplicate",
        "negative",
        "fractional",
        "exponent",
        "signed",
        "overflow",
        "total_overflow",
        "total_ceiling",
        "noninteger_total",
        "negative_total",
        "wrong_instance",
        "wrong_settings",
        "sum_overflow",
        "wrong_total",
        "forward_over",
        "reconstruction_limit",
        "malformed",
        "wrong_projection",
        "wrong_session",
        "wrong_nonce",
        "wrong_account",
        "wrong_snapshot",
    ] {
        p.set_mode(mode);
        let err = observe_delivery_with(&p.f.db, &p.f.attempt, &p.f.anchor, |c, i, s, _| {
            p.read(c, i, s)
        })
        .unwrap_err();
        println!("negative boundary {mode}: {err}");
        assert_eq!(
            p.f.db.delivery_observation_progress(&p.f.attempt).unwrap(),
            checkpoint
        );
        p.restart();
        p.f.assert_pending_without_replay();
    }
    p.set_mode("normal");
    // Same-length mutation is a changed generation, never fabricated observation.
    let bytes = fs::read(&p.transcript).unwrap();
    std::thread::sleep(Duration::from_millis(10));
    fs::write(&p.transcript, &bytes).unwrap();
    assert!(
        observe_delivery_with(&p.f.db, &p.f.attempt, &p.f.anchor, |c, i, s, _| p
            .read(c, i, s))
        .is_err()
    );
    p.f.assert_pending_without_replay();
    p.assert_staging_untouched();
}

#[test]
#[ignore = "requires explicit freshly source-built candidate provider; offline only"]
fn age347_paired_production_quantum_wrong_nonce_and_duplicate_prose_never_ack() {
    for control in ["absent", "wrong_nonce", "assistant", "duplicate", "exact"] {
        let mut p = Paired::new();
        // Production request budgets, including max_inline_body_bytes=0.
        let cancel = CancellationToken::new();
        let page = read_turn_page(SessionProviderReadPageRequest {
            registry: &p.registry,
            identity: identity(p.f.root.path()),
            session_id: SESSION,
            effective_cwd: None,
            projection: SessionProviderTurnProjection::UserObservation,
            expected_delivery_nonce: Some(&p.f.attempt),
            cursor: SessionProviderPageCursor::Tail,
            expected_page_index: 0,
            expected_turn_sequence: 0,
            max_turns: OBSERVATION_MAX_TURNS,
            max_source_bytes: OBSERVATION_MAX_SOURCE_BYTES,
            max_response_bytes: OBSERVATION_MAX_RESPONSE_BYTES,
            max_inline_body_bytes: 0,
            cancellation: &cancel,
            timeout: OBSERVATION_TIMEOUT,
        })
        .unwrap();
        p.f.anchor.resume_token = page.resume_token;
        p.f.anchored_submit();
        match control {
            "wrong_nonce" => p.append("user", &p.f.envelope.replace(&p.f.attempt, "other-nonce")),
            "assistant" => p.append("assistant", &p.f.envelope),
            "duplicate" => {
                p.append("user", &p.f.envelope);
                p.append("user", &p.f.envelope);
            }
            "exact" => p.append("user", &p.f.envelope),
            _ => (),
        }
        let result = confirm_delivery_observation(
            &p.f.db,
            &p.f.attempt,
            &p.registry,
            identity(p.f.root.path()),
            p.f.root.path(),
            &p.f.anchor,
        )
        .unwrap();
        assert_eq!(result, control == "exact", "{control}");
        if !result {
            p.f.assert_pending_without_replay();
        }
        p.assert_staging_untouched();
    }
}

// Subprocess-only entry point. The parent test supplies only its synthetic root;
// durable attempt identity, anchor and continuation are read back from the DB.
#[test]
#[ignore = "invoked only by the offline paired parent"]
fn age347_offline_recovery_subprocess() {
    let root = PathBuf::from(std::env::var("AGE347_OFFLINE_ROOT").unwrap());
    let mut db = MailboxDb::open(&root.join("pid-identity.db")).unwrap();
    let state = oulipoly_state::StateDb::open(&root.join("state.db")).unwrap();
    let pending = db
        .pending_delivery_observations(SESSION, 4)
        .unwrap()
        .remove(0);
    let registry = registry(&root, &root.join("offline-page-only.py"));
    let mut calls = 0;
    assert!(
        !observe_delivery_with(&db, &pending.attempt_id, &pending.anchor, |c, i, s, _| {
            calls += 1;
            let result = read_paired(&root, &registry, &pending.attempt_id, c.clone(), i, s)?;
            if calls == 1 {
                assert_eq!(i, 16);
                let replay = read_paired(&root, &registry, &pending.attempt_id, c, i, s)?;
                assert_eq!(result.page_digest, replay.page_digest);
            }
            Ok(result)
        })
        .unwrap()
    );
    assert_eq!(calls, OBSERVATION_MAX_PAGES);
    assert_eq!(
        deliverable_pending_count_on(&mut db, &state, SESSION).unwrap(),
        1
    );
    assert!(prepare_headless_resume_delivery_on(&mut db, SESSION, "chain", None, None).is_err());
    assert!(
        db.begin_headless_delivery_submission(
            &pending.attempt_id,
            SESSION,
            "native-invocation",
            true
        )
        .is_err()
    );
    println!(
        "fresh Runner test process recovered opaque checkpoint at page16 and advanced16 pages; duplicate submission refused"
    );
}

fn restart_in_fresh_runner_process(root: &Path) {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "age347_offline_recovery_subprocess",
            "--ignored",
            "--nocapture",
        ])
        .env_clear()
        .env("AGE347_OFFLINE_ROOT", root)
        .env("HOME", root.join("home"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("TMPDIR", root)
        .env("PATH", "/usr/bin:/bin")
        .output()
        .unwrap();
    println!("{}", String::from_utf8_lossy(&output.stdout));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[path = "observation_paired_boundaries.rs"]
mod boundaries;

#[test]
#[ignore = "requires explicit freshly source-built candidate provider; offline only"]
fn age353_paired_typed_anchor_stop_explicit_rearm_and_transient_control() {
    let mut p = Paired::new();
    p.set_mode("capacity_error");
    let error = read_paired_typed(
        p.f.root.path(),
        &p.registry,
        &p.f.attempt,
        SessionProviderPageCursor::Tail,
        0,
        0,
    )
    .unwrap_err();
    assert_eq!(
        error.fixed_observation_stop_reason(),
        Some("session_turn_staging_capacity_exceeded")
    );
    let message = retain_observation_failure(&p.f.db, SESSION, &p.f.attempt, error);
    p.f.db
        .record_delivery_observation_anchor_failure(&p.f.attempt, SESSION, &message)
        .unwrap();
    let stop = p.f.db.mailbox_observation_stop(SESSION).unwrap().unwrap();
    p.restart();
    p.set_mode("normal"); // storage restoration is not automatic rearm authority
    assert!(
        prepare_headless_resume_delivery_on(&mut p.f.db, SESSION, "chain", None, None).is_err()
    );
    assert_eq!(p.f.submissions, 0);
    assert_eq!(p.f.db.list_pending(SESSION).unwrap().len(), 1);
    p.f.db
        .rearm_mailbox_observation(SESSION, &stop.stop_id, "fixture refusal disabled")
        .unwrap();
    p.set_mode("transient_error");
    let error = read_paired_typed(
        p.f.root.path(),
        &p.registry,
        &p.f.attempt,
        SessionProviderPageCursor::Tail,
        0,
        0,
    )
    .unwrap_err();
    assert_eq!(error.fixed_observation_stop_reason(), None);
    retain_observation_failure(&p.f.db, SESSION, &p.f.attempt, error);
    assert!(p.f.db.mailbox_observation_stop(SESSION).unwrap().is_none());
    p.set_mode("normal");
    p.anchor_and_submit();
    assert!(p.f.submit().is_err());
    p.append("user", &p.f.envelope);
    assert!(
        confirm_delivery_observation(
            &p.f.db,
            &p.f.attempt,
            &p.registry,
            identity(p.f.root.path()),
            p.f.root.path(),
            &p.f.anchor
        )
        .unwrap()
    );
    p.restart();
    assert_eq!(
        deliverable_pending_count_on(&mut p.f.db, &p.f.state, SESSION).unwrap(),
        0
    );
    assert_eq!(
        p.f.db.list_mailbox(SESSION, true).unwrap()[0].delivery_attempts,
        1
    );
    assert_eq!(p.f.submissions, 1);
    p.assert_staging_untouched();
}

#[test]
#[ignore = "requires explicit frozen source-built provider; no model workloads"]
fn age355_paired_periodic_active_partial_restart_receipt_and_failure() {
    let mut p = Paired::new();
    p.f.state
        .start_invocation(&oulipoly_state::InvocationStart {
            invocation_uuid: "native-invocation".into(),
            model_name: "offline".into(),
            provider_name: "account".into(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    p.f.db
        .wake_sessions()
        .upsert_session_metadata(oulipoly_state::mailbox::SessionMetadataUpsert {
            session_id: SESSION,
            mode: "headless",
            invocation_uuid: Some("native-invocation"),
            provider_name: Some("account"),
            model_name: Some("offline"),
            models_dir: None,
            effective_cwd: Some("/offline"),
        })
        .unwrap();
    p.anchor_and_submit();
    // Native writer appends exact canonical input but no final record delimiter.
    // Neither assistant output nor successful provider exit participates.
    let record = json!({"timestamp":"2026-09-07T12:00:01Z","type":"response_item",
        "payload":{"type":"message","role":"user", "internal_chat_message_metadata_passthrough":{"content_item_kinds":["user.text"]},
            "content":[{"type":"input_text","text":p.f.envelope}]}})
    .to_string();
    fs::OpenOptions::new()
        .append(true)
        .open(&p.transcript)
        .unwrap()
        .write_all(record.as_bytes())
        .unwrap();
    crate::native_receipt::poll_headless_receipt_tick_with(&mut p.f.db, |_| {
        Ok(registry(p.f.root.path(), &p.proxy))
    })
    .unwrap();
    assert!(
        p.f.db
            .delivery_observation_confirmation(&p.f.attempt)
            .unwrap()
            .is_none()
    );
    p.restart();
    fs::OpenOptions::new()
        .append(true)
        .open(&p.transcript)
        .unwrap()
        .write_all(b"\n")
        .unwrap();
    let root = p.f.root.path().to_path_buf();
    let attempt = p.f.attempt.clone();
    let (send, received) = std::sync::mpsc::channel();
    // Exercise the in-process fixture driver and production scanner with private
    // roots. The dedicated process-helper experiment below covers actual polling.
    let guard = crate::native_receipt::start_receipt_polling_with(
        move || {
            let mut db = MailboxDb::open(&root.join("pid-identity.db"))?;
            crate::native_receipt::poll_headless_receipt_tick_with(&mut db, |_| {
                Ok(registry(&root, &root.join("offline-page-only.py")))
            })?;
            if db.delivery_observation_confirmation(&attempt)?.is_some() {
                let _ = send.send(());
            }
            Ok(())
        },
        Duration::from_millis(20),
    )
    .unwrap();
    received
        .recv_timeout(Duration::from_secs(10))
        .expect("periodic observer did not confirm active input");
    drop(guard); // joins observer before deleting any fixture root
    assert_eq!(
        p.f.state
            .get_invocation_by_uuid("native-invocation")
            .unwrap()
            .unwrap()
            .status,
        oulipoly_state::InvocationStatus::Running
    );
    assert!(p.f.db.list_pending(SESSION).unwrap().is_empty());
    assert!(
        p.f.db
            .mark_delivery_attempt_failed(&p.f.attempt, SESSION, None, &[p.f.seq], "native_exit_1")
            .unwrap()
    );
    p.restart();
    assert!(
        p.f.db
            .delivery_observation_confirmation(&p.f.attempt)
            .unwrap()
            .is_some()
    );
    assert_eq!(
        p.f.db.list_mailbox(SESSION, true).unwrap()[0].delivery_attempts,
        1
    );
    assert_eq!(p.f.submissions, 1);
}

#[test]
#[ignore = "requires explicit frozen source-built provider; no model workloads"]
fn age355_paired_contextual_input_and_duplicate_are_not_receipt() {
    for duplicate in [false, true] {
        let mut p = Paired::new();
        p.anchor_and_submit();
        let kind = if duplicate {
            "user.text"
        } else {
            "compaction.summary"
        };
        let record = json!({"timestamp":"2026-09-07T12:00:01Z","type":"response_item",
            "payload":{"type":"message","role":"user", "internal_chat_message_metadata_passthrough":{"content_item_kinds":[kind]},
                "content":[{"type":"input_text","text":p.f.envelope}]}})
        .to_string();
        let mut native = fs::OpenOptions::new()
            .append(true)
            .open(&p.transcript)
            .unwrap();
        writeln!(native, "{record}").unwrap();
        if duplicate {
            writeln!(native, "{record}").unwrap();
        }
        assert!(
            !observe_delivery_with(
                &p.f.db,
                &p.f.attempt,
                &p.f.anchor,
                |cursor, index, seq, _| p.read(cursor, index, seq)
            )
            .unwrap()
        );
        p.f.assert_pending_without_replay();
    }
}

#[test]
#[ignore = "requires explicit frozen source-built provider; no model workloads"]
fn age355_paired_slow_preflight_retains_page_opportunity() {
    let mut p = Paired::new();
    p.anchor_and_submit();
    p.append("user", &p.f.envelope);
    p.set_mode("slow_describe");
    // Fresh private registry forces successful slow preparation. Its describe
    // timeout is independent of the page budget (as is synchronous identity IO).
    // This is not a claim that production's 2s describe timeout permits 2.2s.
    let registry = registry(p.f.root.path(), &p.proxy);
    assert!(
        crate::native_receipt::confirm_delivery_observation_bounded(
            &p.f.db,
            &p.f.attempt,
            &registry,
            identity(p.f.root.path()),
            p.f.root.path(),
            &p.f.anchor,
            1,
            Duration::from_secs(2),
        )
        .unwrap()
    );
    assert!(p.f.db.list_pending(SESSION).unwrap().is_empty());
    assert_eq!(p.f.submissions, 1);
}

#[test]
#[ignore = "requires explicit frozen source-built provider; Unix pinned-handle probe only"]
fn age355_paired_identity_distinguishes_replacement_and_in_place_change() {
    let p = Paired::new();
    let endpoint = p.registry.preflight_account("account").unwrap();
    let client = endpoint.client();
    let initial = client.pinned_executable_identity_sha256().unwrap();
    assert_eq!(initial, client.pinned_executable_identity_sha256().unwrap());
    // Same bytes and pathname do not imply the same retained executable.
    let replacement = p.f.root.path().join("replacement.py");
    fs::copy(&p.proxy, &replacement).unwrap();
    fs::rename(&replacement, &p.proxy).unwrap();
    let new_registry = registry(p.f.root.path(), &p.proxy);
    let new_endpoint = new_registry.preflight_account("account").unwrap();
    let old_pinned = client.pinned_executable_identity_sha256().unwrap();
    let replaced = new_endpoint
        .client()
        .pinned_executable_identity_sha256()
        .unwrap();
    assert_ne!(old_pinned, replaced);
    fs::OpenOptions::new()
        .append(true)
        .open(&p.proxy)
        .unwrap()
        .write_all(b"\n# private in-place revision\n")
        .unwrap();
    assert_ne!(
        replaced,
        new_endpoint
            .client()
            .pinned_executable_identity_sha256()
            .unwrap()
    );
    assert_eq!(
        old_pinned,
        client.pinned_executable_identity_sha256().unwrap()
    );
}

#[test]
#[ignore = "requires explicit source-built Runner and provider; private offline helper execution"]
fn age355_paired_process_helper_cache_admission_invalidation_and_receipt() {
    let binary = std::env::var("AGE355_RUNNER_BINARY").expect("explicit source-built Runner");
    let mut p = Paired::new();
    // A persisted launch-model directory is not a page-read dependency. A
    // non-directory here would fail the former global model collection load.
    let unusable_models = p.f.root.path().join("launch-models-not-a-directory");
    fs::write(&unusable_models, "not a model directory").unwrap();
    p.f.db
        .wake_sessions()
        .upsert_session_metadata(oulipoly_state::mailbox::SessionMetadataUpsert {
            session_id: SESSION,
            mode: "headless",
            invocation_uuid: Some("native-invocation"),
            provider_name: Some("account"),
            model_name: Some("offline-paired"),
            models_dir: unusable_models.to_str(),
            effective_cwd: Some("/offline"),
        })
        .unwrap();
    p.anchor_and_submit();
    let root = p.f.root.path();
    let config_home = root.join("helper-config");
    let config = config_home.join("oulipoly-agent-runner");
    fs::create_dir_all(&config).unwrap();
    let discovered = profile(root);
    let provider_config = format!(
        "[account]\nsettings_id = {:?}\nimplementation = {{ family = {:?}, executable = {:?} }}\n",
        discovered.settings_id,
        discovered.family,
        p.proxy.display().to_string()
    );
    fs::write(config.join("providers.toml"), &provider_config).unwrap();
    let operations = root.join("inspection-operations");
    fs::write(&operations, "").unwrap();
    let start_helper = || {
        let mut command = std::process::Command::new(&binary);
        command
            .arg(crate::native_receipt::helper::ARG)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", root.join("home"))
            .env("OULIPOLY_DATA_DIR", root)
            .env("OULIPOLY_CONFIG_HOME", &config_home)
            .env("XDG_CONFIG_HOME", &config_home)
            .env("XDG_DATA_HOME", root.join("data"))
            .env("TMPDIR", root);
        crate::native_receipt::helper::start_command(command).unwrap()
    };
    let first = start_helper();
    let second = start_helper();
    let count = |name: &str| {
        fs::read_to_string(&operations)
            .unwrap()
            .lines()
            .filter(|line| *line == name)
            .count()
    };
    let wait = |condition: &dyn Fn() -> bool| {
        let start = std::time::Instant::now();
        while !condition() && start.elapsed() < Duration::from_secs(15) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            condition(),
            "helper did not make expected progress; operations={}",
            fs::read_to_string(&operations).unwrap()
        );
    };
    wait(&|| count("session.read_turns") >= 3);
    assert_eq!(
        count("describe"),
        1,
        "unchanged ticks/competing observers must reuse preparation"
    );
    assert!(
        p.f.db
            .delivery_observation_confirmation(&p.f.attempt)
            .unwrap()
            .is_none()
    );
    // Identical bytes, different inode: force new describe and original-anchor
    // requalification, never continue using the pinned prior endpoint.
    fs::copy(&p.proxy, root.join("replacement.py")).unwrap();
    fs::rename(root.join("replacement.py"), &p.proxy).unwrap();
    wait(&|| count("describe") >= 2);
    assert_eq!(count("describe"), 2);
    // All parsed configuration inputs invalidate, not only the pathname.
    fs::write(
        config.join("providers.toml"),
        format!("{provider_config}args = [\"config-revision\"]\n"),
    )
    .unwrap();
    wait(&|| count("describe") >= 3);
    assert_eq!(count("describe"), 3);
    // This fixture requires provider-owned user provenance, not arbitrary role.
    let record = json!({"timestamp":"2026-09-07T12:00:02Z","type":"response_item",
        "payload":{"type":"message","role":"user", "internal_chat_message_metadata_passthrough":{"content_item_kinds":["user.text"]},
        "content":[{"type":"input_text","text":p.f.envelope}]}});
    writeln!(
        fs::OpenOptions::new()
            .append(true)
            .open(&p.transcript)
            .unwrap(),
        "{record}"
    )
    .unwrap();
    wait(&|| {
        p.f.db
            .delivery_observation_confirmation(&p.f.attempt)
            .unwrap()
            .is_some()
    });
    drop(first);
    drop(second);
    assert_eq!(count("describe"), 3);
    assert!(p.f.db.list_pending(SESSION).unwrap().is_empty());
    p.assert_staging_untouched();
}

fn prepare_correction3_helper(p: &Paired, executable: &Path) -> PathBuf {
    let root = p.f.root.path();
    let config_home = root.join("helper-config");
    let config = config_home.join("oulipoly-agent-runner");
    fs::create_dir_all(&config).unwrap();
    let discovered = profile(root);
    fs::write(config.join("providers.toml"), format!(
        "[account]\nsettings_id = {:?}\nimplementation = {{ family = {:?}, executable = {:?} }}\n",
        discovered.settings_id, discovered.family, executable.display().to_string()
    )).unwrap();
    config_home
}

fn correction3_helper_command(
    root: &Path,
    config_home: &Path,
    once: bool,
) -> std::process::Command {
    let binary = std::env::var("AGE355_RUNNER_BINARY").expect("explicit source-built Runner");
    let mut command = std::process::Command::new(binary);
    command.arg(crate::native_receipt::helper::ARG);
    if once {
        command.arg("once");
    }
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", root.join("home"))
        .env("OULIPOLY_DATA_DIR", root)
        .env("OULIPOLY_CONFIG_HOME", config_home)
        .env("XDG_CONFIG_HOME", config_home)
        .env("XDG_DATA_HOME", root.join("data"))
        .env("TMPDIR", root);
    command
}

struct ReceiptRouteTestGuard(Option<std::ffi::OsString>);
impl Drop for ReceiptRouteTestGuard {
    fn drop(&mut self) {
        crate::native_receipt::helper::TEST_COMMAND.with_borrow_mut(|c| *c = None);
        crate::native_receipt::helper::TEST_BOUND.set(None);
        unsafe {
            match self.0.as_ref() {
                Some(value) => std::env::set_var("OULIPOLY_DATA_DIR", value),
                None => std::env::remove_var("OULIPOLY_DATA_DIR"),
            }
        }
    }
}

// Calls the actual production resume startup / terminal observation functions.
// Only the helper executable/env and watchdog duration are injected: config,
// preparation, hashing, scan admission and publication run in the real binary.
fn correction3_resume_route(p: &Paired, config_home: &Path, startup: bool) -> Result<bool, String> {
    correction3_resume_route_with(
        p,
        config_home,
        startup,
        confirm_mailbox_delivery_from_anchor,
    )
}

fn correction3_resume_route_with(
    p: &Paired,
    config_home: &Path,
    startup: bool,
    terminal: impl FnOnce(
        &ResumeAttemptInput<'_>,
        &oulipoly_config::ProviderConfig,
    ) -> Result<bool, String>,
) -> Result<bool, String> {
    use crate::migration_providers::ResumeExecutionEnvironment;
    let root = p.f.root.path();
    let services = crate::wiring::AgentRuntimeServices::production(crate::wiring::RuntimePaths {
        config_root: config_home.join("oulipoly-agent-runner"),
        models_dir: root.join("models"),
        agents_dir: root.join("agents"),
        data_root: root.into(),
        state_db_path: root.join("state.db"),
        lock_dir: root.join("locks"),
        working_dir: root.into(),
    })
    .unwrap();
    let mut resolved = oulipoly_state::ResolvedResume {
        chain_id: "chain".into(),
        active_session_id: SESSION.into(),
        active_provider: "account".into(),
        model_name: Some("offline-paired".into()),
        model: None,
    };
    if startup {
        reconcile_pending_headless_delivery_observations(
            &resolved,
            Path::new("/offline"),
            &config_home.join("oulipoly-agent-runner"),
        )?;
        return Ok(p
            .f
            .db
            .delivery_observation_confirmation(&p.f.attempt)?
            .is_some());
    }
    let env = ResumeExecutionEnvironment {
        state: oulipoly_state::StateDb::open(&root.join("state.db")).unwrap(),
        providers_cfg: ProvidersConfig {
            entries: HashMap::new(),
        },
        models: HashMap::new(),
        sessions_cfg: oulipoly_config::SessionsConfig {
            entries: HashMap::new(),
        },
        config_root: config_home.join("oulipoly-agent-runner"),
        models_dir: root.join("models"),
    };
    let mut zero_turn = crate::zero_turn_orchestration::ZeroTurnConfirmationState::new();
    let mut accepted = false;
    let seqs = [p.f.seq];
    let input = ResumeAttemptInput {
        agent_runtime_services: &services,
        env: &env,
        resolved: &mut resolved,
        answer: Some(&p.f.envelope),
        mailbox_session_id: SESSION,
        mailbox_delivery_seqs: &seqs,
        mailbox_delivery_nonce: Some(&p.f.attempt),
        mailbox_delivery_requires_turn_confirmation: true,
        manual_migrate: None,
        reservation: None,
        session_id: SESSION,
        working_dir: Some(root),
        attempts: 1,
        max_attempts: 1,
        parent_invocation_id: None,
        effective_spawn_cwd: Path::new("/offline"),
        zero_turn_confirmation: &mut zero_turn,
        provider_prompt_accepted: &mut accepted,
    };
    terminal(
        &input,
        &oulipoly_config::ProviderConfig::model_provider("account", vec![]),
    )
}

#[test]
#[ignore = "requires source-built Runner/provider; actual startup and terminal routes, offline only"]
fn age355_paired_correction3_startup_terminal_paths_are_owned_and_exact() {
    let _lock = crate::mailbox_delivery::DATA_DIR_ENV_LOCK.lock().unwrap();
    for startup in [true, false] {
        for mode in ["blocked", "locked", "exact"] {
            let blocked = mode == "blocked";
            let locked = mode == "locked";
            let mut p = Paired::new();
            p.anchor_and_submit();
            let root = p.f.root.path();
            let _guard = ReceiptRouteTestGuard(std::env::var_os("OULIPOLY_DATA_DIR"));
            unsafe {
                std::env::set_var("OULIPOLY_DATA_DIR", root);
            }
            let executable = if blocked {
                let path = root.join("blocked-preparation.py");
                fs::write(&path, format!("#!/usr/bin/python3\nimport os,time\nopen({:?},'w').write(str(os.getpid()))\ntime.sleep(30)\n", root.join("blocked-pid").display().to_string())).unwrap();
                fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
                path
            } else {
                p.proxy.clone()
            };
            let config_home = prepare_correction3_helper(&p, &executable);
            let owned_root = root.to_path_buf();
            let owned_config = config_home.clone();
            crate::native_receipt::helper::TEST_COMMAND.with_borrow_mut(|c| {
                *c = Some(Box::new(move |once| {
                    let mut command = correction3_helper_command(&owned_root, &owned_config, once);
                    command.env(
                        "OULIPOLY_CONFIG_HOME",
                        owned_root.join("not-selected-config"),
                    );
                    command
                }))
            });
            crate::native_receipt::helper::TEST_BOUND.set(Some(if blocked {
                Duration::from_millis(500)
            } else {
                Duration::from_secs(10)
            }));
            let record = json!({"timestamp":"2026-09-07T12:00:02Z","type":"response_item",
                "payload":{"type":"message","role":"user", "internal_chat_message_metadata_passthrough":{"content_item_kinds":["user.text"]},
                "content":[{"type":"input_text","text":p.f.envelope}]}});
            writeln!(
                fs::OpenOptions::new()
                    .append(true)
                    .open(&p.transcript)
                    .unwrap(),
                "{record}"
            )
            .unwrap();
            let _admission = if locked {
                crate::native_receipt::helper::try_admit(p.f.db.path(), "receipt-scan").unwrap()
            } else {
                None
            };
            let operations_before = fs::read_to_string(root.join("inspection-operations")).unwrap();
            let start = std::time::Instant::now();
            let result = correction3_resume_route(&p, &config_home, startup);
            if blocked || locked {
                assert!(
                    start.elapsed() < Duration::from_secs(3),
                    "startup={startup}: {result:?}"
                );
                if locked {
                    assert!(
                        !result.unwrap(),
                        "contention should complete without a watchdog error"
                    );
                } else {
                    assert!(!result.unwrap_or(false));
                }
                if blocked {
                    let pid: i32 = fs::read_to_string(root.join("blocked-pid"))
                        .unwrap()
                        .parse()
                        .unwrap();
                    assert_process_not_live(pid);
                } else {
                    assert_eq!(
                        fs::read_to_string(root.join("inspection-operations")).unwrap(),
                        operations_before,
                        "target inspected while global scan admission was held"
                    );
                }
                assert!(
                    p.f.db
                        .delivery_observation_confirmation(&p.f.attempt)
                        .unwrap()
                        .is_none()
                );
            } else {
                assert!(result.unwrap(), "startup={startup}");
                assert!(p.f.db.list_pending(SESSION).unwrap().is_empty());
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn assert_process_not_live(pid: i32) {
    let start = std::time::Instant::now();
    loop {
        let live = fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .is_some_and(|stat| {
                !matches!(
                    stat.rsplit_once(')')
                        .unwrap()
                        .1
                        .split_whitespace()
                        .next()
                        .unwrap(),
                    "Z" | "X"
                )
            });
        if !live {
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(8),
            "process {pid} remains live"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(not(target_os = "linux"))]
fn assert_process_not_live(pid: i32) {
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "private subprocess entry for parent-death test only"]
fn age355_correction3_private_supervisor() {
    let root = PathBuf::from(std::env::var_os("AGE355_ORPHAN_ROOT").unwrap());
    let mut command = correction3_helper_command(&root, &root.join("helper-config"), false);
    let _ = crate::native_receipt::helper::supervise(
        &mut command,
        &CancellationToken::new(),
        Duration::from_secs(10),
    );
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires source-built Runner/provider; parent death and TERM-resistant descendants"]
fn age355_paired_correction3_parent_death_heartbeat_and_timeout_kill_descendants() {
    for mode in ["heartbeat", "timeout", "self-exit"] {
        let timeout = mode == "timeout";
        let mut p = Paired::new();
        p.anchor_and_submit();
        p.f.db
            .wake_sessions()
            .upsert_session_metadata(oulipoly_state::mailbox::SessionMetadataUpsert {
                session_id: SESSION,
                mode: "headless",
                invocation_uuid: Some("native-invocation"),
                provider_name: Some("account"),
                model_name: Some("offline-paired"),
                models_dir: None,
                effective_cwd: Some("/offline"),
            })
            .unwrap();
        let root = p.f.root.path();
        let wrapper = root.join("orphan-provider.py");
        fs::write(&wrapper, format!(r#"#!/usr/bin/python3
import os,sys,subprocess,time,pathlib
root=pathlib.Path({root:?})
if sys.argv[1] == 'describe':
    child=subprocess.Popen(['/usr/bin/python3','-c',"import signal,time,os; signal.signal(signal.SIGTERM,signal.SIG_IGN); open("+repr(str(root/'descendant-pid'))+",'w').write(str(os.getpid())); time.sleep(30)"],stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
    (root/'helper-pid').write_text(str(os.getppid()))
    while not (root/'release').exists(): time.sleep(.005)
    if {timeout}: time.sleep(30)
os.execv({proxy:?},[{proxy:?}]+sys.argv[1:])
"#, root=root.display().to_string(), proxy=p.proxy.display().to_string(), timeout=if timeout {"True"} else {"False"})).unwrap();
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
        prepare_correction3_helper(&p, &wrapper);
        let mut supervisor = if mode == "self-exit" {
            use std::os::unix::process::CommandExt;
            let mut command = correction3_helper_command(root, &root.join("helper-config"), true);
            command
                .process_group(0)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null());
            let mut child = command.spawn().unwrap();
            child.stdin.take().unwrap().write_all(&[1]).unwrap();
            child
        } else {
            std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--ignored", "--exact", "run::resume::wake::observation_tests::paired_tests::age355_correction3_private_supervisor", "--nocapture"])
                .env("AGE355_ORPHAN_ROOT", root).stdout(std::process::Stdio::null()).spawn().unwrap()
        };
        let start = std::time::Instant::now();
        while !root.join("descendant-pid").exists() {
            if start.elapsed() > Duration::from_secs(5) {
                let _ = supervisor.kill();
                let _ = supervisor.wait();
                panic!("no descendant fixture");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let helper: i32 = fs::read_to_string(root.join("helper-pid"))
            .unwrap()
            .parse()
            .unwrap();
        struct Cleanup(i32);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                unsafe {
                    libc::kill(-self.0, libc::SIGKILL);
                }
            }
        }
        let _cleanup = Cleanup(helper);
        let descendant: i32 = fs::read_to_string(root.join("descendant-pid"))
            .unwrap()
            .parse()
            .unwrap();
        // Demonstrate resistance before removing the supervising parent.
        assert_eq!(unsafe { libc::kill(descendant, libc::SIGTERM) }, 0);
        if mode != "self-exit" {
            supervisor.kill().unwrap();
            supervisor.wait().unwrap();
        }
        fs::write(root.join("release"), "go").unwrap();
        assert_process_not_live(helper);
        assert_process_not_live(descendant);
        if mode == "self-exit" {
            // No OwnedHelper parent sent this signal. Successful entry itself
            // must discharge descendants before exiting, even with open stdout.
            use std::os::unix::process::ExitStatusExt;
            assert_eq!(supervisor.wait().unwrap().signal(), Some(libc::SIGKILL));
        }
        assert!(
            crate::native_receipt::helper::try_admit(p.f.db.path(), "receipt-owner")
                .unwrap()
                .is_some()
        );
    }
}

#[test]
#[ignore = "requires source-built provider; completed-checkpoint settings admission"]
fn age355_paired_correction3_changed_settings_cannot_publish_cached_match() {
    let mut p = Paired::new();
    p.anchor_and_submit();
    let endpoint = p.registry.preflight_account("account").unwrap();
    let reader = endpoint
        .client()
        .pinned_executable_identity_sha256()
        .unwrap();
    // A crash after checkpoint publication and before final CAS can leave this
    // complete match. Seed that state to isolate semantic admission, not parsing.
    let checkpoint = serde_json::to_string(&ObservationProgress {
        receipt_policy: 1,
        reader_identity: Some(reader),
        complete: true,
        matching_turns: 1,
        matching_turn_id: Some("retained-exact-match".into()),
        ..Default::default()
    })
    .unwrap();
    p.f.db
        .advance_delivery_observation_progress(&p.f.attempt, None, &checkpoint)
        .unwrap();
    let discovered = profile(p.f.root.path());
    let providers = ProvidersConfig {
        entries: HashMap::from([(
            "account".into(),
            ProviderEntry {
                implementation: Some(ProviderEndpointConfig {
                    family: discovered.family,
                    executable: p.proxy.display().to_string(),
                }),
                settings_id: Some("different-settings".into()),
                ..ProviderEntry::default()
            },
        )]),
    };
    let changed = ProviderRegistry::from_configs(
        &[],
        &providers,
        ProviderRegistryOptions::default()
            .with_config_root(p.f.root.path().join("config"))
            .with_data_root(p.f.root.path().join("data")),
    )
    .unwrap();
    let error = confirm_delivery_observation(
        &p.f.db,
        &p.f.attempt,
        &changed,
        identity(p.f.root.path()),
        Path::new("/offline"),
        &p.f.anchor,
    )
    .unwrap_err();
    assert!(
        error.contains("receipt endpoint identity changed"),
        "{error}"
    );
    assert!(
        p.f.db
            .delivery_observation_confirmation(&p.f.attempt)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        p.f.db
            .delivery_observation_progress(&p.f.attempt)
            .unwrap()
            .as_deref(),
        Some(checkpoint.as_str())
    );
    assert_eq!(
        p.f.db
            .delivery_observation_anchor(&p.f.attempt)
            .unwrap()
            .unwrap(),
        p.f.anchor
    );
}

fn correction4_install_target(p: &Paired, config_home: &Path) -> ReceiptRouteTestGuard {
    let guard = ReceiptRouteTestGuard(std::env::var_os("OULIPOLY_DATA_DIR"));
    let root = p.f.root.path().to_path_buf();
    unsafe {
        std::env::set_var("OULIPOLY_DATA_DIR", &root);
    }
    let config_home = config_home.to_path_buf();
    crate::native_receipt::helper::TEST_COMMAND.with_borrow_mut(|c| {
        *c = Some(Box::new(move |once| {
            correction3_helper_command(&root, &config_home, once)
        }))
    });
    // Long enough to distinguish prompt skipping from watchdog expiry. Tests
    // require the entire contended call to return in less than three seconds.
    crate::native_receipt::helper::TEST_BOUND.set(Some(Duration::from_secs(10)));
    guard
}

fn correction4_pending_snapshot(p: &Paired) -> Vec<(String, String, String, String)> {
    let conn = rusqlite::Connection::open(p.f.db.path()).unwrap();
    p.f.db.pending_delivery_observations(SESSION, 10).unwrap().iter().map(|pending| {
        let (started, state): (String, String) = conn.query_row(
            "SELECT submission_started_at, headless_submission_state FROM mailbox_delivery_attempts WHERE attempt_id = ?1 AND resolved_at IS NULL",
            [&pending.attempt_id], |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        (pending.attempt_id.clone(), started, state, crate::native_receipt::helper::anchor_identity(&pending.anchor))
    }).collect()
}

#[test]
#[ignore = "requires source-built Runner/provider; offline held-lock startup batch"]
fn age355_paired_correction4_startup_skips_all_contended_attempts_without_watchdogs() {
    let _lock = crate::mailbox_delivery::DATA_DIR_ENV_LOCK.lock().unwrap();
    let mut p = Paired::new();
    p.anchor_and_submit();
    for id in ["correction4-a", "correction4-b", "correction4-c"] {
        let seq = enqueue_additional(&mut p.f, id);
        p.f.db
            .register_headless_delivery_attempt(id, SESSION, None, id, &[seq], 0)
            .unwrap();
        p.f.db
            .record_delivery_observation_anchor(id, SESSION, &p.f.anchor)
            .unwrap();
        p.f.db
            .begin_headless_delivery_submission(id, SESSION, id, true)
            .unwrap();
    }
    let config_home = prepare_correction3_helper(&p, &p.proxy);
    let _guard = correction4_install_target(&p, &config_home);
    let admission = crate::native_receipt::helper::try_admit(p.f.db.path(), "receipt-scan")
        .unwrap()
        .unwrap();
    let before = correction4_pending_snapshot(&p);
    assert_eq!(before.len(), 4);
    let operations = fs::read_to_string(p.f.root.path().join("inspection-operations")).unwrap();
    let count = std::rc::Rc::new(std::cell::Cell::new(0));
    let calls = count.clone();
    crate::native_receipt::helper::TEST_COMMAND.with_borrow_mut(|c| {
        let factory = c.take().unwrap();
        *c = Some(Box::new(move |once| {
            calls.set(calls.get() + 1);
            factory(once)
        }));
    });
    let start = std::time::Instant::now();
    assert!(!correction3_resume_route(&p, &config_home, true).unwrap());
    assert!(
        start.elapsed() < Duration::from_secs(3),
        "startup waited on scan admission"
    );
    assert_eq!(
        count.get(),
        4,
        "all pending attempts must remain eligible for startup observation"
    );
    assert_eq!(correction4_pending_snapshot(&p), before);
    assert_eq!(p.f.db.list_pending(SESSION).unwrap().len(), 4);
    assert_eq!(
        fs::read_to_string(p.f.root.path().join("inspection-operations")).unwrap(),
        operations
    );
    assert!(p.f.submit().is_err());
    assert!(
        prepare_headless_resume_delivery_on(&mut p.f.db, SESSION, "chain", None, None).is_err()
    );
    drop(admission);
}

#[test]
#[ignore = "requires source-built Runner/provider; offline full terminal disposition and later exact settlement"]
fn age355_paired_correction4_contended_terminal_retains_pending_then_settles_exactly() {
    let _lock = crate::mailbox_delivery::DATA_DIR_ENV_LOCK.lock().unwrap();
    let mut p = Paired::new();
    p.anchor_and_submit();
    // The provider has already accepted the exact native input. Contention
    // prevents observing it; exit 1 must not be interpreted as non-submission.
    let record = json!({"timestamp":"2026-09-07T12:00:02Z","type":"response_item",
        "payload":{"type":"message","role":"user", "internal_chat_message_metadata_passthrough":{"content_item_kinds":["user.text"]},
        "content":[{"type":"input_text","text":p.f.envelope}]}});
    writeln!(
        fs::OpenOptions::new()
            .append(true)
            .open(&p.transcript)
            .unwrap(),
        "{record}"
    )
    .unwrap();
    let config_home = prepare_correction3_helper(&p, &p.proxy);
    let _guard = correction4_install_target(&p, &config_home);
    let admission = crate::native_receipt::helper::try_admit(p.f.db.path(), "receipt-scan")
        .unwrap()
        .unwrap();
    let before = correction4_pending_snapshot(&p);
    let operations = fs::read_to_string(p.f.root.path().join("inspection-operations")).unwrap();
    let mut invocation_id = String::new();
    let start = std::time::Instant::now();
    correction3_resume_route_with(&p, &config_home, false, |input, provider| {
        invocation_id =
            super::super::super::terminal::races_tests::correction4_unconfirmed_terminal(
                input, provider,
            );
        Ok(false)
    })
    .unwrap();
    assert!(
        start.elapsed() < Duration::from_secs(3),
        "terminal waited on scan admission"
    );
    assert_eq!(
        fs::read_to_string(p.f.root.path().join("inspection-operations")).unwrap(),
        operations
    );
    assert_eq!(correction4_pending_snapshot(&p), before);
    let rows = p.f.db.list_pending(SESSION).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].delivery_error.as_deref(),
        Some("mailbox_delivery_unconfirmed")
    );
    p.f.assert_pending_without_replay();
    assert!(p.f.submit().is_err());
    drop(admission);
    assert!(correction3_resume_route(&p, &config_home, true).unwrap());
    assert!(p.f.db.list_pending(SESSION).unwrap().is_empty());
    assert!(
        p.f.db
            .delivery_attempt_fully_settled(&p.f.attempt, SESSION, Some("chain"), &[p.f.seq])
            .unwrap()
    );
    let confirmation =
        p.f.db
            .delivery_observation_confirmation(&p.f.attempt)
            .unwrap()
            .unwrap();
    assert!(correction3_resume_route(&p, &config_home, false).unwrap());
    assert_eq!(
        p.f.db
            .delivery_observation_confirmation(&p.f.attempt)
            .unwrap()
            .unwrap(),
        confirmation
    );
    let invocation =
        p.f.state
            .get_invocation_by_uuid(&invocation_id)
            .unwrap()
            .unwrap();
    assert_eq!(invocation.success, Some(false));
    assert_eq!(invocation.exit_code, Some(1));
    assert_eq!(
        invocation.error_category.as_deref(),
        Some("mailbox_delivery_unconfirmed")
    );
    assert_eq!(p.f.submissions, 1);
    p.assert_staging_untouched();
}
