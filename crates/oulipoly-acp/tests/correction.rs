//! Rivals from the funded consequence decision D2–D7.
mod support;

use oulipoly_acp::{
    AcpClient, ClientInfo, DeliveryOutcome, LineTransport, MessageKey, NegotiationFailure,
    NoAckCause, OutboundMessage, RequestFailure,
};
use serde_json::{Value, json};
use std::io::Cursor;
use support::{PeerConfig, PromptFault, SharedStore, new_store, spawn_peer};

fn info() -> ClientInfo {
    ClientInfo {
        name: "correction-test".into(),
        version: "0".into(),
    }
}
fn init() -> Value {
    json!({"jsonrpc":"2.0","id":1,"result":{
        "protocolVersion":2,"info":{"name":"peer","version":"0"},
        "capabilities":{"session":{}}
    }})
}
fn scripted(replies: Vec<Value>) -> AcpClient<LineTransport<Cursor<Vec<u8>>, Vec<u8>>> {
    let input: String = replies.iter().map(|r| format!("{r}\n")).collect();
    AcpClient::new(
        LineTransport::new(Cursor::new(input.into_bytes()), Vec::new()),
        info(),
    )
}
fn connect(
    config: PeerConfig,
    store: &SharedStore,
    session: Option<&str>,
) -> (AcpClient<support::ChannelTransport>, String) {
    let (transport, _) = spawn_peer(config, store.clone());
    let mut client = AcpClient::new(transport, info());
    client.initialize().unwrap();
    let session = match session {
        Some(id) => {
            client.resume_session(id, "/work").unwrap();
            id.to_owned()
        }
        None => client.open_session("/work").unwrap(),
    };
    (client, session)
}
fn unknown(outcome: DeliveryOutcome) {
    let DeliveryOutcome::DuplicateUnknown(ack) = outcome else {
        panic!("unproved at-most-once: {outcome:?}");
    };
    assert!(!ack.at_most_once);
}

#[test]
fn prior_noncontract_attempts_are_not_repaired_by_current_dedup() {
    let store = new_store();
    let fault = || {
        PeerConfig::v2()
            .without_dedup()
            .with_fault(PromptFault::ExitAfterInsert)
    };
    let (mut first, session) = connect(fault(), &store, None);
    let mut msg = OutboundMessage::fresh("hello").unwrap();
    assert!(matches!(
        first.submit(&session, &mut msg),
        DeliveryOutcome::NotAcknowledged(_)
    ));
    let (mut second, _) = connect(fault(), &store, Some(&session));
    assert!(matches!(
        second.submit(&session, &mut msg),
        DeliveryOutcome::NotAcknowledged(_)
    ));
    let (mut third, _) = connect(PeerConfig::v2(), &store, Some(&session));
    unknown(third.submit(&session, &mut msg));
    assert_eq!(store.lock().unwrap().insertions.len(), 2);
}

#[test]
fn supplied_key_history_is_unknown_even_with_current_contract() {
    let store = new_store();
    let (mut client, session) = connect(PeerConfig::v2(), &store, None);
    let mut msg = OutboundMessage::new(MessageKey::new("recreated").unwrap(), "hello");
    unknown(client.submit(&session, &mut msg));
    assert!(!msg.is_owed());
}

#[test]
fn recovered_key_after_lost_ack_is_not_complete_history() {
    let store = new_store();
    let (mut first, session) = connect(
        PeerConfig::v2().with_fault(PromptFault::ExitAfterInsert),
        &store,
        None,
    );
    let mut msg = OutboundMessage::fresh("hello").unwrap();
    assert!(matches!(
        first.submit(&session, &mut msg),
        DeliveryOutcome::NotAcknowledged(_)
    ));
    let key = store.lock().unwrap().insertions[0]
        .message_key
        .clone()
        .unwrap();
    let mut recreated = OutboundMessage::new(MessageKey::new(key).unwrap(), "hello");
    let (mut second, _) = connect(PeerConfig::v2(), &store, Some(&session));
    unknown(second.submit(&session, &mut recreated));
    assert_eq!(store.lock().unwrap().insertions.len(), 1);
}

