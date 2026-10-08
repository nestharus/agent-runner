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
//! * `off-session-no-ack`: emit the configured off-session turn, then exit
//!   without acknowledging insertion.
//! * `close-stdin`: close its stdin before answering `session/new`, then stay
//!   alive without reading or writing.
//!
//! `--launch-modes m1,m2,...` picks the mode by this state's launch number
//! (the last entry repeats), so one test can script successive launches,
//! including launches by a restarted supervisor.
//!
//! `--no-idle` acknowledges prompts but never reports idle (a turn that
//! never ends). `--silent-after-acks N` records later prompts but never
//! answers them once it has acknowledged N.
//! `--turn-before-ack` sends an echo reply and idle before the insertion
//! response, exercising the ordering allowed by the native endpoint.
//!
//! `--off-session-turn SESSION --turn-gate PATH` emits a colliding reply/idle
//! for SESSION before the first ACK (or after with `--off-session-after-ack`),
//! then waits at PATH before the real idle.
//! The current-session notice marks the end of the injected evidence.
//!
//! `--reject-prompt-once` rejects the first ordinary prompt with -32011
//! before insertion, then stays alive for later inputs.
//!
//! `--reject-completion-once` rejects the first background
//! completion with -32001 before insertion, then stays alive for later inputs.
//!
//! `--live-reattach` declares the live reattachment contract in every
//! `initialize` response (a later owner may converse with this live
//! process again); `--live-reattach-first` declares it only to the first
//! `initialize` of this process. Its sessions and ascending message ids
//! already persist in its state file.
//!
//! `--no-dedup` disables the local dedup contract. `--exit-after-acks N`
//! exits normally after the Nth acknowledgement and its idle update;
//! otherwise the peer stays alive until stdin ends.
//!
//! `--on-reinit normal|exit` acts on a second `initialize` on the same
//! stdio (a restarted owner attaching to this surviving process): switch to
//! `normal`, or exit with status 1 without answering. `--exit-when-file P`
//! makes the state's first launch exit with status 7 once file `P` exists.
//!
//! With `OULIPOLY_ACP_V2_SOCKET` set, the peer speaks on a Unix socket it
//! listens on at that path instead of stdio, one connection at a time; a
//! closed connection is not its end (it accepts the next one), and it
//! stays alive until killed or `--exit-after-acks`. `close-stdin` is
//! stdio-only.
//!
//! Commands are read from the prompt's last paragraph (after the last blank
//! line), so an owner-prefixed brief does not hide them. Besides `bash:`:
//! `echo:TEXT` replies TEXT; `explore:ROUTE:QUESTION` asks the root's owner
//! for a registered child through `oulipoly-root-child` (next to this
//! binary) and replies with its result line; `spawn:COMMAND` starts
//! `/bin/sh -c COMMAND` without waiting for it and replies with its pid.
//!
//! Message ids are `msg-NNNN`, ascending. Every idle after an
//! acknowledgement carries `_meta` [`TURN_INPUT_META`] naming that message.
//! A prompt whose text starts with `bash:` runs the rest with `/bin/sh -c`
//! through `oulipoly-root-bash` (next to this binary), i.e. through the
//! root's Bash ingress, after acknowledging it; its exit code and output
//! come back as one `agent_message` tagged with [`PARENT_MESSAGE_META`],
//! then the idle. `--untagged` sends neither tag. A prompt starting
//! `[Background Bash completion]` (an owner's background-run completion)
//! is handled the same way with the command `@completion <its last line>`,
//! for a test's requester to interpret; the prototype requester runs it as
//! a shell command, which fails visibly.
//!
//! The peer sets no parent-death signal: whether it outlives its owner is
//! decided by its custody, not by the peer.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;

use agent_provider_contract::acp::{
    DEDUP_CONTRACT_META, DUPLICATE_META, LIVE_REATTACH_META, MESSAGE_KEY_META, PARENT_MESSAGE_META,
    TURN_INPUT_META,
};
use serde_json::{Value, json};

struct Args {
    state: PathBuf,
    mode: String,
    launch_modes: String,
    dedup: bool,
    /// `None`, `"always"` or `"first"`.
    live_reattach: Option<&'static str>,
    exit_after_acks: Option<u64>,
    on_reinit: Option<String>,
    exit_when_file: Option<PathBuf>,
    tagged: bool,
    idle: bool,
    silent_after_acks: Option<u64>,
    turn_before_ack: bool,
    reject_completion_once: bool,
    reject_prompt_once: bool,
    off_session_turn: Option<String>,
    off_session_after_ack: bool,
    turn_gate: Option<PathBuf>,
    resident_evidence: Option<String>,
}

