use super::*;
use fs4::FileExt;

fn write_fresh_join(socket: &mut UnixStream) {
    socket
        .write_all(b"join!\n{\"protocol\":\"root-authority-v1\",\"mode\":{\"kind\":\"fresh\"}}\n")
        .unwrap();
}

fn fresh_join_request(socket: UnixStream, context: SourceProcessIdentity) -> ControlRequest {
    ControlRequest::Join(JoinRequest {
        socket,
        context,
        request: super::original_work::RootJoinRequest {
            protocol: super::original_work::ROOT_PROTOCOL.into(),
            mode: super::original_work::RootJoinMode::Fresh,
        },
    })
}

fn root_join_response(owner: &CompletionDomainOwner) -> super::original_work::RootJoinResponse {
    super::original_work::RootJoinResponse {
        owner: owner.clone(),
        root_authority: super::original_work::RootAuthorityGrant {
            control_protocol: super::original_work::SOURCE_CONTROL_PROTOCOL.into(),
            protocol: super::original_work::ROOT_PROTOCOL.into(),
            completion_protocol: owner.protocol.clone(),
            domain_id: owner.domain_id.clone(),
            supervisor_authority_id: owner.supervisor_authority_id.clone(),
            root_id: "test-root".into(),
            capability: "test-capability".into(),
            root_identity: owner.guardian_identity.clone(),
            guardian_identity: owner.guardian_identity.clone(),
        },
    }
}

#[test]
fn guardian_readiness_is_not_failed_by_the_retired_five_second_cap() {
    let (mut parent, mut guardian) = UnixStream::pair().unwrap();
    let writer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(5_100));
        guardian.write_all(&[1]).unwrap();
        let identity = identity(i64::from(std::process::id())).unwrap();
        let grant = super::original_work::RootAuthorityGrant {
            control_protocol: super::original_work::SOURCE_CONTROL_PROTOCOL.into(),
            protocol: super::original_work::ROOT_PROTOCOL.into(),
            completion_protocol: PROTOCOL.into(),
            domain_id: "readiness-test".into(),
            supervisor_authority_id: "readiness-authority".into(),
            root_id: "readiness-root".into(),
            capability: "readiness-capability".into(),
            root_identity: identity.clone(),
            guardian_identity: identity,
        };
        serde_json::to_writer(&mut guardian, &grant).unwrap();
        guardian.write_all(b"\n").unwrap();
    });
    await_guardian_ready(&mut parent, 42).unwrap();
    writer.join().unwrap();
}

