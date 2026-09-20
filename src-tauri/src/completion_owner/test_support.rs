// Relational fixtures only; these rows do not attest native endpoint servicing.
use oulipoly_state::completion_continuation::{PROTOCOL, SourceProcessIdentity};
use oulipoly_state::mailbox::{CompletionDomainOwner, MailboxDb};

pub(crate) fn install_owner(db: &mut MailboxDb) {
    let live =
        oulipoly_state::pid_identity::read_live_process_identity(i64::from(std::process::id()))
            .unwrap()
            .unwrap();
    let identity = SourceProcessIdentity {
        pid: live.os_pid,
        boot_id: live.os_boot_id,
        starttime_ticks: live.os_pid_starttime_ticks,
    };
    db.publish_completion_continuation_owner(&CompletionDomainOwner {
        protocol: PROTOCOL.into(),
        domain_id: db.completion_continuation_domain().unwrap().unwrap(),
        supervisor_authority_id: uuid::Uuid::new_v4().to_string(),
        owner_generation: uuid::Uuid::new_v4().to_string(),
        guardian_identity: identity.clone(),
        driver_identity: identity,
        endpoint: "/fixture/not-a-native-endpoint".into(),
    })
    .unwrap();
}

pub(crate) fn revoke_unspent(db: &mut MailboxDb, session: &str, token: &str) {
    let attempt = db.continuation_activation(session, token).unwrap().unwrap();
    db.revoke_unaccepted_continuation_attempt(&attempt).unwrap();
}
