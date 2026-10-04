//! Client behaviour against the deterministic reference peer.

mod support;

use std::io::Cursor;
use std::sync::mpsc::channel;

use oulipoly_acp::{
    AcpClient, AtMostOnceBasis, ClientInfo, DEDUP_CONTRACT_META, DeliveryOutcome, IdleWaitFailure,
    LineTransport, MESSAGE_KEY_META, MessageKey, NegotiationFailure, NoAckCause, OutboundMessage,
    SessionEvent, Transport,
};
use serde_json::{Value, json};
use support::{PeerConfig, PromptFault, SharedStore, insertions_for, new_store, spawn_peer};

const CWD: &str = "/work";

fn info() -> ClientInfo {
    ClientInfo {
        name: "oulipoly-acp-test".to_owned(),
        version: "0".to_owned(),
    }
}

fn message(_label: &str) -> OutboundMessage {
    OutboundMessage::fresh("hello").unwrap()
}

// Assert repeated insertions use the original wire key, without exporting
// the client's fresh identity (which would allow caller-side forks).
fn original_key_insertions(store: &SharedStore) -> usize {
    let key = store
        .lock()
        .unwrap()
        .insertions
        .first()
        .and_then(|i| i.message_key.clone());
    key.map_or(0, |key| insertions_for(store, &key))
}

/// Connects, negotiates and opens (or resumes) a session.
fn connect(
    config: PeerConfig,
    store: &SharedStore,
    resume: Option<&str>,
) -> (AcpClient<support::ChannelTransport>, String) {
    let (transport, _peer) = spawn_peer(config, store.clone());
    let mut client = AcpClient::new(transport, info());
    client.initialize().expect("v2 negotiation");
    let session = match resume {
        Some(session) => {
            client.resume_session(session, CWD).expect("resume");
            session.to_owned()
        }
        None => client.open_session(CWD).expect("session/new"),
    };
    (client, session)
}

// (a) Version negotiation.

#[test]
fn negotiates_protocol_version_two() {
    let store = new_store();
    let (transport, _peer) = spawn_peer(PeerConfig::v2(), store);
    let mut client = AcpClient::new(transport, info());
    let peer = client.initialize().unwrap();
    assert_eq!(peer.protocol_version, 2);
    assert!(peer.dedup_contract);
}

#[test]
fn v1_only_peer_is_unsupported_and_never_receives_a_prompt() {
    let store = new_store();
    let config = PeerConfig {
        protocol_version: 1,
        ..PeerConfig::v2()
    };
    let (transport, _peer) = spawn_peer(config, store.clone());
    let mut client = AcpClient::new(transport, info());
    assert_eq!(
        client.initialize(),
        Err(NegotiationFailure::UnsupportedVersion { agent_version: 1 })
    );
    let mut msg = message("k-v1");
    assert_eq!(
        client.submit("sess-1", &mut msg),
        DeliveryOutcome::NotNegotiated
    );
    assert!(msg.is_owed());
    assert_eq!(store.lock().unwrap().prompts_received, 0);
}

#[test]
fn newer_version_answer_is_unsupported() {
    let config = PeerConfig {
        protocol_version: 3,
        ..PeerConfig::v2()
    };
    let (transport, _peer) = spawn_peer(config, new_store());
    let mut client = AcpClient::new(transport, info());
    assert_eq!(
        client.initialize(),
        Err(NegotiationFailure::UnsupportedVersion { agent_version: 3 })
    );
}

#[test]
fn peer_without_session_capability_is_refused() {
    let response = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": { "protocolVersion": 2, "info": { "name": "a", "version": "1" } },
    });
    let input = format!("{response}\n");
    let mut client = AcpClient::new(
        LineTransport::new(Cursor::new(input.into_bytes()), Vec::new()),
        info(),
    );
    assert_eq!(
        client.initialize(),
        Err(NegotiationFailure::NoSessionSurface)
    );
}

// (b) Insertion acknowledgement is distinct from session idle.

#[test]
fn acknowledgement_arrives_while_the_turn_is_still_running() {
    let store = new_store();
    let (release, hold) = channel();
    let config = PeerConfig {
        hold_turn: Some(hold),
        ..PeerConfig::v2()
    };
    let (mut client, session) = connect(config, &store, None);
    let mut msg = message("k-ack");

    // The peer will not report idle until released, so returning here
    // shows the acknowledgement does not wait for session idle.
    let DeliveryOutcome::Accepted(acceptance) = client.submit(&session, &mut msg) else {
        panic!("expected acceptance");
    };
    assert!(acceptance.at_most_once);
    assert!(!acceptance.recovered);
    assert_eq!(acceptance.basis, Some(AtMostOnceBasis::SingleAttempt));
    assert!(!msg.is_owed());
    assert!(
        !client
            .events()
            .iter()
            .any(|event| matches!(event, SessionEvent::Idle { .. })),
        "no session idle may be observed before release"
    );

    release.send(()).unwrap();
    let end = client.await_session_idle(&session).unwrap();
    assert_eq!(end.stop_reason.as_deref(), Some("end_turn"));
    assert!(client.events().contains(&SessionEvent::UserMessage {
        session_id: session.clone(),
        message_id: acceptance.message_id.clone(),
    }));
}

