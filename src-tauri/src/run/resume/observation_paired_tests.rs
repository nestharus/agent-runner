//! Opt-in offline pairing: AGE347_PROVIDER_BINARY must name a source-built
//! candidate, never an installed provider. The proxy admits only describe and
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
        f.anchor.provider_instance_id = "codex-instance".into();
        f.anchor.settings_id = "codex".into();
        let native = f.root.path().join("home/.codex/sessions/2026/09/07");
        fs::create_dir_all(&native).unwrap();
        let transcript = native.join(format!("rollout-offline-{SESSION}.jsonl"));
        fs::write(&transcript, format!("{}\n", json!({"timestamp":"2026-09-07T12:00:00Z","type":"session_meta","payload":{"id":SESSION,"cwd":"/offline"}}))).unwrap();
        let mode = f.root.path().join("mode");
        fs::write(&mode, "normal").unwrap();
        let proxy = write_proxy(f.root.path(), &binary);
        let staging = f
            .root
            .path()
            .join("data/provider-state/codex/session-pages-v1");
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
        read_paired(&self.registry, &self.f.attempt, cursor, index, sequence)
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
            .join("data/provider-state/codex/observation-auth-v1");
        assert_eq!(fs::read_dir(&key_dir).unwrap().count(), 1);
        assert_eq!(fs::metadata(key_dir.join("key")).unwrap().len(), 32);
    }
}

fn identity() -> SessionProviderIdentity {
    SessionProviderIdentity {
        model_name: "offline-paired".into(),
        provider_name: "account".into(),
        provider_instance_id: Some("codex-instance".into()),
        settings_id: "codex".into(),
    }
}

fn registry(root: &Path, proxy: &Path) -> ProviderRegistry {
    let providers = ProvidersConfig {
        entries: HashMap::from([(
            "account".into(),
            ProviderEntry {
                implementation: Some(ProviderEndpointConfig {
                    family: "codex".into(),
                    executable: proxy.display().to_string(),
                }),
                settings_id: Some("codex".into()),
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
    registry: &ProviderRegistry,
    nonce: &str,
    cursor: SessionProviderPageCursor,
    index: u64,
    sequence: u64,
) -> Result<SessionProviderReadPageResult, String> {
    let cancellation = CancellationToken::new();
    read_turn_page(SessionProviderReadPageRequest {
        registry,
        identity: identity(),
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
    .map_err(|err| err.to_string())
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
            identity: identity(),
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
            identity(),
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
            let result = read_paired(&registry, &pending.attempt_id, c.clone(), i, s)?;
            if calls == 1 {
                assert_eq!(i, 16);
                let replay = read_paired(&registry, &pending.attempt_id, c, i, s)?;
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
