//! Proof driver: the real owner-side `AcpClient` over a Unix socket.
//!
//! Usage: `v2_socket_submit <socket> <cwd> <text> [existing-session-id]`
//!
//! Negotiates v2, then either opens a session (`session/new: <id>`) or, given
//! an existing session id, attaches to it (`session/resume: <id>`), then waits for
//! one line on stdin (the driver's go signal, so it can bind a viewer to the
//! session first), submits one fresh message and prints what the client
//! recorded. Exit 0 only when the submit was `Accepted`; 3 otherwise. The peer is whatever listens on the socket; this
//! example claims nothing about it beyond the printed outcomes.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

use oulipoly_acp::{AcpClient, ClientInfo, DeliveryOutcome, LineTransport, OutboundMessage};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (socket, cwd, text, existing) = match args.as_slice() {
        [_, socket, cwd, text] => (socket, cwd, text, None),
        [_, socket, cwd, text, id] => (socket, cwd, text, Some(id.clone())),
        _ => {
            eprintln!("usage: v2_socket_submit <socket> <cwd> <text> [existing-session-id]");
            std::process::exit(2);
        }
    };
    let stream = UnixStream::connect(socket).expect("connect");
    let reader = BufReader::new(stream.try_clone().expect("clone"));
    let mut client = AcpClient::new(
        LineTransport::new(reader, stream),
        ClientInfo {
            name: "oulipoly-insertion-proof".to_owned(),
            version: "0".to_owned(),
        },
    );

    let peer = client.initialize();
    println!("initialize: {peer:?}");
    if peer.is_err() {
        std::process::exit(1);
    }
    let session = match existing {
        Some(id) => match client.resume_session(&id, cwd) {
            Ok(()) => {
                println!("session/resume: {id}");
                id
            }
            Err(why) => {
                println!("session/resume: {why:?}");
                std::process::exit(1);
            }
        },
        None => match client.open_session(cwd) {
            Ok(session) => {
                println!("session/new: {session}");
                session
            }
            Err(why) => {
                println!("session/new: {why:?}");
                std::process::exit(1);
            }
        },
    };
    std::io::stdout().flush().expect("flush");
    let mut go = String::new();
    std::io::stdin().lock().read_line(&mut go).expect("go line");

    let mut message = OutboundMessage::fresh(text.clone()).expect("fresh key");
    let outcome = client.submit(&session, &mut message);
    println!("submit: {outcome:?}");
    for event in client.events() {
        println!("event: {event:?}");
    }
    if !matches!(outcome, DeliveryOutcome::Accepted(_)) {
        std::process::exit(3);
    }
}