#[test]
fn peer_exit_after_acknowledgement_leaves_message_accepted_but_turn_unknown() {
    let store = new_store();
    let config = PeerConfig::v2().with_fault(PromptFault::ExitAfterAck);
    let (mut client, session) = connect(config, &store, None);
    let mut msg = message("k-exit-after-ack");
    assert!(matches!(
        client.submit(&session, &mut msg),
        DeliveryOutcome::Accepted(_)
    ));
    assert!(!msg.is_owed());
    assert_eq!(
        client.await_session_idle(&session),
        Err(IdleWaitFailure::PeerGone)
    );
}

// (c) Lost acknowledgement, then the same key to a complying receiver.

#[test]
fn resend_after_lost_ack_recovers_original_acceptance_with_one_insertion() {
    let store = new_store();
    let first = PeerConfig::v2().with_fault(PromptFault::ExitAfterInsert);
    let (mut client, session) = connect(first, &store, None);
    let mut msg = message("k-lost");

    assert_eq!(
        client.submit(&session, &mut msg),
        DeliveryOutcome::NotAcknowledged(NoAckCause::PeerGone)
    );
    assert!(msg.is_owed());
    assert_eq!(msg.unacknowledged_attempts(), 1);
    let original = store.lock().unwrap().insertions[0].message_id.clone();

    let (mut client, session) = connect(PeerConfig::v2(), &store, Some(&session));
    let DeliveryOutcome::Accepted(acceptance) = client.submit(&session, &mut msg) else {
        panic!("expected recovered acceptance");
    };
    assert_eq!(acceptance.message_id, original);
    assert!(acceptance.recovered);
    assert!(acceptance.at_most_once);
    assert_eq!(acceptance.basis, Some(AtMostOnceBasis::SessionContract));
    assert_eq!(original_key_insertions(&store), 1);
    assert_eq!(store.lock().unwrap().insertions.len(), 1);
}

#[test]
fn acknowledged_message_is_not_resent() {
    let store = new_store();
    let (mut client, session) = connect(PeerConfig::v2().without_dedup(), &store, None);
    let mut msg = message("k-once");
    let first = client.submit(&session, &mut msg);
    client.await_session_idle(&session).unwrap();
    assert_eq!(client.submit(&session, &mut msg), first);
    assert_eq!(store.lock().unwrap().prompts_received, 1);
}

// (d) The same retry to a receiver without dedup.

#[test]
fn resend_after_lost_ack_without_dedup_is_duplicate_unknown() {
    let store = new_store();
    let first = PeerConfig::v2()
        .without_dedup()
        .with_fault(PromptFault::ExitAfterInsert);
    let (mut client, session) = connect(first, &store, None);
    let mut msg = message("k-nodedup");
    assert!(matches!(
        client.submit(&session, &mut msg),
        DeliveryOutcome::NotAcknowledged(NoAckCause::PeerGone)
    ));

    let (mut client, session) = connect(PeerConfig::v2().without_dedup(), &store, Some(&session));
    assert!(!client.peer().unwrap().dedup_contract);
    let DeliveryOutcome::DuplicateUnknown(acceptance) = client.submit(&session, &mut msg) else {
        panic!("expected duplicate-unknown");
    };
    assert!(!acceptance.at_most_once);
    assert!(!acceptance.recovered);
    assert!(!msg.acceptance().unwrap().at_most_once);
    // The receiver really did insert twice; the label must not hide that.
    assert_eq!(original_key_insertions(&store), 2);
}

#[test]
fn advertised_dedup_with_mismatched_key_echo_is_duplicate_unknown() {
    let store = new_store();
    let lying = || PeerConfig {
        echo_wrong_key: true,
        ..PeerConfig::v2().without_dedup()
    };
    let (mut client, session) = connect(
        lying().with_fault(PromptFault::ExitAfterInsert),
        &store,
        None,
    );
    let mut msg = message("k-echo");
    assert!(matches!(
        client.submit(&session, &mut msg),
        DeliveryOutcome::NotAcknowledged(_)
    ));

    let (mut client, session) = connect(lying(), &store, Some(&session));
    assert!(client.peer().unwrap().dedup_contract);
    assert!(matches!(
        client.submit(&session, &mut msg),
        DeliveryOutcome::DuplicateUnknown(_)
    ));
    assert_eq!(original_key_insertions(&store), 2);
}