#[test]
fn closing_admission_waits_for_election_release_before_first_hello() {
    let root = tempfile::tempdir().unwrap();
    let endpoint = root.path().join("owner.sock");
    let retiring = admission_gate(&endpoint).unwrap();
    let election = std::fs::File::create(root.path().join("election.lock")).unwrap();
    FileExt::lock(&election).unwrap();
    assert!(try_begin_retirement(&retiring, || true).unwrap());
    let incoming = admission_gate(&endpoint).unwrap();
    assert!(matches!(
        FileExt::try_lock(&incoming),
        Err(fs4::TryLockError::WouldBlock)
    ));
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (hello_tx, hello_rx) = std::sync::mpsc::channel();
    let next_endpoint = endpoint.clone();
    let entrant = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        FileExt::lock(&incoming).unwrap();
        let next_election = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(next_endpoint.with_file_name("election.lock"))
            .unwrap();
        FileExt::try_lock(&next_election).unwrap();
        // First contact is deferred by ownership coordination, not retried
        // after an empty hello. No provider has been dispatched.
        hello_tx.send(()).unwrap();
    });
    started_rx.recv().unwrap();
    assert!(matches!(
        hello_rx.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    // This barrier models the guardian's ECHILD branch, not a timed delay.
    drop(election);
    drop(retiring);
    hello_rx.recv().unwrap();
    entrant.join().unwrap();
}

#[test]
fn hello_to_join_gate_prevents_idle_close_and_releases_on_failed_close() {
    let root = tempfile::tempdir().unwrap();
    let endpoint = root.path().join("owner.sock");
    let entry = admission_gate(&endpoint).unwrap();
    let guardian = admission_gate(&endpoint).unwrap();
    FileExt::lock(&entry).unwrap();
    let listener = UnixListener::bind(&endpoint).unwrap();
    let id = identity(i64::from(std::process::id())).unwrap();
    let owner = CompletionDomainOwner {
        protocol: PROTOCOL.into(),
        domain_id: "boundary".into(),
        supervisor_authority_id: "11111111-1111-4111-8111-111111111111".into(),
        owner_generation: "generation".into(),
        guardian_identity: id.clone(),
        driver_identity: id,
        endpoint: endpoint.to_string_lossy().into_owned(),
    };
    let expected = owner.clone();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut bytes = [0; 6];
        socket.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"hello\n");
        serde_json::to_writer(&mut socket, &owner).unwrap();
        drop(socket);
        let (mut socket, _) = listener.accept().unwrap();
        socket.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"join!\n");
        serde_json::to_writer(&mut socket, &root_join_response(&owner)).unwrap();
        socket.write_all(b"\n").unwrap();
        socket
    });
    assert_eq!(hello(&endpoint).unwrap().owner_generation, "generation");
    // Deterministically attempt retirement in the unreserved hello-to-join
    // interval. The actual close transaction must not even be invoked.
    assert!(!try_begin_retirement(&guardian, || panic!("closed before join")).unwrap());
    let context = connect_context(&endpoint, &expected).unwrap();
    let retained_peer = server.join().unwrap();
    drop(entry);
    // Once joined, the existing context/State obligation fence determines
    // idleness; the admission gate alone does not authorize retirement.
    let mut close_called = false;
    assert!(
        !try_begin_retirement(&guardian, || {
            close_called = true;
            false
        })
        .unwrap()
    );
    assert!(
        close_called,
        "failed-close callback never acquired admission; competing entry description may still be held"
    );
    let next = admission_gate(&endpoint).unwrap();
    FileExt::try_lock(&next).unwrap();
    drop((context, retained_peer));
}

#[test]
fn hello_still_rejects_invalid_owner_and_empty_response() {
    for invalid in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let endpoint = root.path().join("owner.sock");
        let listener = UnixListener::bind(&endpoint).unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0; 6];
            socket.read_exact(&mut request).unwrap();
            if invalid {
                let mut id = identity(i64::from(std::process::id())).unwrap();
                id.starttime_ticks += 1;
                let owner = CompletionDomainOwner {
                    protocol: PROTOCOL.into(),
                    domain_id: "invalid".into(),
                    supervisor_authority_id: "22222222-2222-4222-8222-222222222222".into(),
                    owner_generation: "invalid".into(),
                    guardian_identity: id.clone(),
                    driver_identity: id,
                    endpoint: "invalid".into(),
                };
                serde_json::to_writer(&mut socket, &owner).unwrap();
            }
        });
        let error = hello(&endpoint).unwrap_err();
        assert!(
            if invalid {
                error.contains("identity mismatch")
            } else {
                error.contains("EOF")
            },
            "{error}"
        );
        server.join().unwrap();
    }
}

