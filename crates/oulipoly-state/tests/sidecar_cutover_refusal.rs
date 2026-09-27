use oulipoly_state::mailbox::MailboxDb;
use oulipoly_state::pid_identity::PidIdentityDb;
use rusqlite::Connection;

#[test]
fn every_public_sidecar_writer_refuses_a_newer_broker_version() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pid-identity.db");
    drop(MailboxDb::open(&path).unwrap());
    let raw = Connection::open(&path).unwrap();
    raw.pragma_update(None, "user_version", 30).unwrap();
    drop(raw);

    for result in [
        MailboxDb::open(&path).map(|_| ()),
        MailboxDb::open_existing_native_authority(&path).map(|_| ()),
        PidIdentityDb::open(&path).map(|_| ()),
    ] {
        let error = result.expect_err("a direct writer must refuse broker version 30");
        assert!(
            error.contains("Unsupported PID mailbox sidecar schema version 30"),
            "{error}"
        );
    }
}