// (e) Peer exit before acknowledgement.

#[test]
fn peer_exit_before_ack_leaves_message_owed() {
    let store = new_store();
    let config = PeerConfig::v2().with_fault(PromptFault::ExitBeforeInsert);
    let (mut client, session) = connect(config, &store, None);
    let mut msg = message("k-exit");
    assert_eq!(
        client.submit(&session, &mut msg),
        DeliveryOutcome::NotAcknowledged(NoAckCause::PeerGone)
    );
    assert!(msg.is_owed());
    assert!(msg.acceptance().is_none());
    assert_eq!(msg.unacknowledged_attempts(), 1);
    assert!(
        !client
            .events()
            .iter()
            .any(|event| matches!(event, SessionEvent::Idle { .. }))
    );
    assert_eq!(store.lock().unwrap().insertions.len(), 0);
}

// Response correlation and validation.

#[test]
fn response_with_foreign_request_id_is_not_an_acknowledgement() {
    let store = new_store();
    let config = PeerConfig::v2().with_fault(PromptFault::WrongResponseId);
    let (mut client, session) = connect(config, &store, None);
    let mut msg = message("k-id");
    assert!(matches!(
        client.submit(&session, &mut msg),
        DeliveryOutcome::NotAcknowledged(NoAckCause::InvalidResponse(_))
    ));
    assert!(msg.is_owed());
}

#[test]
fn null_message_id_is_not_an_acknowledgement() {
    let store = new_store();
    let config = PeerConfig::v2().with_fault(PromptFault::NullMessageId);
    let (mut client, session) = connect(config, &store, None);
    let mut msg = message("k-null");
    assert!(matches!(
        client.submit(&session, &mut msg),
        DeliveryOutcome::NotAcknowledged(NoAckCause::InvalidResponse(_))
    ));
    assert!(msg.is_owed());
}

#[test]
fn rejected_prompt_is_owed_and_insertion_uncertain() {
    let store = new_store();
    let (mut client, _session) = connect(PeerConfig::v2(), &store, None);
    let mut msg = message("k-reject");
    assert!(matches!(
        client.submit("no-such-session", &mut msg),
        DeliveryOutcome::Rejected { code: -32002, .. }
    ));
    assert!(msg.is_owed());
    assert_eq!(msg.unacknowledged_attempts(), 1);
}

#[test]
fn resume_of_unknown_session_is_rejected() {
    let (transport, _peer) = spawn_peer(PeerConfig::v2(), new_store());
    let mut client = AcpClient::new(transport, info());
    client.initialize().unwrap();
    assert!(client.resume_session("missing", CWD).is_err());
}

// Wire shape against the pinned schema's field names.

#[test]
fn wire_requests_use_pinned_v2_field_names_and_meta_key() {
    let responses = [
        json!({ "jsonrpc": "2.0", "id": 1, "result": {
            "protocolVersion": 2,
            "info": { "name": "a", "version": "1" },
            "capabilities": { "session": {} },
        }}),
        json!({ "jsonrpc": "2.0", "id": 2, "result": { "sessionId": "s" } }),
        json!({ "jsonrpc": "2.0", "id": 3, "result": { "messageId": "m" } }),
    ];
    let input: String = responses.iter().map(|r| format!("{r}\n")).collect();
    let mut client = AcpClient::new(
        LineTransport::new(Cursor::new(input.into_bytes()), Vec::new()),
        info(),
    );
    let peer = client.initialize().unwrap();
    assert!(!peer.dedup_contract);
    let session = client.open_session(CWD).unwrap();
    let mut msg = OutboundMessage::new(MessageKey::new("k-wire").unwrap(), "hello");
    assert!(matches!(
        client.submit(&session, &mut msg),
        DeliveryOutcome::DuplicateUnknown(_)
    ));

    let (_, written) = client.into_transport().into_parts();
    let sent: Vec<Value> = String::from_utf8(written)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(sent.len(), 3);

    assert_eq!(sent[0]["method"], "initialize");
    assert_eq!(sent[0]["params"]["protocolVersion"], 2);
    assert_eq!(sent[0]["params"]["info"]["name"], "oulipoly-acp-test");
    assert_eq!(
        sent[0]["params"]["_meta"][DEDUP_CONTRACT_META]["version"],
        1
    );

    assert_eq!(sent[1]["method"], "session/new");
    assert_eq!(sent[1]["params"]["cwd"], CWD);

    assert_eq!(sent[2]["method"], "session/prompt");
    assert_eq!(sent[2]["params"]["sessionId"], "s");
    assert_eq!(sent[2]["params"]["prompt"][0]["type"], "text");
    assert_eq!(sent[2]["params"]["_meta"][MESSAGE_KEY_META], "k-wire");
    // Custom data never goes at the root of a specified type.
    let prompt_params = sent[2]["params"].as_object().unwrap();
    let mut keys: Vec<&str> = prompt_params.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["_meta", "prompt", "sessionId"]);
}

