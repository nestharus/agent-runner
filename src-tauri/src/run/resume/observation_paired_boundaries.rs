//! Source-paired boundary regressions. Cursor strings are always opaque.
use super::*;
use oulipoly_provider::schemas::SchemaRegistry;
use oulipoly_state::SessionTurnPageBodyState;
use serde_json::Value;

#[derive(Clone, Copy)]
struct Budgets {
    source: u64,
    response: u64,
    inline: u64,
    projection: SessionProviderTurnProjection,
}
impl Default for Budgets {
    fn default() -> Self {
        Self {
            source: 1_048_576,
            response: 4096,
            inline: 65536,
            projection: SessionProviderTurnProjection::UserObservation,
        }
    }
}

fn read(
    p: &Paired,
    cursor: SessionProviderPageCursor,
    index: u64,
    sequence: u64,
    budgets: Budgets,
) -> Result<SessionProviderReadPageResult, String> {
    let cancellation = CancellationToken::new();
    read_turn_page(SessionProviderReadPageRequest {
        registry: &p.registry,
        identity: identity(),
        session_id: SESSION,
        effective_cwd: None,
        projection: budgets.projection,
        expected_delivery_nonce: (budgets.projection
            == SessionProviderTurnProjection::UserObservation)
            .then_some(p.f.attempt.as_str()),
        cursor,
        expected_page_index: index,
        expected_turn_sequence: sequence,
        max_turns: 10,
        max_source_bytes: budgets.source,
        max_response_bytes: budgets.response,
        max_inline_body_bytes: budgets.inline,
        cancellation: &cancellation,
        timeout: Duration::from_secs(5),
    })
    .map_err(|e| e.to_string())
}
fn beginning() -> SessionProviderPageCursor {
    SessionProviderPageCursor::Beginning { after_token: None }
}
fn continuation(page: &SessionProviderReadPageResult) -> SessionProviderPageCursor {
    SessionProviderPageCursor::Continuation {
        snapshot_id: page.snapshot_id.clone(),
        page_token: page.next_page_token.clone().unwrap(),
    }
}
fn raw(p: &Paired) -> Value {
    serde_json::from_slice(&fs::read(p.f.root.path().join("last-response.json")).unwrap()).unwrap()
}
fn append_raw(p: &Paired, bytes: &[u8]) {
    fs::OpenOptions::new()
        .append(true)
        .open(&p.transcript)
        .unwrap()
        .write_all(bytes)
        .unwrap();
}
fn assert_io(page: &SessionProviderReadPageResult, quantum: u64, metadata: u64) -> (u64, u64) {
    let declarations: Vec<_> = page
        .warnings
        .iter()
        .filter_map(|w| w.strip_prefix("codex_observation_io_v1:"))
        .collect();
    assert_eq!(declarations.len(), 1);
    let counts: Vec<u64> = declarations[0]
        .split(';')
        .map(|f| f.split_once('=').unwrap().1.parse().unwrap())
        .collect();
    assert_eq!(counts.len(), 3);
    assert_eq!(counts[2], metadata);
    assert!(counts[0].checked_add(counts[2]).unwrap() <= quantum);
    assert!(counts[1] < 8_388_608);
    assert_eq!(
        counts
            .iter()
            .try_fold(0u64, |a, b| a.checked_add(*b))
            .unwrap(),
        page.source_bytes_examined
    );
    assert!(page.source_bytes_examined <= 16_777_215);
    (counts[0], counts[1])
}

