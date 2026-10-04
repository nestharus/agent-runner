//! Deterministic ACP v2 peer: a real subprocess speaking stdio JSON lines,
//! used only as an owned harness in process-level tests. No model, no real
//! harness. Its "agent memory" lives in a JSON state file named by `--state`,
//! so a relaunched peer remembers earlier sessions and insertions, as the
//! dedup contract requires of a receiver that advertises it.
//!
//! `--mode`:
//! * `normal`: insert, acknowledge, report idle.
//! * `exit-before-ack-once`: on the first prompt of its state, insert and
//!   exit without acknowledging (a lost acknowledgement); later launches are
//!   normal.
//! * `exit-before-ack-always`: exit on every prompt without acknowledging.
//! * `silent`: never answer prompts; stay alive reading stdin.
//! * `insert-then-silent`: insert each prompt (honouring dedup) but never
//!   acknowledge it; stay alive reading stdin (an owner killed mid-delivery).
//! * `close-stdin`: close its stdin before answering `session/new`, then stay
//!   alive without reading or writing.
//!
//! `--launch-modes m1,m2,...` picks the mode by this state's launch number
//! (the last entry repeats), so one test can script successive launches,
//! including launches by a restarted supervisor.
//!
//! `--no-dedup` disables the local dedup contract. `--exit-after-acks N`
//! exits normally after the Nth acknowledgement and its idle update;
//! otherwise the peer stays alive until stdin ends.

use std::io::{BufRead, Write};
use std::path::PathBuf;

use oulipoly_acp::{DEDUP_CONTRACT_META, DUPLICATE_META, MESSAGE_KEY_META};
use serde_json::{Value, json};

struct Args {
    state: PathBuf,
    mode: String,
    launch_modes: String,
    dedup: bool,
    exit_after_acks: Option<u64>,
}

fn parse_args() -> Args {
    let mut args = Args {
        state: PathBuf::new(),
        mode: "normal".to_owned(),
        launch_modes: String::new(),
        dedup: true,
        exit_after_acks: None,
    };
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--state" => args.state = iter.next().expect("--state path").into(),
            "--mode" => args.mode = iter.next().expect("--mode value"),
            "--launch-modes" => args.launch_modes = iter.next().expect("--launch-modes value"),
            "--no-dedup" => args.dedup = false,
            "--exit-after-acks" => {
                args.exit_after_acks = Some(iter.next().expect("count").parse().expect("number"));
            }
            other => panic!("unknown argument {other}"),
        }
    }
    assert!(!args.state.as_os_str().is_empty(), "--state is required");
    args
}

fn load(path: &PathBuf) -> Value {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_else(|| {
            json!({
                "launches": [],
                "sessions": [],
                "prompts": [],
                "insertions": [],
                "fault_used": false,
            })
        })
}

fn save(path: &PathBuf, state: &Value) {
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, state.to_string()).expect("write state");
    std::fs::rename(&tmp, path).expect("commit state");
}

fn send(out: &mut impl Write, value: &Value) {
    let _ = writeln!(out, "{value}").and_then(|()| out.flush());
}

fn linger() -> ! {
    loop {
        std::thread::park();
    }
}