// Independent intent: hello is liveness, not a durable admission ACK. A join
// whose persistence has not run must remain pending even while hello succeeds.
#[test]
fn responsive_hello_does_not_ack_pending_join_and_pause_preserves_it() {
    let root = tempfile::tempdir().unwrap();
    let endpoint = root.path().join("owner.sock");
    let listener = UnixListener::bind(&endpoint).unwrap();
    listener.set_nonblocking(true).unwrap();
    let id = identity(i64::from(std::process::id())).unwrap();
    let owner = CompletionDomainOwner {
        protocol: PROTOCOL.into(),
        domain_id: "control-only-not-admission".into(),
        supervisor_authority_id: "33333333-3333-4333-8333-333333333333".into(),
        owner_generation: "control-only-generation".into(),
        guardian_identity: id.clone(),
        driver_identity: id.clone(),
        endpoint: endpoint.to_string_lossy().into_owned(),
    };
    let service = ControlService::start(&listener, &owner).unwrap();
    let mut joining = UnixStream::connect(&endpoint).unwrap();
    write_fresh_join(&mut joining);
    assert_eq!(
        hello(&endpoint).unwrap().owner_generation,
        owner.owner_generation
    );
    service.pause().unwrap();
    let queued = service.pending();
    assert_eq!(queued.len(), 1);
    assert!(matches!(&queued[0], ControlRequest::Join(request) if request.context == id));
    joining.set_nonblocking(true).unwrap();
    assert_eq!(
        joining.read(&mut [0]).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert_eq!(hello(&endpoint).unwrap().domain_id, owner.domain_id);
    let mut refused = UnixStream::connect(&endpoint).unwrap();
    write_fresh_join(&mut refused);
    assert_eq!(
        hello(&endpoint).unwrap().owner_generation,
        owner.owner_generation
    );
    let mut negative = String::new();
    refused.read_to_string(&mut negative).unwrap();
    let negative: JoinRefusal = serde_json::from_str(&negative).unwrap();
    assert!(matches!(
        negative.completion_join_refusal,
        RefusalReason::Retiring
    ));
    assert!(service.pending().is_empty());
    service.resume().unwrap();
    assert_eq!(hello(&endpoint).unwrap().domain_id, owner.domain_id);
    service.close().unwrap();
    assert!(hello(&endpoint).is_err());
    assert!(service.stop().unwrap().is_empty());
    assert_eq!(
        joining.read(&mut [0]).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    drop(queued);
    assert_eq!(joining.read(&mut [0]).unwrap(), 0);
}

pub(super) fn test_owner(endpoint: &Path) -> CompletionDomainOwner {
    let id = identity(i64::from(std::process::id())).unwrap();
    CompletionDomainOwner {
        protocol: PROTOCOL.into(),
        domain_id: "control-test-domain".into(),
        supervisor_authority_id: "44444444-4444-4444-8444-444444444444".into(),
        owner_generation: "control-test-generation".into(),
        guardian_identity: id.clone(),
        driver_identity: id,
        endpoint: endpoint.to_string_lossy().into_owned(),
    }
}

// Root C4: versioned successes, fixed nonsecret negatives and fail-closed
// identity/framing. No negative result proves no prior side effect or replay.
#[test]
fn join_refusal_compatibility_and_identity_boundaries() {
    for case in [
        "success",
        "queue",
        "retiring",
        "persistence",
        "generation",
        "domain",
        "peer",
        "unknown",
        "malformed",
    ] {
        let root = tempfile::tempdir().unwrap();
        let endpoint = root.path().join("owner.sock");
        let listener = UnixListener::bind(&endpoint).unwrap();
        let expected = test_owner(&endpoint);
        let owner = expected.clone();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0; 6];
            socket.read_exact(&mut request).unwrap();
            assert_eq!(&request, b"join!\n");
            if case == "success" {
                serde_json::to_writer(&mut socket, &root_join_response(&owner)).unwrap();
                socket.write_all(b"\n").unwrap();
                return;
            }
            let reason = match case {
                "retiring" => RefusalReason::Retiring,
                "persistence" => RefusalReason::Persistence,
                _ => RefusalReason::QueueFull,
            };
            let refusal = JoinRefusal::new(&owner, reason);
            let mut value = serde_json::to_value(refusal).unwrap();
            match case {
                "generation" => value["owner_generation"] = "other".into(),
                "domain" => value["domain_id"] = "other".into(),
                "peer" => value["guardian_identity"]["starttime_ticks"] = 0.into(),
                "unknown" => value["completion_join_refusal"] = "unknown_code".into(),
                "malformed" => value["unexpected"] = true.into(),
                _ => {}
            }
            let bytes = serde_json::to_vec(&value).unwrap();
            // Old client parsing boundary: negatives cannot be owner success.
            assert!(serde_json::from_slice::<CompletionDomainOwner>(&bytes).is_err());
            assert!(!value.as_object().unwrap().contains_key("endpoint"));
            socket.write_all(&bytes).unwrap();
            socket.write_all(b"\n").unwrap();
        });
        let result = connect_context(&endpoint, &expected);
        if case == "success" {
            assert!(result.is_ok(), "{result:?}");
        } else {
            let error = result.unwrap_err();
            let valid = matches!(case, "queue" | "retiring" | "persistence");
            assert_eq!(
                error.contains("admission outcome uncertain; no replay authorized"),
                valid,
                "{case}: {error}"
            );
            if valid {
                let category = match case {
                    "queue" => "queue full",
                    "retiring" => "retirement pause",
                    _ => "persistence failure",
                };
                assert!(error.contains(category), "{error}");
            }
        }
        server.join().unwrap();
    }
}