#[test]
fn agent_request_during_prompt_is_refused_and_ack_still_read() {
    let responses = [
        json!({ "jsonrpc": "2.0", "id": 1, "result": {
            "protocolVersion": 2,
            "info": { "name": "a", "version": "1" },
            "capabilities": { "session": {} },
        }}),
        json!({ "jsonrpc": "2.0", "id": 77, "method": "session/request_permission", "params": {} }),
        json!({ "jsonrpc": "2.0", "id": 2, "result": { "messageId": "m" } }),
    ];
    let input: String = responses.iter().map(|r| format!("{r}\n")).collect();
    let mut client = AcpClient::new(
        LineTransport::new(Cursor::new(input.into_bytes()), Vec::new()),
        info(),
    );
    client.initialize().unwrap();
    let mut msg = message("k-perm");
    assert!(matches!(
        client.submit("s", &mut msg),
        DeliveryOutcome::Accepted(_)
    ));
    let (_, written) = client.into_transport().into_parts();
    let refusal: Value = String::from_utf8(written)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|message| message["id"] == 77)
        .expect("refusal sent");
    assert_eq!(refusal["error"]["code"], -32601);
}

#[test]
fn line_transport_reports_malformed_and_closed() {
    let mut transport = LineTransport::new(Cursor::new(b"not json\n".to_vec()), Vec::new());
    assert!(matches!(
        transport.recv(),
        oulipoly_acp::Incoming::Malformed(_)
    ));
    assert_eq!(transport.recv(), oulipoly_acp::Incoming::Closed);
}

// (c) A turn's output, notices and agent requests are observed, and every
// agent request is refused, without changing the insertion ACK.

#[test]
fn turn_output_notices_and_refused_requests_are_observed() {
    let lines = [
        json!({ "jsonrpc": "2.0", "id": 1, "result": {
            "protocolVersion": 2, "info": { "name": "a", "version": "1" }, "capabilities": { "session": {} } } }),
        json!({ "jsonrpc": "2.0", "id": 2, "result": { "sessionId": "s" } }),
        json!({ "jsonrpc": "2.0", "id": 3, "result": { "messageId": "m-user" } }),
        json!({ "jsonrpc": "2.0", "id": "p1", "method": "session/request_permission", "params": {
            "sessionId": "s", "title": "bash", "options": [] } }),
        json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": "s", "update": {
            "sessionUpdate": "notice", "severity": "warning", "title": "permission rejected: bash" } } }),
        json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": "s", "update": {
            "sessionUpdate": "agent_message", "messageId": "m-agent",
            "content": [{ "type": "text", "text": "RE" }, { "type": "text", "text": "FUSED" }] } } }),
        json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": "s", "update": {
            "sessionUpdate": "state_update", "state": "idle", "stopReason": "end_turn" } } }),
    ];
    let input: String = lines.iter().map(|line| format!("{line}\n")).collect();
    let mut client = AcpClient::new(
        LineTransport::new(Cursor::new(input.into_bytes()), Vec::new()),
        info(),
    );
    client.initialize().expect("v2 negotiation");
    let session = client.open_session(CWD).expect("session/new");
    let mut msg = message("turn");
    assert!(matches!(
        client.submit(&session, &mut msg),
        DeliveryOutcome::Accepted(_)
    ));
    let idle = client.await_session_idle(&session).unwrap();
    assert_eq!(idle.stop_reason.as_deref(), Some("end_turn"));
    let events = client.events().to_vec();
    assert!(events.contains(&SessionEvent::RequestRefused {
        method: "session/request_permission".to_owned(),
        session_id: Some("s".to_owned()),
    }));
    assert!(events.contains(&SessionEvent::Notice {
        session_id: "s".to_owned(),
        severity: "warning".to_owned(),
        title: "permission rejected: bash".to_owned(),
        description: None,
    }));
    assert!(events.contains(&SessionEvent::AgentMessage {
        session_id: "s".to_owned(),
        message_id: "m-agent".to_owned(),
        text: "REFUSED".to_owned(),
    }));
    let (_, written) = client.into_transport().into_parts();
    let refusal = String::from_utf8(written)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|line| line["id"] == "p1")
        .expect("a reply to the agent request");
    assert_eq!(refusal["error"]["code"], -32601);
}