fn parse_args() -> Args {
    let mut args = Args {
        state: PathBuf::new(),
        mode: "normal".to_owned(),
        launch_modes: String::new(),
        dedup: true,
        live_reattach: None,
        exit_after_acks: None,
        on_reinit: None,
        exit_when_file: None,
        tagged: true,
        idle: true,
        silent_after_acks: None,
        turn_before_ack: false,
        reject_completion_once: false,
        reject_prompt_once: false,
        off_session_turn: None,
        off_session_after_ack: false,
        turn_gate: None,
        resident_evidence: None,
    };
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--resident-evidence" => {
                args.resident_evidence = Some(iter.next().expect("evidence shape"))
            }
            "--state" => args.state = iter.next().expect("--state path").into(),
            "--mode" => args.mode = iter.next().expect("--mode value"),
            "--launch-modes" => args.launch_modes = iter.next().expect("--launch-modes value"),
            "--no-dedup" => args.dedup = false,
            "--live-reattach" => args.live_reattach = Some("always"),
            "--live-reattach-first" => args.live_reattach = Some("first"),
            "--reject-prompt-once" => args.reject_prompt_once = true,
            "--reject-completion-once" => args.reject_completion_once = true,
            "--untagged" => args.tagged = false,
            "--no-idle" => args.idle = false,
            "--turn-before-ack" => args.turn_before_ack = true,
            "--off-session-turn" => args.off_session_turn = Some(iter.next().expect("session")),
            "--off-session-after-ack" => args.off_session_after_ack = true,
            "--turn-gate" => args.turn_gate = Some(iter.next().expect("gate path").into()),
            "--silent-after-acks" => {
                args.silent_after_acks = Some(
                    iter.next()
                        .expect("--silent-after-acks value")
                        .parse()
                        .expect("number"),
                );
            }
            "--on-reinit" => args.on_reinit = Some(iter.next().expect("--on-reinit value")),
            "--exit-when-file" => {
                args.exit_when_file = Some(iter.next().expect("--exit-when-file path").into());
            }
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

/// Real supported native updates with colliding parent and last-user tags.
fn off_session_turn(out: &mut impl Write, session: &str, message_id: &Value) {
    for update in [
        json!({ "sessionUpdate": "agent_message", "messageId": "foreign-reply",
            "content": [{ "type": "text", "text": "OFF-SESSION" }],
            "_meta": { PARENT_MESSAGE_META: message_id } }),
        json!({ "sessionUpdate": "state_update", "state": "idle",
            "stopReason": "end_turn", "_meta": { TURN_INPUT_META: message_id } }),
    ] {
        send(
            out,
            &json!({ "jsonrpc": "2.0", "method": "session/update",
            "params": { "sessionId": session, "update": update } }),
        );
    }
}

fn linger() -> ! {
    loop {
        std::thread::park();
    }
}

