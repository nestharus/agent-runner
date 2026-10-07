//! Actual prototype process over a configured owner socket, no native agent.
//! Linux /dev/full supplies a real output-write failure; reply-loss follows
//! a real one-time fixture effect before the socket closes.
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn exec_failure_uses_ordered_stage_evidence_without_losing_wait_or_output() {
    let accepted = json!({ "event": "accepted", "durable": true, "work": 2 });
    let started = json!({ "event": "started", "pid": null, "exec_error": "fixture ENOENT" });
    let ordinary = json!({ "event": "started", "pid": 123, "exec_error": null });
    let output = json!({ "event": "output", "b64": "aGkK" });
    let closed = json!({ "event": "output-closed", "bytes": 3 });
    let end = json!({ "event": "end", "status": "code:127", "output": { "state": "closed" } });
    let cases = [
        (
            "exec-error",
            vec![
                accepted.clone(),
                started.clone(),
                output.clone(),
                closed.clone(),
                end.clone(),
            ],
            71,
            true,
        ),
        (
            "ordinary-127",
            vec![
                accepted.clone(),
                ordinary.clone(),
                output.clone(),
                closed.clone(),
                end.clone(),
            ],
            127,
            false,
        ),
        (
            "redundant-only",
            vec![
                accepted.clone(),
                ordinary,
                output.clone(),
                closed.clone(),
                json!({ "event": "end", "status": "code:127", "exec_error": "redundant ENOENT" }),
            ],
            127,
            false,
        ),
        (
            "lost-end",
            vec![
                accepted.clone(),
                started.clone(),
                output.clone(),
                closed.clone(),
            ],
            75,
            true,
        ),
        (
            "unknown-end",
            vec![
                accepted.clone(),
                started.clone(),
                output.clone(),
                json!({ "event": "end-unknown" }),
            ],
            75,
            true,
        ),
        (
            "bad-wait",
            vec![
                accepted.clone(),
                started.clone(),
                output.clone(),
                closed.clone(),
                json!({ "event": "end", "status": "unknown" }),
            ],
            75,
            true,
        ),
        (
            "no-closure",
            vec![
                accepted.clone(),
                started.clone(),
                output.clone(),
                end.clone(),
            ],
            74,
            true,
        ),
        (
            "bad-output",
            vec![
                accepted.clone(),
                started.clone(),
                json!({ "event": "output", "b64": "!!!!" }),
                closed.clone(),
                end.clone(),
            ],
            74,
            true,
        ),
        (
            "unwritten",
            vec![
                accepted.clone(),
                started.clone(),
                output.clone(),
                closed.clone(),
                end.clone(),
            ],
            74,
            true,
        ),
        (
            "start-before-accept",
            vec![
                started.clone(),
                accepted.clone(),
                output.clone(),
                closed.clone(),
                end.clone(),
            ],
            75,
            false,
        ),
        (
            "undurable",
            vec![
                json!({ "event": "accepted", "durable": false, "work": 2 }),
                started.clone(),
                output.clone(),
                closed.clone(),
                end.clone(),
            ],
            75,
            false,
        ),
        (
            "start-after-output",
            vec![
                accepted.clone(),
                output.clone(),
                started.clone(),
                closed.clone(),
                end.clone(),
            ],
            75,
            false,
        ),
        (
            "duplicate-start",
            vec![
                accepted.clone(),
                started.clone(),
                started.clone(),
                output.clone(),
                closed.clone(),
                end.clone(),
            ],
            75,
            true,
        ),
        (
            "duplicate-accept",
            vec![
                accepted.clone(),
                accepted.clone(),
                started.clone(),
                output.clone(),
                closed.clone(),
                end.clone(),
            ],
            75,
            false,
        ),
        (
            "failed-after-start",
            vec![
                accepted.clone(),
                started.clone(),
                output.clone(),
                closed.clone(),
                json!({ "event": "launch-failed", "not_started": true }),
            ],
            75,
            true,
        ),
        (
            "ordering-and-output-fault",
            vec![
                started.clone(),
                accepted.clone(),
                json!({ "event": "output-failed" }),
                output.clone(),
                closed.clone(),
                end.clone(),
            ],
            74,
            false,
        ),
        (
            "malformed-error",
            vec![
                accepted.clone(),
                json!({ "event": "started", "exec_error": 2 }),
                output.clone(),
                closed.clone(),
                end.clone(),
            ],
            75,
            false,
        ),
        (
            "empty-error",
            vec![
                accepted,
                json!({ "event": "started", "exec_error": "" }),
                output,
                closed,
                end,
            ],
            75,
            false,
        ),
    ];
    for (mode, events, code, failure_known) in cases {
        let dir = Fixture(std::env::temp_dir().join(format!(
            "requester-exec-{}",
            oulipoly_root_supervisor::sys::random_hex().unwrap()
        )));
        std::fs::create_dir(&dir.0).unwrap();
        let path = dir.0.join("owner.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let effect = dir.0.join("effects");
        let owner_effect = effect.clone();
        let owner = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut request = String::new();
            BufReader::new(&stream).read_line(&mut request).unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&request).unwrap()["op"],
                "run"
            );
            // A stand-in's accepted/setup effect, not execution of requested argv.
            std::fs::write(owner_effect, b"x").unwrap();
            for event in events {
                writeln!(stream, "{event}").unwrap();
            }
        });
        let mut requester = Command::new(env!("CARGO_BIN_EXE_oulipoly-root-bash"));
        requester
            .env("OULIPOLY_ROOT_BASH_V1", path)
            .arg("fixture-command")
            .current_dir(&dir.0)
            .stdin(Stdio::null());
        if mode == "unwritten" {
            requester.stdout(
                std::fs::OpenOptions::new()
                    .write(true)
                    .open("/dev/full")
                    .unwrap(),
            );
        }
        let result = requester.output().unwrap();
        owner.join().unwrap();
        let stderr = String::from_utf8(result.stderr).unwrap();
        eprintln!(
            "case={mode} fixture={} exit={:?} stdout={:?} stderr={stderr}",
            dir.0.display(),
            result.status.code(),
            result.stdout
        );
        assert_eq!(std::fs::read(effect).unwrap(), b"x");
        assert_eq!(result.status.code(), Some(code), "{mode}: {stderr}");
        let reports: Vec<Value> = stderr
            .lines()
            .map(|line| {
                serde_json::from_str(line.strip_prefix("oulipoly-root-bash: ").unwrap()).unwrap()
            })
            .collect();
        let failures: Vec<_> = reports
            .iter()
            .filter(|v| v["outcome"] == "requested-program-exec-failed")
            .collect();
        assert_eq!(
            failures.len(),
            usize::from(failure_known),
            "{mode}: {stderr}"
        );
        if failure_known {
            assert_eq!(failures[0]["exec_error"], "fixture ENOENT");
            assert_eq!(failures[0]["work"], 2);
            assert!(
                failures[0]["meaning"]
                    .as_str()
                    .unwrap()
                    .contains("setup effects")
            );
            assert!(
                failures[0]["meaning"]
                    .as_str()
                    .unwrap()
                    .contains("do not replay")
            );
        }
        if !matches!(mode, "lost-end" | "unknown-end" | "failed-after-start") {
            assert!(reports.iter().any(|v| v["event"] == "end"), "wait retained");
        }
        if !matches!(mode, "bad-output" | "unwritten") {
            assert_eq!(result.stdout, b"hi\n", "{mode}: actual output retained");
        }
    }
}

