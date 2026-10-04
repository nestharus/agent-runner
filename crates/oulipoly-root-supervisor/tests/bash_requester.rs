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