#[test]
fn identity_refusal_codes_keep_fresh_root_and_image_guards_closed() {
    fn refusal(
        path: &Path,
        owner: &CompletionDomainOwner,
        contexts: &mut ContextLeases,
        authorities: &mut super::original_work::RootAuthorities,
        supervisor: &super::root_supervisor::RootSupervisor,
        context: SourceProcessIdentity,
        protocol: &str,
    ) -> JoinRefusal {
        let (server, mut client) = UnixStream::pair().unwrap();
        retain_pending_context(
            path,
            owner,
            contexts,
            authorities,
            supervisor,
            JoinRequest {
                socket: server,
                context,
                request: super::original_work::RootJoinRequest {
                    protocol: protocol.into(),
                    mode: super::original_work::RootJoinMode::Fresh,
                },
            },
        );
        let mut frame = Vec::new();
        loop {
            let mut byte = [0];
            client.read_exact(&mut byte).unwrap();
            if byte == [b'\n'] {
                break;
            }
            frame.push(byte[0]);
        }
        assert!(serde_json::from_slice::<CompletionDomainOwner>(&frame).is_err());
        assert!(!String::from_utf8_lossy(&frame).contains("capability"));
        serde_json::from_slice(&frame).unwrap()
    }

    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("pid-identity.db");
    let owner = test_owner(&root.path().join("owner.sock"));
    let mut contexts = ContextLeases::inherit(&path).unwrap();
    let (driver, _driver_peer) = UnixStream::pair().unwrap();
    let supervisor = super::root_supervisor::RootSupervisor::new(&path, driver).unwrap();
    let mut authorities = super::original_work::RootAuthorities::default();
    let self_identity = owner.guardian_identity.clone();

    let wrong_protocol = refusal(
        &path,
        &owner,
        &mut contexts,
        &mut authorities,
        &supervisor,
        self_identity.clone(),
        "wrong-protocol",
    );
    assert!(
        wrong_protocol
            .diagnostic
            .unwrap()
            .starts_with("J03_PROTOCOL mode=fresh")
    );

    let mut foreign_image = std::process::Command::new("/usr/bin/sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let foreign_identity = identity(i64::from(foreign_image.id())).unwrap();
    let different_image = refusal(
        &path,
        &owner,
        &mut contexts,
        &mut authorities,
        &supervisor,
        foreign_identity,
        super::original_work::ROOT_PROTOCOL,
    );
    assert!(
        different_image
            .diagnostic
            .unwrap()
            .starts_with("J02_FRESH_IMAGE mode=fresh")
    );
    foreign_image.kill().unwrap();
    foreign_image.wait().unwrap();

    authorities.fresh(&owner, self_identity.clone()).unwrap();
    assert!(
        authorities
            .fresh_from_accepted_native(&owner, self_identity.clone(), &supervisor)
            .is_err(),
        "a copied fresh actor has no accepted native activation"
    );
    let inside_root = refusal(
        &path,
        &owner,
        &mut contexts,
        &mut authorities,
        &supervisor,
        self_identity,
        super::original_work::ROOT_PROTOCOL,
    );
    assert!(
        inside_root
            .diagnostic
            .unwrap()
            .starts_with("J01_FRESH_INSIDE_ROOT mode=fresh")
    );
    assert!(
        MailboxDb::open(&path)
            .unwrap()
            .completion_contexts()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn saturated_reader_refuses_without_admission_and_bounds_collection() {
    let root = tempfile::tempdir().unwrap();
    let endpoint = root.path().join("owner.sock");
    let owner = test_owner(&endpoint);
    let listener = UnixListener::bind(&endpoint).unwrap();
    listener.set_nonblocking(true).unwrap();
    let service = ControlService::start(&listener, &owner).unwrap();
    let mut clients = Vec::new();
    for _ in 0..control::PENDING_LIMIT {
        let mut client = UnixStream::connect(&endpoint).unwrap();
        write_fresh_join(&mut client);
        clients.push(client);
    }
    // Later accepted hello is a barrier through earlier queued connections.
    hello(&endpoint).unwrap();
    let error = connect_context(&endpoint, &owner).unwrap_err();
    assert!(error.contains("queue full"), "{error}");
    for client in &mut clients {
        client.set_nonblocking(true).unwrap();
        assert_eq!(
            client.read(&mut [0]).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
    let collected = service.pending();
    assert_eq!(collected.len(), control::PENDING_LIMIT);
    service.pause().unwrap();
    assert!(service.pending().is_empty());
    let mut backlog = collected;
    let (socket, mut client) = UnixStream::pair().unwrap();
    append_pending(
        &mut backlog,
        vec![fresh_join_request(socket, owner.guardian_identity.clone())],
        &owner,
    );
    let mut negative = String::new();
    client.read_to_string(&mut negative).unwrap();
    let refusal: JoinRefusal = serde_json::from_str(&negative).unwrap();
    assert!(matches!(
        refusal.completion_join_refusal,
        RefusalReason::QueueFull
    ));
    assert_eq!(backlog.len(), control::PENDING_LIMIT);
    service.stop().unwrap();
}

// Root C3/C5: two local leases share durable identity; failed final DELETE is
// observed directly, not inferred from time. Retry is performed by the same
// continuing owner, then inheritance is exercised as a distinct ownership case.
#[test]
fn incarnation_leases_preserve_siblings_and_retry_observed_delete_failure() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("pid-identity.db");
    let mut leases = ContextLeases::inherit(&path).unwrap();
    let id = identity(i64::from(std::process::id())).unwrap();
    let db = MailboxDb::open(&path).unwrap();
    let (server_a, client_a) = UnixStream::pair().unwrap();
    let (server_b, client_b) = UnixStream::pair().unwrap();
    leases.admit(&path, &id).unwrap();
    leases.retain_local(id.clone(), server_a);
    leases.admit(&path, &id).unwrap();
    leases.retain_local(id.clone(), server_b);
    drop(client_a);
    leases.release_disconnected(&path).unwrap();
    assert_eq!(db.completion_contexts().unwrap(), vec![id.clone()]);
    let sql = rusqlite::Connection::open(&path).unwrap();
    sql.execute_batch("CREATE TRIGGER deny_context_delete BEFORE DELETE ON completion_continuation_context BEGIN SELECT RAISE(ABORT, 'fixture_release_denied'); END;").unwrap();
    drop(client_b);
    let failure = leases.release_disconnected(&path).unwrap_err();
    assert!(failure.contains("fixture_release_denied"), "{failure}");
    assert!(!leases.is_empty());
    assert_eq!(db.completion_contexts().unwrap(), vec![id.clone()]);
    // A new local socket joins while a failed final release is still owed.
    let (server_c, client_c) = UnixStream::pair().unwrap();
    leases.admit(&path, &id).unwrap();
    leases.retain_local(id.clone(), server_c);
    sql.execute_batch("DROP TRIGGER deny_context_delete")
        .unwrap();
    leases.release_disconnected(&path).unwrap();
    assert_eq!(db.completion_contexts().unwrap(), vec![id.clone()]);
    drop(client_c);
    leases.release_disconnected(&path).unwrap();
    assert!(leases.is_empty());
    assert!(db.completion_contexts().unwrap().is_empty());
    println!(
        "actual failed DELETE={failure}; continuing owner retry committed after final local release"
    );
}

#[test]
fn inherited_context_is_not_released_by_a_new_local_socket() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("pid-identity.db");
    let id = identity(i64::from(std::process::id())).unwrap();
    let db = MailboxDb::open(&path).unwrap();
    db.retain_completion_context(&id).unwrap();
    let mut successor = ContextLeases::inherit(&path).unwrap();
    let (server, client) = UnixStream::pair().unwrap();
    successor.admit(&path, &id).unwrap();
    successor.retain_local(id.clone(), server);
    drop(client);
    successor.release_disconnected(&path).unwrap();
    assert!(!successor.is_empty());
    assert_eq!(db.completion_contexts().unwrap(), vec![id]);
}

#[test]
fn failed_release_transfers_to_successor_until_independent_identity_expiry() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("pid-identity.db");
    let mut original = ContextLeases::inherit(&path).unwrap();
    let mut child = std::process::Command::new("/bin/cat")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let id = identity(i64::from(child.id())).unwrap();
    let (server, client) = UnixStream::pair().unwrap();
    original.admit(&path, &id).unwrap();
    original.retain_local(id.clone(), server);
    let sql = rusqlite::Connection::open(&path).unwrap();
    sql.execute_batch("CREATE TRIGGER deny_successor_release BEFORE DELETE ON completion_continuation_context BEGIN SELECT RAISE(ABORT, 'fixture_successor_delete_denied'); END;").unwrap();
    drop(client);
    let error = original.release_disconnected(&path).unwrap_err();
    assert!(error.contains("fixture_successor_delete_denied"), "{error}");
    drop(original);
    let mut successor = ContextLeases::inherit(&path).unwrap();
    sql.execute_batch("DROP TRIGGER deny_successor_release")
        .unwrap();
    successor.release_disconnected(&path).unwrap();
    assert!(!successor.is_empty(), "live inherited responsibility lost");
    drop(child.stdin.take());
    assert!(child.wait().unwrap().success());
    successor.release_disconnected(&path).unwrap();
    assert!(successor.is_empty());
    assert!(
        MailboxDb::open(&path)
            .unwrap()
            .completion_contexts()
            .unwrap()
            .is_empty()
    );
    println!(
        "actual original DELETE error={error}; successor release committed after exact child exit"
    );
}

#[test]
fn committed_join_returns_versioned_root_capability_and_persistence_refusal_is_not_success() {
    for fail in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("pid-identity.db");
        let endpoint = root.path().join("owner.sock");
        let owner = test_owner(&endpoint);
        let mut contexts = ContextLeases::inherit(&path).unwrap();
        let db = rusqlite::Connection::open(&path).unwrap();
        if fail {
            db.execute_batch("CREATE TRIGGER deny_context_retain BEFORE INSERT ON completion_continuation_context BEGIN SELECT RAISE(ABORT, 'private_fixture_error_not_public'); END;").unwrap();
        }
        let (server, mut client) = UnixStream::pair().unwrap();
        let mut authorities = super::original_work::RootAuthorities::default();
        let (driver, _driver_peer) = UnixStream::pair().unwrap();
        let supervisor = super::root_supervisor::RootSupervisor::new(&path, driver).unwrap();
        retain_pending_context(
            &path,
            &owner,
            &mut contexts,
            &mut authorities,
            &supervisor,
            JoinRequest {
                socket: server,
                context: owner.guardian_identity.clone(),
                request: super::original_work::RootJoinRequest {
                    protocol: super::original_work::ROOT_PROTOCOL.into(),
                    mode: super::original_work::RootJoinMode::Fresh,
                },
            },
        );
        let mut bytes = Vec::new();
        loop {
            let mut byte = [0];
            client.read_exact(&mut byte).unwrap();
            if byte == [b'\n'] {
                break;
            }
            bytes.push(byte[0]);
        }
        if fail {
            let refusal: JoinRefusal = serde_json::from_slice(&bytes).unwrap();
            assert!(matches!(
                refusal.completion_join_refusal,
                RefusalReason::Persistence
            ));
            assert!(serde_json::from_slice::<CompletionDomainOwner>(&bytes).is_err());
            assert!(
                !String::from_utf8(bytes)
                    .unwrap()
                    .contains("private_fixture_error_not_public")
            );
            assert!(
                MailboxDb::open(&path)
                    .unwrap()
                    .completion_contexts()
                    .unwrap()
                    .is_empty()
            );
        } else {
            let response: super::original_work::RootJoinResponse =
                serde_json::from_slice(&bytes).unwrap();
            assert_eq!(
                response.owner.supervisor_authority_id,
                owner.supervisor_authority_id
            );
            assert_eq!(
                response.root_authority.protocol,
                super::original_work::ROOT_PROTOCOL
            );
            assert_eq!(
                MailboxDb::open(&path)
                    .unwrap()
                    .completion_contexts()
                    .unwrap(),
                vec![owner.guardian_identity]
            );
        }
    }
}