#[test]
fn actual_requester_distinguishes_output_failure_and_possible_effect_from_success() {
    for mode in [
        "normal",
        "unwritten",
        "missing-output",
        "corrupt-output",
        "lost-accept",
    ] {
        let dir = Fixture(std::env::temp_dir().join(format!(
            "requester-honesty-{}",
            oulipoly_root_supervisor::sys::random_hex().unwrap()
        )));
        std::fs::create_dir(&dir.0).unwrap();
        eprintln!("owned-fixture: {}", dir.0.display());
        let path = dir.0.join("owner.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let fixture_dir = dir.0.clone();
        let owner = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line).unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["op"], "run");
            assert!(
                Command::new("/bin/sh")
                    .args(["-c", "printf x >> effects"])
                    .current_dir(fixture_dir)
                    .status()
                    .unwrap()
                    .success()
            );
            if mode == "lost-accept" {
                return;
            }
            writeln!(stream, "{}", json!({ "event": "accepted" })).unwrap();
            if mode != "missing-output" {
                writeln!(stream, "{}", json!({ "event": "output", "b64": if mode == "corrupt-output" { "!!!!" } else { "aGkK" } })).unwrap();
                writeln!(
                    stream,
                    "{}",
                    json!({ "event": "output-closed", "bytes": 3 })
                )
                .unwrap();
            }
            writeln!(
                stream,
                "{}",
                json!({ "event": "end", "status": "code:0", "observer": "fixture-wait" })
            )
            .unwrap();
        });
        let mut requester = Command::new(env!("CARGO_BIN_EXE_oulipoly-root-bash"));
        requester
            .env("OULIPOLY_ROOT_BASH_V1", path)
            .arg("fixture-command")
            .current_dir(&dir.0)
            .stdin(Stdio::null());
        if mode == "unwritten" {
            requester.stdout(
                std::fs::OpenOptions::new()
                    .write(true)
                    .open("/dev/full")
                    .unwrap(),
            );
        }
        let result = requester.output().unwrap();
        owner.join().unwrap();
        assert_eq!(
            std::fs::read(dir.0.join("effects")).unwrap(),
            b"x",
            "one request effect, no retry"
        );
        let stderr = String::from_utf8(result.stderr).unwrap();
        if mode == "normal" {
            assert_eq!(result.status.code(), Some(0));
            assert_eq!(result.stdout, b"hi\n");
        } else if mode == "lost-accept" {
            assert_eq!(result.status.code(), Some(75));
            assert!(stderr.contains("possible-effect-unknown"));
        } else {
            assert_eq!(result.status.code(), Some(74), "{mode}: {stderr}");
            assert!(
                stderr.contains("code:0"),
                "retain wait separately: {stderr}"
            );
            assert!(stderr.contains("output-failed"));
        }
    }
}