#[test]
fn exporting_and_cloning_a_fresh_key_abandons_both_history_claims() {
    let store = new_store();
    let (mut client, session) = connect(PeerConfig::v2().without_dedup(), &store, None);
    let mut original = OutboundMessage::fresh("hello").unwrap();
    let mut fork = OutboundMessage::new(original.key().clone(), "hello");
    unknown(client.submit(&session, &mut fork));
    client.await_session_idle(&session).unwrap();
    unknown(client.submit(&session, &mut original));
    assert_eq!(store.lock().unwrap().insertions.len(), 2);
}

#[test]
fn valid_rejection_counts_before_a_noncontract_retry() {
    let mut client = scripted(vec![
        init(),
        json!({"jsonrpc":"2.0","id":2,"error":{"code":-32002,"message":"rejected"}}),
        json!({"jsonrpc":"2.0","id":3,"result":{"messageId":"m"}}),
    ]);
    client.initialize().unwrap();
    let mut msg = OutboundMessage::fresh("hello").unwrap();
    assert_eq!(
        client.submit("s", &mut msg),
        DeliveryOutcome::Rejected {
            code: -32002,
            message: "rejected".into()
        }
    );
    assert!(msg.is_owed());
    unknown(client.submit("s", &mut msg));
    assert_eq!(msg.unacknowledged_attempts(), 1);
}

fn malformed_prompt(response: Value) {
    let mut client = scripted(vec![init(), response]);
    client.initialize().unwrap();
    let mut msg = OutboundMessage::fresh("hello").unwrap();
    assert!(matches!(
        client.submit("s", &mut msg),
        DeliveryOutcome::NotAcknowledged(NoAckCause::InvalidResponse(_))
    ));
    assert!(msg.is_owed());
    assert_eq!(msg.unacknowledged_attempts(), 1);
}
macro_rules! malformed {
    ($name:ident, $response:expr) => {
        #[test]
        fn $name() {
            malformed_prompt($response);
        }
    };
}
malformed!(
    wrong_jsonrpc_is_unknown,
    json!({"jsonrpc":"1.0","id":2,"result":{"messageId":"m"}})
);
malformed!(
    missing_jsonrpc_is_unknown,
    json!({"id":2,"result":{"messageId":"m"}})
);
malformed!(
    both_result_and_error_are_unknown,
    json!({"jsonrpc":"2.0","id":2,"result":{"messageId":"m"},"error":{"code":1,"message":"e"}})
);
malformed!(
    null_error_is_unknown,
    json!({"jsonrpc":"2.0","id":2,"error":null})
);
malformed!(
    noninteger_error_code_is_unknown,
    json!({"jsonrpc":"2.0","id":2,"error":{"code":"1","message":"e"}})
);
malformed!(
    missing_error_message_is_unknown,
    json!({"jsonrpc":"2.0","id":2,"error":{"code":1}})
);

#[test]
fn invalid_session_capability_is_not_negotiation_success() {
    for shape in [json!(false), json!("session"), json!([])] {
        let mut response = init();
        response["result"]["capabilities"]["session"] = shape;
        let mut client = scripted(vec![response]);
        assert!(matches!(
            client.initialize(),
            Err(NegotiationFailure::ProtocolViolation(_))
        ));
        assert!(client.peer().is_none());
    }
}
#[test]
fn nonobject_resume_is_not_session_success() {
    for shape in [Value::Null, json!(17), json!([])] {
        let mut client = scripted(vec![init(), json!({"jsonrpc":"2.0","id":2,"result":shape})]);
        client.initialize().unwrap();
        assert!(matches!(
            client.resume_session("s", "/work"),
            Err(RequestFailure::ProtocolViolation(_))
        ));
    }
}