#[test]
#[ignore = "requires explicit freshly source-built candidate provider; offline only"]
fn age347_paired_actual_ceiling_page_passes_embedded_gate_and_canonical_neighbor_fails() {
    for record_size in [8_388_608, 8_388_609] {
        let p = Paired::new();
        p.set_mode("unaltered");
        // Same exact 134-byte identity record as the provider's ceiling control.
        fs::write(&p.transcript, format!("{}\n", json!({"timestamp":"2026-09-04T12:00:00Z","type":"session_meta","payload":{"id":SESSION,"cwd":"/workspace"}}))).unwrap();
        assert_eq!(fs::metadata(&p.transcript).unwrap().len(), 134);
        let mut record = json!({"type":"compacted","payload":{"padding":""}});
        let overhead = record.to_string().len() + 1;
        record["payload"]["padding"] = json!("x".repeat(record_size - overhead));
        append_raw(&p, format!("{record}\n").as_bytes());
        p.append("user", &"z".repeat(12000));
        let mut cursor = beginning();
        let mut ceiling = false;
        let mut terminal = false;
        for index in 0..12 {
            match read(&p, cursor, index, 0, Budgets::default()) {
                Ok(page) => {
                    let (forward, reconstruction) = assert_io(&page, 1_048_576, 134);
                    let envelope = raw(&p);
                    SchemaRegistry::new()
                        .validate_response("session.read_turns", &envelope)
                        .unwrap();
                    assert!(
                        fs::metadata(p.f.root.path().join("last-response.json"))
                            .unwrap()
                            .len()
                            <= 4096
                    );
                    if page.source_bytes_examined > 8_388_608 {
                        assert_eq!(page.source_bytes_examined, 8_388_742);
                        assert_eq!((forward, reconstruction), (1072, 8_387_536));
                        ceiling = true;
                        let mut canonical = envelope;
                        canonical["result"]["turn_projection"] = json!("canonical_ingest");
                        canonical["result"]["warnings"] = json!([]);
                        assert!(
                            SchemaRegistry::new()
                                .validate_response("session.read_turns", &canonical)
                                .is_err()
                        );
                        println!(
                            "actual paired ceiling record={record_size}; total=8388742; forward={forward}; reconstruction={reconstruction}; metadata=134; embedded schema and production read_turn_page accepted; canonical neighbor rejected"
                        );
                    }
                    if page.snapshot_complete {
                        assert_eq!(record_size, 8_388_608);
                        assert_eq!(page.turns.len(), 1);
                        assert_eq!(
                            page.turns[0].body_state,
                            SessionTurnPageBodyState::OmittedOversize
                        );
                        terminal = true;
                        break;
                    }
                    assert!(page.turns.is_empty());
                    cursor = continuation(&page);
                }
                Err(e) => {
                    assert_eq!(record_size, 8_388_609, "{e}");
                    assert!(e.contains("session_turn_record_ceiling_exceeded"), "{e}");
                    terminal = true;
                    break;
                }
            }
        }
        assert!(terminal);
        assert_eq!(ceiling, record_size == 8_388_608);
        p.assert_staging_untouched();
    }
}

#[test]
#[ignore = "requires explicit freshly source-built candidate provider; offline only"]
fn age347_paired_fallback_discovery_replay_remains_exact_for_both_projections() {
    for projection in [
        SessionProviderTurnProjection::UserObservation,
        SessionProviderTurnProjection::CanonicalIngest,
    ] {
        for fallback in [false, true] {
            let mut p = Paired::new();
            p.set_mode("unaltered");
            if projection == SessionProviderTurnProjection::CanonicalIngest {
                // This fixture owns this sparse admission sentinel; canonical
                // controls use available capacity, never production staging.
                fs::remove_file(p.staging.join("unrelated-retained-sparse")).unwrap();
            }
            if fallback {
                let target = p
                    .transcript
                    .with_file_name("rollout-nonstandard-target.jsonl");
                fs::rename(&p.transcript, &target).unwrap();
                p.transcript = target;
            }
            let metadata = fs::metadata(&p.transcript).unwrap().len();
            p.append("user", &"x".repeat(1800));
            let original = fs::read(&p.transcript).unwrap();
            let budgets = Budgets {
                source: 512,
                response: 8192,
                projection,
                ..Budgets::default()
            };
            let first = read(&p, beginning(), 0, 0, budgets).unwrap();
            let cursor = continuation(&first);
            let before = read(&p, cursor.clone(), 1, 0, budgets).unwrap();
            let unrelated = p.transcript.with_file_name("rollout-unrelated.jsonl");
            for padding in [0, 1000] {
                fs::write(&unrelated, format!("{}\n", json!({"type":"session_meta","payload":{"id":"unrelated-session","padding":"z".repeat(padding)}}))).unwrap();
                p.restart();
                let after = read(&p, cursor.clone(), 1, 0, budgets).unwrap();
                assert_eq!(before.page_digest, after.page_digest);
                if projection == SessionProviderTurnProjection::UserObservation {
                    assert_eq!(
                        assert_io(&after, 512, metadata),
                        (512 - metadata, 512 - metadata)
                    );
                } else {
                    assert_eq!(after.source_bytes_examined, 512);
                }
            }
            let mut cursor = cursor;
            let mut turns = Vec::new();
            let mut complete = false;
            for index in 1..10 {
                let page = read(&p, cursor, index, turns.len() as u64, budgets).unwrap();
                turns.extend(page.turns.clone());
                if page.snapshot_complete {
                    complete = true;
                    break;
                }
                cursor = continuation(&page);
            }
            assert!(complete);
            assert_eq!(turns.len(), 1);
            assert_eq!(turns[0].body.as_ref().unwrap()[0]["text"], "x".repeat(1800));
            assert!(turns[0].canonical_text_digest_verified);
            assert_eq!(fs::read(&p.transcript).unwrap(), original);
            if projection == SessionProviderTurnProjection::UserObservation {
                p.assert_staging_untouched();
            }
            println!(
                "paired fallback={fallback}; projection={projection:?}; unrelated discovery and restart replay exact; one complete turn"
            );
        }
    }
}