fn main() {
    let mut args = parse_args();
    // Test peer only: die with the thread that launched it, so a killed
    // supervisor never leaves this peer behind.
    // SAFETY: prctl with integer arguments.
    unsafe {
        libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
    }
    let mut state = load(&args.state);
    // SAFETY: getppid has no preconditions.
    let parent = unsafe { libc::getppid() };
    state["launches"]
        .as_array_mut()
        .expect("launches")
        .push(json!({ "pid": std::process::id(), "ppid": parent }));
    save(&args.state, &state);
    if !args.launch_modes.is_empty() {
        let modes: Vec<&str> = args.launch_modes.split(',').collect();
        let launch = state["launches"].as_array().expect("launches").len();
        args.mode = modes[(launch - 1).min(modes.len() - 1)].to_owned();
    }

    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    let mut acks = 0u64;
    for line in stdin.lock().lines() {
        let Ok(line) = line else { return };
        let Ok(request) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let params = request.get("params").cloned().unwrap_or(Value::Null);
        match request.get("method").and_then(Value::as_str) {
            Some("initialize") => {
                let mut result = json!({
                    "protocolVersion": 2,
                    "info": { "name": "deterministic-peer", "version": "1" },
                    "capabilities": { "session": {} },
                });
                if args.dedup {
                    result["_meta"] = json!({ DEDUP_CONTRACT_META: { "version": 1 } });
                }
                send(
                    &mut out,
                    &json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                );
            }
            Some("session/new") => {
                let sessions = state["sessions"].as_array_mut().expect("sessions");
                let session_id = format!("sess-{}", sessions.len() + 1);
                sessions.push(Value::String(session_id.clone()));
                save(&args.state, &state);
                if args.mode == "close-stdin" {
                    // SAFETY: closing our own stdin descriptor.
                    unsafe {
                        libc::close(0);
                    }
                    send(
                        &mut out,
                        &json!({ "jsonrpc": "2.0", "id": id, "result": { "sessionId": session_id } }),
                    );
                    linger();
                }
                send(
                    &mut out,
                    &json!({ "jsonrpc": "2.0", "id": id, "result": { "sessionId": session_id } }),
                );
            }
            Some("session/resume") => {
                let wanted = params["sessionId"].clone();
                let known = state["sessions"]
                    .as_array()
                    .expect("sessions")
                    .contains(&wanted);
                let reply = if known {
                    json!({ "jsonrpc": "2.0", "id": id, "result": {} })
                } else {
                    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32002, "message": "unknown session" } })
                };
                send(&mut out, &reply);
            }
            Some("session/prompt") => {
                let session_id = params["sessionId"].as_str().unwrap_or_default().to_owned();
                let key = params["_meta"][MESSAGE_KEY_META].clone();
                state["prompts"]
                    .as_array_mut()
                    .expect("prompts")
                    .push(json!({ "session": session_id, "key": key }));
                save(&args.state, &state);
                match args.mode.as_str() {
                    "silent" => continue,
                    "exit-before-ack-always" => std::process::exit(1),
                    _ => {}
                }
                let earlier = args
                    .dedup
                    .then(|| {
                        state["insertions"]
                            .as_array()
                            .expect("insertions")
                            .iter()
                            .find(|insertion| {
                                insertion["session"] == session_id.as_str()
                                    && insertion["key"] == key
                            })
                            .map(|insertion| insertion["messageId"].clone())
                    })
                    .flatten();
                let duplicate = earlier.is_some();
                let message_id = earlier.unwrap_or_else(|| {
                    let insertions = state["insertions"].as_array_mut().expect("insertions");
                    let message_id = format!("msg-{}", insertions.len() + 1);
                    insertions.push(json!({
                        "session": session_id,
                        "key": key,
                        "messageId": message_id,
                    }));
                    Value::String(message_id)
                });
                save(&args.state, &state);
                if args.mode == "insert-then-silent" {
                    continue;
                }
                if args.mode == "exit-before-ack-once" && state["fault_used"] == false {
                    state["fault_used"] = Value::Bool(true);
                    save(&args.state, &state);
                    std::process::exit(0);
                }
                let mut result = json!({ "messageId": message_id });
                if args.dedup {
                    result["_meta"] = json!({ MESSAGE_KEY_META: key, DUPLICATE_META: duplicate });
                }
                send(
                    &mut out,
                    &json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                );
                send(
                    &mut out,
                    &json!({
                        "jsonrpc": "2.0",
                        "method": "session/update",
                        "params": {
                            "sessionId": session_id,
                            "update": { "sessionUpdate": "state_update", "state": "idle", "stopReason": "end_turn" },
                        },
                    }),
                );
                acks += 1;
                if args.exit_after_acks == Some(acks) {
                    std::process::exit(0);
                }
            }
            _ => send(
                &mut out,
                &json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": "method not found" } }),
            ),
        }
    }
}