fn main() {
    let mut args = parse_args();
    let mut state = load(&args.state);
    // SAFETY: getppid has no preconditions.
    let parent = unsafe { libc::getppid() };
    state["launches"]
        .as_array_mut()
        .expect("launches")
        .push(json!({ "pid": std::process::id(), "ppid": parent }));
    save(&args.state, &state);
    let launch = state["launches"].as_array().expect("launches").len();
    if !args.launch_modes.is_empty() {
        let modes: Vec<&str> = args.launch_modes.split(',').collect();
        args.mode = modes[(launch - 1).min(modes.len() - 1)].to_owned();
    }
    if let Some(path) = args.exit_when_file.clone().filter(|_| launch == 1) {
        std::thread::spawn(move || {
            while !path.exists() {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            std::process::exit(7);
        });
    }
    let mut peer = Peer {
        args,
        state,
        initializations: 0,
        acks: 0,
    };
    if let Some(path) = std::env::var_os("OULIPOLY_ACP_V2_SOCKET") {
        let listener = UnixListener::bind(path).expect("listen");
        loop {
            let (stream, _) = listener.accept().expect("accept");
            let reader = BufReader::new(stream.try_clone().expect("clone"));
            peer.serve(reader, stream);
        }
    }
    peer.serve(std::io::stdin().lock(), std::io::stdout().lock());
}

struct Peer {
    args: Args,
    state: Value,
    initializations: u32,
    acks: u64,
}

impl Peer {
    /// Answers one connection until it ends.
    fn serve(&mut self, input: impl BufRead, mut out: impl Write) {
        let Self {
            args,
            state,
            initializations,
            acks,
        } = self;
        let out = &mut out;
        for line in input.lines() {
            let Ok(line) = line else { return };
            let Ok(request) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let id = request.get("id").cloned().unwrap_or(Value::Null);
            let params = request.get("params").cloned().unwrap_or(Value::Null);
            match request.get("method").and_then(Value::as_str) {
                Some("initialize") => {
                    *initializations += 1;
                    if *initializations > 1 {
                        match args.on_reinit.as_deref() {
                            Some("exit") => std::process::exit(1),
                            Some(mode) => args.mode = mode.to_owned(),
                            None => {}
                        }
                    }
                    let mut result = json!({
                        "protocolVersion": 2,
                        "info": { "name": "deterministic-peer", "version": "1" },
                        "capabilities": { "session": {} },
                    });
                    let mut meta = serde_json::Map::new();
                    if args.dedup {
                        meta.insert(DEDUP_CONTRACT_META.to_owned(), json!({ "version": 1 }));
                    }
                    if args.live_reattach == Some("always")
                        || (args.live_reattach == Some("first") && *initializations == 1)
                    {
                        meta.insert(LIVE_REATTACH_META.to_owned(), json!({ "version": 1 }));
                    }
                    if args.resident_evidence.is_some() {
                        meta.insert(agent_provider_contract::acp::RESIDENT_SESSION_META.to_owned(), json!({
                            "protocol": "oulipoly.resident_session/v1", "acp_schema": agent_provider_contract::acp::pin::SCHEMA_TAG,
                        }));
                    }
                    if !meta.is_empty() {
                        result["_meta"] = Value::Object(meta);
                    }
                    send(
                        out,
                        &json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                    );
                }
                Some("session/new") => {
                    if args.mode == "exit-at-start" {
                        std::process::exit(1);
                    }
                    if args.mode == "refuse-start" {
                        send(
                            out,
                            &json!({"jsonrpc":"2.0", "id":id, "error":{"code":-32012, "message":"PRIVATE-START-PAYLOAD"}}),
                        );
                        continue;
                    }
                    let sessions = state["sessions"].as_array_mut().expect("sessions");
                    let session_id = format!("sess-{}", sessions.len() + 1);
                    sessions.push(Value::String(session_id.clone()));
                    save(&args.state, state);
                    if args.mode == "close-stdin" {
                        // SAFETY: closing our own stdin descriptor.
                        unsafe {
                            libc::close(0);
                        }
                        send(
                            out,
                            &json!({ "jsonrpc": "2.0", "id": id, "result": { "sessionId": session_id } }),
                        );
                        linger();
                    }
                    send(
                        out,
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
                    send(out, &reply);
                }
                Some("session/prompt") => {
                    let session_id = params["sessionId"].as_str().unwrap_or_default().to_owned();
                    let key = params["_meta"][MESSAGE_KEY_META].clone();
                    state["prompts"]
                        .as_array_mut()
                        .expect("prompts")
                        .push(json!({ "session": session_id, "key": key }));
                    save(&args.state, state);
                    if args.silent_after_acks.is_some_and(|limit| *acks >= limit) {
                        continue;
                    }
                    match args.mode.as_str() {
                        "silent" => continue,
                        "exit-before-ack-always" => std::process::exit(1),
                        _ => {}
                    }
                    let text = params["prompt"][0]["text"].as_str().unwrap_or_default();
                    if matches!(
                        args.resident_evidence.as_deref(),
                        Some("rejected" | "not-inserted")
                    ) {
                        send(
                            out,
                            &json!({"jsonrpc":"2.0", "id":id, "error": {
                                "code": if args.resident_evidence.as_deref() == Some("not-inserted") { -32010 } else { -32011 }, "message": "PRIVATE-RESIDENT-PAYLOAD",
                                "data": {"nativeTurn": {"request_id":"fixture", "custody":"complete", "status":{"code":0}},
                                         "recordError":{"message_id":"unacked-fixture", "record_error":"PRIVATE-RESIDENT-PAYLOAD"}}
                            }}),
                        );
                        continue;
                    }
                    if args.reject_prompt_once
                        && !text.starts_with("[Background Bash completion]")
                        && state["fault_used"] == false
                    {
                        state["fault_used"] = Value::Bool(true);
                        save(&args.state, state);
                        send(
                            out,
                            &json!({ "jsonrpc": "2.0", "id": id,
                            "error": { "code": -32011, "message": "fixture prompt rejected" } }),
                        );
                        continue;
                    }
                    if args.reject_completion_once
                        && text.starts_with("[Background Bash completion]")
                        && state["fault_used"] == false
                    {
                        state["fault_used"] = Value::Bool(true);
                        save(&args.state, state);
                        send(
                            out,
                            &json!({ "jsonrpc": "2.0", "id": id,
                            "error": { "code": -32001, "message": "fixture completion rejected" } }),
                        );
                        continue;
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
                        let message_id = format!("msg-{:04}", insertions.len() + 1);
                        insertions.push(json!({
                            "session": session_id,
                            "key": key,
                            "messageId": message_id,
                        }));
                        Value::String(message_id)
                    });
                    save(&args.state, state);
                    if args.mode == "insert-then-silent" {
                        continue;
                    }
                    if args.mode == "exit-before-ack-once" && state["fault_used"] == false {
                        state["fault_used"] = Value::Bool(true);
                        save(&args.state, state);
                        std::process::exit(0);
                    }
                    let mut result = json!({ "messageId": message_id });
                    if args.dedup {
                        result["_meta"] =
                            json!({ MESSAGE_KEY_META: key, DUPLICATE_META: duplicate });
                    }
                    // Deterministic beyond-window diagnostic: only after the
                    // host admitted and sent the next turn, for the first tag.
                    if args.resident_evidence.as_deref() == Some("later") && *acks > 0 {
                        let first_id = &state["insertions"][0]["messageId"];
                        send(
                            out,
                            &json!({"jsonrpc":"2.0", "method":"session/update", "params": {
                                "sessionId":session_id, "update":{"sessionUpdate":"session_info_update", "_meta": {
                                    agent_provider_contract::acp::NATIVE_TURN_META: {"message_id":first_id, "record_error":"PRIVATE-RESIDENT-PAYLOAD"}
                                }}
                            }}),
                        );
                    }
                    if *acks == 0 && !args.off_session_after_ack {
                        if let Some(other) = &args.off_session_turn {
                            off_session_turn(out, other, &message_id);
                        }
                    }
                    if args.mode == "off-session-no-ack" {
                        std::process::exit(0);
                    }
                    if !args.turn_before_ack {
                        send(
                            out,
                            &json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                        );
                    }
                    if *acks == 0 && args.off_session_after_ack {
                        if let Some(other) = &args.off_session_turn {
                            off_session_turn(out, other, &message_id);
                        }
                    }
                    if *acks == 0 && args.off_session_turn.is_some() {
                        send(
                            out,
                            &json!({ "jsonrpc": "2.0", "method": "session/update",
                            "params": { "sessionId": session_id, "update": {
                                "sessionUpdate": "notice", "severity": "info", "title": "off-session-sent"
                            } } }),
                        );
                        if let Some(gate) = &args.turn_gate {
                            while !gate.exists() {
                                std::thread::sleep(std::time::Duration::from_millis(10));
                            }
                        }
                    }
                    if args.turn_before_ack {
                        let mut update = json!({
                            "sessionUpdate": "agent_message",
                            "messageId": format!("reply-{}", message_id.as_str().unwrap()),
                            "content": [{ "type": "text", "text": text }],
                        });
                        if args.tagged {
                            update["_meta"] = json!({ PARENT_MESSAGE_META: message_id });
                        }
                        send(
                            out,
                            &json!({
                                "jsonrpc": "2.0", "method": "session/update",
                                "params": { "sessionId": session_id, "update": update },
                            }),
                        );
                    }
                    let command = text.rsplit_once("\n\n").map_or(text, |(_, last)| last);
                    let reply = if let Some(echo) = command.strip_prefix("echo:") {
                        Some(echo.to_owned())
                    } else if let Some(child) = command.strip_prefix("explore:") {
                        Some(run_child(child))
                    } else {
                        command.strip_prefix("spawn:").map(spawn)
                    };
                    if let Some(result) = reply {
                        let mut update = json!({
                            "sessionUpdate": "agent_message",
                            "messageId": format!("reply-{}", message_id.as_str().unwrap_or_default()),
                            "content": [{ "type": "text", "text": result }],
                        });
                        if args.tagged {
                            update["_meta"] = json!({ PARENT_MESSAGE_META: message_id });
                        }
                        send(
                            out,
                            &json!({
                                "jsonrpc": "2.0",
                                "method": "session/update",
                                "params": { "sessionId": session_id, "update": update },
                            }),
                        );
                    }
                    // An owner completion of background Bash: its facts line
                    // goes to the Bash requester as `@completion <facts>`.
                    let completion = text.starts_with("[Background Bash completion]").then(|| {
                        format!(
                            "@completion {}",
                            text.trim_end().rsplit('\n').next().unwrap_or("")
                        )
                    });
                    if let Some(command) = command.strip_prefix("bash:").or(completion.as_deref()) {
                        let result = run_bash(command);
                        // Kept too, in case no owner is attached to read it.
                        state["bash"]
                            .as_array_mut()
                            .map(|runs| runs.push(json!(result)))
                            .unwrap_or_else(|| state["bash"] = json!([result]));
                        save(&args.state, state);
                        let mut update = json!({
                            "sessionUpdate": "agent_message",
                            "messageId": format!("reply-{}", message_id.as_str().unwrap_or_default()),
                            "content": [{ "type": "text", "text": result }],
                        });
                        if args.tagged {
                            update["_meta"] = json!({ PARENT_MESSAGE_META: message_id });
                        }
                        send(
                            out,
                            &json!({
                                "jsonrpc": "2.0",
                                "method": "session/update",
                                "params": { "sessionId": session_id, "update": update },
                            }),
                        );
                    }
                    let mut idle = json!({ "sessionUpdate": "state_update", "state": "idle", "stopReason": "end_turn" });
                    if args.tagged {
                        idle["_meta"] = json!({ TURN_INPUT_META: message_id });
                    }
                    if let Some(shape) = args.resident_evidence.as_deref() {
                        let report = match shape {
                            "null" => Value::Null,
                            "invalid" => {
                                json!({"custody":"future", "private":"PRIVATE-RESIDENT-PAYLOAD"})
                            }
                            "incomplete" => json!({"request_id":"fixture", "custody":"incomplete"}),
                            _ => {
                                json!({"request_id":"fixture", "custody":"complete", "status":{"code":0}})
                            }
                        };
                        if shape != "absent" {
                            idle["_meta"][agent_provider_contract::acp::NATIVE_TURN_META] = report;
                        }
                        if shape == "covering" {
                            idle["_meta"][TURN_INPUT_META] = json!("zz-covering-tag");
                        }
                    }
                    if args.idle {
                        send(
                            out,
                            &json!({
                                "jsonrpc": "2.0",
                                "method": "session/update",
                                "params": { "sessionId": session_id, "update": idle },
                            }),
                        );
                    }
                    if args.resident_evidence.as_deref() == Some("late") {
                        send(
                            out,
                            &json!({"jsonrpc":"2.0", "method":"session/update", "params": {
                                "sessionId":session_id, "update":{"sessionUpdate":"session_info_update", "_meta": {
                                    agent_provider_contract::acp::NATIVE_TURN_META: {"message_id":message_id, "record_error":"PRIVATE-RESIDENT-PAYLOAD"}
                                }}
                            }}),
                        );
                    }
                    if args.turn_before_ack {
                        send(
                            out,
                            &json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                        );
                    }
                    *acks += 1;
                    if args.exit_after_acks == Some(*acks) {
                        std::process::exit(0);
                    }
                }
                _ => send(
                    out,
                    &json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": "method not found" } }),
                ),
            }
        }
    }
}

/// Asks for one registered child (`ROUTE:QUESTION`) through the production
/// child requester and returns its stdout (the result or refusal line).
fn run_child(request: &str) -> String {
    let client = std::env::current_exe()
        .expect("own path")
        .with_file_name("oulipoly-root-child");
    let (route, question) = request.split_once(':').unwrap_or((request, ""));
    match std::process::Command::new(client)
        .args([route, question])
        .stderr(std::process::Stdio::null())
        .output()
    {
        Ok(output) => format!(
            "exit={:?}\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).trim()
        ),
        Err(error) => format!("requester-not-run: {error}"),
    }
}

/// Starts `/bin/sh -c command` and does not wait for it.
fn spawn(command: &str) -> String {
    match std::process::Command::new("/bin/sh")
        .args(["-c", command])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => format!("spawned={}", child.id()),
        Err(error) => format!("spawn-failed: {error}"),
    }
}

/// Runs `command` through the root's Bash ingress via the prototype
/// requester and describes what came back: its exit and its output.
fn run_bash(command: &str) -> String {
    let client = std::env::current_exe()
        .expect("own path")
        .with_file_name("oulipoly-root-bash");
    match std::process::Command::new(client)
        .args(["--", "/bin/sh", "-c", command])
        .output()
    {
        Ok(output) => format!(
            "exit={:?}\nstdout={}\nstderr={}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        ),
        Err(error) => format!("requester-not-run: {error}"),
    }
}