fn near_limit_fixture() -> (Paired, String) {
    let p = Paired::new();
    p.set_mode("unaltered");
    let header = fs::read(&p.transcript).unwrap();
    let (mut low, mut high) = (0, 4096);
    while low + 1 < high {
        let size = (low + high) / 2;
        fs::write(&p.transcript, &header).unwrap();
        p.append("user", &"x".repeat(size));
        let page = read(&p, beginning(), 0, 0, Budgets::default()).unwrap();
        if page.turns[0].body_state == SessionTurnPageBodyState::Inline {
            low = size;
        } else {
            high = size;
        }
    }
    assert!(low > 256);
    let text = "x".repeat(low - 32);
    fs::write(&p.transcript, header).unwrap();
    p.append("user", &text);
    let page = read(&p, beginning(), 0, 0, Budgets::default()).unwrap();
    assert_eq!(page.turns[0].body_state, SessionTurnPageBodyState::Inline);
    let bytes = fs::metadata(p.f.root.path().join("last-response.json"))
        .unwrap()
        .len();
    assert!(
        (4030..=4096).contains(&bytes),
        "near-limit neighbor bytes={bytes}"
    );
    (p, text)
}

#[test]
#[ignore = "requires explicit freshly source-built candidate provider; offline only"]
fn age347_paired_near_response_partial_trailer_preserves_turns_and_honest_reads() {
    for at_eof in [true, false] {
        let (mut p, text) = near_limit_fixture();
        let header = fs::read(&p.transcript)
            .unwrap()
            .iter()
            .position(|b| *b == b'\n')
            .unwrap() as u64
            + 1;
        let boundary = fs::metadata(&p.transcript).unwrap().len();
        let record = format!(
            "{}\n",
            json!({"timestamp":"2026-09-04T12:00:02Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"next notification"}]}})
        );
        let split = record.len() / 2;
        append_raw(
            &p,
            if at_eof {
                &record.as_bytes()[..split]
            } else {
                record.as_bytes()
            },
        );
        let budgets = Budgets {
            source: boundary + split as u64,
            ..Budgets::default()
        };
        let page = read(&p, beginning(), 0, 0, budgets).unwrap();
        let response_bytes = fs::metadata(p.f.root.path().join("last-response.json"))
            .unwrap()
            .len();
        assert!(response_bytes <= 4096);
        assert_eq!(page.snapshot_complete, at_eof);
        assert!(!page.source_final);
        assert_eq!(page.turns.len(), 1);
        assert_eq!(page.turns[0].turn_id, format!("{SESSION}:byte:{header}"));
        assert_eq!(
            page.turns[0].canonical_text_sha256.as_deref(),
            Some(normalized_text_sha256(&text).as_str())
        );
        assert_eq!(
            page.turns[0].body_state,
            SessionTurnPageBodyState::OmittedOversize
        );
        let body = format!("[{{\"type\":\"text\",\"text\":\"{text}\"}}]");
        use sha2::{Digest, Sha256};
        assert_eq!(page.turns[0].body_bytes, Some(body.len() as u64));
        assert_eq!(
            page.turns[0].body_sha256.as_deref(),
            Some(format!("{:x}", Sha256::digest(body.as_bytes())).as_str())
        );
        assert_eq!(
            assert_io(&page, budgets.source, header),
            (budgets.source - header, 0)
        );
        p.restart();
        assert_eq!(
            read(&p, beginning(), 0, 0, budgets).unwrap().page_digest,
            page.page_digest
        );
        let roomy = read(
            &p,
            beginning(),
            0,
            0,
            Budgets {
                response: 8192,
                ..budgets
            },
        )
        .unwrap();
        assert_eq!(roomy.turns[0].body.as_ref().unwrap()[0]["text"], text);
        let (cursor, index, sequence) = if at_eof {
            let resume = SessionProviderPageCursor::Beginning {
                after_token: page.resume_token.clone(),
            };
            let unfinished = read(&p, resume.clone(), 0, 0, budgets).unwrap();
            assert!(unfinished.turns.is_empty());
            assert_eq!(
                assert_io(&unfinished, budgets.source, header),
                (0, split as u64)
            );
            append_raw(&p, &record.as_bytes()[split..]);
            (resume, 0, 0)
        } else {
            (continuation(&page), 1, 1)
        };
        let second = read(&p, cursor.clone(), index, sequence, budgets).unwrap();
        assert_eq!(second.turns.len(), 1);
        assert!(second.snapshot_complete);
        assert_eq!(
            second.turns[0].turn_id,
            format!("{SESSION}:byte:{boundary}")
        );
        assert_eq!(
            second.turns[0].body.as_ref().unwrap()[0]["text"],
            "next notification"
        );
        assert_eq!(
            assert_io(&second, budgets.source, header),
            ((record.len() - split) as u64, split as u64)
        );
        p.restart();
        assert_eq!(
            read(&p, cursor, index, sequence, budgets)
                .unwrap()
                .page_digest,
            second.page_digest
        );
        p.assert_staging_untouched();
        println!(
            "paired partial-trailer eof={at_eof}; response_bytes={response_bytes}; complete turn digest preserved; unfinished record not observed; next turn exactly once and restart replay exact"
        );
    }
}

#[test]
#[ignore = "requires explicit freshly source-built candidate provider; offline only"]
fn age347_paired_request_context_not_warning_identifies_observation() {
    let p = Paired::new();
    // A real observation response (including its valid accounting) cannot turn
    // the original canonical request into observation authority.
    p.set_mode("observation_request");
    let err = read(
        &p,
        beginning(),
        0,
        0,
        Budgets {
            projection: SessionProviderTurnProjection::CanonicalIngest,
            ..Budgets::default()
        },
    )
    .unwrap_err();
    assert!(err.contains("provider_page_identity_mismatch"), "{err}");
    // Warning-free ordinary observation remains compatible at the original
    // quota, just as other unchanged providers require. This is a mutation of a
    // real tail page, not an assertion that this provider omits its declaration.
    p.set_mode("missing");
    let ordinary = p.read(SessionProviderPageCursor::Tail, 0, 0).unwrap();
    assert!(ordinary.source_bytes_examined <= 512 && ordinary.warnings.is_empty());
    let cancellation = CancellationToken::new();
    for nonce in [None, Some("invalid")] {
        let err = read_turn_page(SessionProviderReadPageRequest {
            registry: &p.registry,
            identity: identity(),
            session_id: SESSION,
            effective_cwd: None,
            projection: SessionProviderTurnProjection::UserObservation,
            expected_delivery_nonce: nonce,
            cursor: SessionProviderPageCursor::Tail,
            expected_page_index: 0,
            expected_turn_sequence: 0,
            max_turns: 10,
            max_source_bytes: 512,
            max_response_bytes: 4096,
            max_inline_body_bytes: 0,
            cancellation: &cancellation,
            timeout: Duration::from_secs(5),
        })
        .unwrap_err()
        .to_string();
        assert!(err.contains("session_observation_nonce_invalid"), "{err}");
    }
    println!(
        "paired context: canonical request rejects real observation response despite valid declaration; missing/invalid request nonce rejects; ordinary warning-free tail retains original quota"
    );
}

#[test]
#[ignore = "requires explicit freshly source-built candidate provider; offline only"]
fn age347_paired_metadata_only_boundary_charges_uncheckpointed_suffix() {
    let mut p = Paired::new();
    p.set_mode("unaltered");
    let header = fs::metadata(&p.transcript).unwrap().len();
    p.append("user", &"x".repeat(128));
    let boundary = fs::metadata(&p.transcript).unwrap().len();
    p.append("user", &"y".repeat(1200));
    let mut budgets = Budgets {
        source: boundary,
        inline: 0,
        ..Budgets::default()
    };
    let (mut low, mut high) = (1023, 4096);
    // Find the smallest envelope that holds the complete metadata-only neighbor,
    // then allow only decimal counter growth, not an extra partial-prefix token.
    while low + 1 < high {
        let size = (low + high) / 2;
        budgets.response = size;
        match read(&p, beginning(), 0, 0, budgets) {
            Ok(_) => high = size,
            Err(e) => {
                assert!(e.contains("session_turn_page_budget_too_small"), "{e}");
                low = size;
            }
        }
    }
    budgets.response = high + 32;
    let control = read(&p, beginning(), 0, 0, budgets).unwrap();
    assert_eq!(control.turns.len(), 1);
    assert!(!control.snapshot_complete);
    budgets.source += 80;
    let page = read(&p, beginning(), 0, 0, budgets).unwrap();
    assert_eq!(page.turns.len(), 1);
    assert_eq!(page.turns[0].turn_id, control.turns[0].turn_id);
    assert_eq!(page.turns[0].body_sha256, control.turns[0].body_sha256);
    assert!(!page.snapshot_complete);
    assert_eq!(
        assert_io(&page, budgets.source, header),
        (budgets.source - header, 0)
    );
    assert!(
        fs::metadata(p.f.root.path().join("last-response.json"))
            .unwrap()
            .len()
            <= budgets.response
    );
    p.restart();
    assert_eq!(
        read(&p, beginning(), 0, 0, budgets).unwrap().page_digest,
        page.page_digest
    );
    let mut cursor = continuation(&page);
    let mut seen = Vec::new();
    let mut complete = false;
    for index in 1..10 {
        let next = read(&p, cursor.clone(), index, 1 + seen.len() as u64, budgets).unwrap();
        let (_, reconstruction) = assert_io(&next, budgets.source, header);
        if index == 1 {
            assert_eq!(reconstruction, 0, "charged suffix was not checkpointed");
        }
        assert_eq!(
            read(&p, cursor, index, 1 + seen.len() as u64, budgets)
                .unwrap()
                .page_digest,
            next.page_digest
        );
        assert!(
            fs::metadata(p.f.root.path().join("last-response.json"))
                .unwrap()
                .len()
                <= budgets.response
        );
        seen.extend(next.turns.clone());
        if next.snapshot_complete {
            complete = true;
            break;
        }
        cursor = continuation(&next);
    }
    assert!(complete);
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].turn_id, format!("{SESSION}:byte:{boundary}"));
    assert_eq!(
        seen[0].canonical_text_sha256.as_deref(),
        Some(normalized_text_sha256(&"y".repeat(1200)).as_str())
    );
    p.assert_staging_untouched();
    println!(
        "paired metadata-only fallback: forward includes 80-byte uncheckpointed suffix; next reconstruction=0; both turns retained once; bounded response and replay preserved"
    );
}
