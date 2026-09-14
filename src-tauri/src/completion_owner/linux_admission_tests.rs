use super::*;
use fs4::FileExt;

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
        owner_generation: "generation".into(),
        guardian_identity: id.clone(),
        driver_identity: id,
        endpoint: endpoint.to_string_lossy().into_owned(),
    };
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
        serde_json::to_writer(&mut socket, &owner).unwrap();
        socket.write_all(b"\n").unwrap();
        socket
    });
    assert_eq!(hello(&endpoint).unwrap().owner_generation, "generation");
    // Deterministically attempt retirement in the unreserved hello-to-join
    // interval. The actual close transaction must not even be invoked.
    assert!(!try_begin_retirement(&guardian, || panic!("closed before join")).unwrap());
    let context = connect_context(&endpoint).unwrap();
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
