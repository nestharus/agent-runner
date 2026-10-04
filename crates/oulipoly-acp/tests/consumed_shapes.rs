//! Consumed alpha7 object/envelope rivals, without a whole-schema claim.
use oulipoly_acp::{
    AcpClient, ClientInfo, DeliveryOutcome, IdleWaitFailure, LineTransport, NoAckCause,
    OutboundMessage, RequestFailure, SessionEvent,
};
use serde_json::{Value, json};
use std::io::Cursor;

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
        ClientInfo {
            name: "shapes-test".into(),
            version: "0".into(),
        },
    )
}
fn ack(id: u64) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":{"messageId":"m"}})
}
fn idle() -> Value {
    json!({"jsonrpc":"2.0","method":"session/update","params":{
        "sessionId":"s","update":{"sessionUpdate":"state_update","state":"idle"}
    }})
}

#[test]
fn positional_prompt_result_stays_owed_and_retry_is_uncertain() {
    let mut client = scripted(vec![
        init(),
        json!({"jsonrpc":"2.0","id":2,"result":["m",null]}),
        ack(3),
    ]);
    client.initialize().unwrap();
    let mut message = OutboundMessage::fresh("hello").unwrap();
    assert!(matches!(
        client.submit("s", &mut message),
        DeliveryOutcome::NotAcknowledged(NoAckCause::InvalidResponse(_))
    ));
    assert!(message.is_owed());
    assert!(message.acceptance().is_none());
    assert_eq!(message.unacknowledged_attempts(), 1);
    assert!(matches!(
        client.submit("s", &mut message),
        DeliveryOutcome::DuplicateUnknown(_)
    ));
    assert!(!message.is_owed());
    assert_eq!(message.unacknowledged_attempts(), 1);
}

#[test]
fn positional_new_session_result_is_not_success() {
    let mut client = scripted(vec![init(), json!({"jsonrpc":"2.0","id":2,"result":["s"]})]);
    client.initialize().unwrap();
    assert!(matches!(
        client.open_session("/work"),
        Err(RequestFailure::ProtocolViolation(_))
    ));
}

fn failed_init(response: Value) {
    let mut client = scripted(vec![response]);
    assert!(
        client.initialize().is_err(),
        "invalid initialize became a peer"
    );
    assert!(client.peer().is_none());
    let mut message = OutboundMessage::fresh("hello").unwrap();
    assert_eq!(
        client.submit("s", &mut message),
        DeliveryOutcome::NotNegotiated
    );
    assert!(message.is_owed());
    assert_eq!(message.unacknowledged_attempts(), 0);
}

#[test]
fn positional_initialize_result_is_not_negotiation() {
    let mut response = init();
    response["result"] = json!([2,{"name":"peer","version":"0"},{"session":{}},null]);
    failed_init(response);
}
#[test]
fn positional_initialize_info_is_not_negotiation() {
    let mut response = init();
    response["result"]["info"] = json!(["peer", "0"]);
    failed_init(response);
}
#[test]
fn positional_initialize_capabilities_do_not_advertise_sessions() {
    let mut response = init();
    response["result"]["capabilities"] = json!([{}]);
    failed_init(response);
}

// Exercise both ingestion sites: while a prompt awaits its response, and
// while waiting for readiness. A separate valid ACK remains valid evidence.
fn invalid_notification(notification: Value) {
    for during_prompt in [true, false] {
        let replies = if during_prompt {
            vec![init(), notification.clone(), ack(2)]
        } else {
            vec![init(), ack(2), notification.clone()]
        };
        let mut client = scripted(replies);
        client.initialize().unwrap();
        let mut message = OutboundMessage::fresh("hello").unwrap();
        assert!(matches!(
            client.submit("s", &mut message),
            DeliveryOutcome::Accepted(_)
        ));
        assert_eq!(
            client.await_session_idle("s"),
            Err(IdleWaitFailure::PeerGone),
            "malformed notification became readiness: {notification}"
        );
        assert!(
            client.events().is_empty(),
            "malformed notification retained an event"
        );
    }
}

#[test]
fn missing_notification_jsonrpc_does_not_establish_readiness() {
    let mut notification = idle();
    notification.as_object_mut().unwrap().remove("jsonrpc");
    invalid_notification(notification);
}
#[test]
fn wrong_notification_jsonrpc_does_not_establish_readiness() {
    for version in [json!("1.0"), json!(2), Value::Null] {
        let mut notification = idle();
        notification["jsonrpc"] = version;
        invalid_notification(notification);
    }
}
#[test]
fn positional_notification_params_do_not_establish_readiness() {
    let mut notification = idle();
    notification["params"] = json!(["s",{"sessionUpdate":"state_update","state":"idle"}]);
    invalid_notification(notification);
}
#[test]
fn malformed_notification_params_and_updates_are_not_events() {
    let mut cases = Vec::new();
    for params in [
        Value::Null,
        json!(true),
        json!("params"),
        json!({"update":{"sessionUpdate":"state_update","state":"idle"}}),
        json!({"sessionId":7,"update":{"sessionUpdate":"state_update","state":"idle"}}),
    ] {
        let mut notification = idle();
        notification["params"] = params;
        cases.push(notification);
    }
    for update in [
        Value::Null,
        json!(["state_update", "idle"]),
        json!("idle"),
        json!({"state":"idle"}),
        json!({"sessionUpdate":7,"state":"idle"}),
        json!({"sessionUpdate":"state_update"}),
        json!({"sessionUpdate":"state_update","state":7}),
        json!({"sessionUpdate":"user_message","messageId":null}),
    ] {
        let mut notification = idle();
        notification["params"]["update"] = update;
        cases.push(notification);
    }
    for notification in cases {
        invalid_notification(notification);
    }
}

#[test]
fn valid_objects_and_ignored_optional_fields_preserve_evidence() {
    let mut initialize = init();
    initialize["result"]["info"]["title"] = json!(17);
    initialize["result"]["capabilities"]["session"]["prompt"] = json!(false);
    let mut notification = idle();
    notification["params"]["update"]["stopReason"] = json!(false);
    let mut client = scripted(vec![
        initialize,
        json!({"jsonrpc":"2.0","id":2,"result":{"sessionId":"s","configOptions":false}}),
        json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"user_message","messageId":"m","content":false}}}),
        json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"state_update","state":"running"}}}),
        notification,
        ack(3),
    ]);
    client.initialize().unwrap();
    assert_eq!(client.open_session("/work").unwrap(), "s");
    let mut message = OutboundMessage::fresh("hello").unwrap();
    assert!(matches!(
        client.submit("s", &mut message),
        DeliveryOutcome::Accepted(_)
    ));
    assert!(!message.is_owed());
    assert_eq!(message.unacknowledged_attempts(), 0);
    assert_eq!(client.await_session_idle("s").unwrap().stop_reason, None);
    assert!(matches!(
        client.events(),
        [
            SessionEvent::UserMessage { .. },
            SessionEvent::Running { .. },
            SessionEvent::Idle { .. }
        ]
    ));
}