#[test]
fn failed_renegotiation_blocks_prompt_until_later_success() {
    let failures = [
        json!({"jsonrpc":"2.0","id":2,"result":{"protocolVersion":1}}),
        json!({"jsonrpc":"2.0","id":2,"error":{"code":1,"message":"no"}}),
        json!({"jsonrpc":"2.0","id":2,"result":null}),
    ];
    for failure in failures {
        let mut later = init();
        later["id"] = json!(3);
        let mut client = scripted(vec![
            init(),
            failure,
            later,
            json!({"jsonrpc":"2.0","id":4,"result":{"messageId":"m"}}),
        ]);
        client.initialize().unwrap();
        assert!(client.initialize().is_err());
        let mut msg = OutboundMessage::fresh("hello").unwrap();
        assert_eq!(client.submit("s", &mut msg), DeliveryOutcome::NotNegotiated);
        assert_eq!(msg.unacknowledged_attempts(), 0);
        client.initialize().unwrap();
        assert!(matches!(
            client.submit("s", &mut msg),
            DeliveryOutcome::Accepted(_)
        ));
        let (_, written) = client.into_transport().into_parts();
        let sent: Vec<Value> = String::from_utf8(written)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            sent.iter()
                .filter(|r| r["method"] == "session/prompt")
                .count(),
            1
        );
    }
}

fn idle(session: &str) -> Value {
    json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":session,"update":{"sessionUpdate":"state_update","state":"idle","stopReason":"end_turn"}}})
}
#[test]
fn another_session_wait_never_discards_observed_idle() {
    let mut client = scripted(vec![
        init(),
        json!({"jsonrpc":"2.0","id":2,"result":{"sessionId":"A"}}),
        json!({"jsonrpc":"2.0","id":3,"result":{"sessionId":"B"}}),
        json!({"jsonrpc":"2.0","id":4,"result":{"messageId":"a"}}),
        idle("A"),
        json!({"jsonrpc":"2.0","id":5,"result":{"messageId":"b"}}),
        idle("B"),
    ]);
    client.initialize().unwrap();
    let a = client.open_session("/work").unwrap();
    let b = client.open_session("/work").unwrap();
    client.submit(&a, &mut OutboundMessage::fresh("a").unwrap());
    client.submit(&b, &mut OutboundMessage::fresh("b").unwrap());
    assert!(client.await_session_idle(&b).is_ok());
    assert!(
        client.await_session_idle(&a).is_ok(),
        "A idle was already observed during B submission"
    );
}
#[test]
fn same_session_wait_retains_each_already_observed_idle() {
    let mut client = scripted(vec![
        init(),
        idle("s"),
        idle("s"),
        json!({"jsonrpc":"2.0","id":2,"result":{"messageId":"m"}}),
    ]);
    client.initialize().unwrap();
    client.submit("s", &mut OutboundMessage::fresh("hello").unwrap());
    assert!(client.await_session_idle("s").is_ok());
    assert!(client.await_session_idle("s").is_ok());
}
#[test]
fn reference_dedup_is_scoped_to_session_and_key() {
    let store = new_store();
    let (mut client, a) = connect(PeerConfig::v2(), &store, None);
    let b = client.open_session("/work").unwrap();
    let supplied = || OutboundMessage::new(MessageKey::new("same-key").unwrap(), "hello");
    client.submit(&a, &mut supplied());
    client.await_session_idle(&a).unwrap();
    client.submit(&b, &mut supplied());
    assert_eq!(store.lock().unwrap().insertions.len(), 2);
}
#[test]
fn cached_acceptance_cannot_be_rebound_to_another_session() {
    let mut client = scripted(vec![
        init(),
        json!({"jsonrpc":"2.0","id":2,"result":{"messageId":"m"}}),
    ]);
    client.initialize().unwrap();
    let mut msg = OutboundMessage::fresh("hello").unwrap();
    assert!(matches!(
        client.submit("A", &mut msg),
        DeliveryOutcome::Accepted(_)
    ));
    assert_eq!(
        client.submit("B", &mut msg),
        DeliveryOutcome::SessionMismatch
    );
}

#[test]
fn idle_after_attempt_label_requires_a_tracked_attempt() {
    let mut client = scripted(vec![init(), idle("s")]);
    client.initialize().unwrap();
    assert_eq!(
        client.await_session_idle("s"),
        Err(oulipoly_acp::IdleWaitFailure::NoAttempt)
    );
}
