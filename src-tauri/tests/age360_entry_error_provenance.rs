//! Root-accepted entry contract: reject command-owned invalid input without
//! data/config effects; classify storage refusal by provenance, not path text.
#![cfg(unix)]

use std::path::Path;
use std::process::{Command, Output, Stdio};

const SESSION: &str = "99999999-9999-4999-8999-999999999999";

fn entry(root: &Path, data: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", root.join("home"))
        .env("CODEX_HOME", root.join("provider-home"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("xdg-data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_RUNTIME_DIR", root.join("runtime"))
        .env("OULIPOLY_DATA_DIR", data)
        .stdin(Stdio::null())
        .args(args)
        .output()
        .unwrap()
}

fn assert_no_effects(args: &[&str], code: i32, message: &str) -> Output {
    let root = tempfile::tempdir().unwrap();
    let output = entry(root.path(), &root.path().join("data"), args);
    assert_eq!(output.status.code(), Some(code), "{args:?}: {output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(message),
        "{output:?}"
    );
    assert_eq!(
        std::fs::read_dir(root.path()).unwrap().count(),
        0,
        "{args:?}"
    );
    output
}

#[test]
fn schema_looking_directory_obstructions_remain_operational() {
    for name in ["schema is incompatible", "unrecognized schema shape"] {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join(name);
        std::fs::write(&data, b"filesystem obstruction, not a database").unwrap();
        let output = entry(root.path(), &data, &["session", "import-replace", SESSION]);
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(output.stdout.is_empty(), "{output:?}");
        let json: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(json["error"]["code"], "operational-error", "{json}");
        let diagnostic = String::from_utf8_lossy(&output.stderr);
        assert!(diagnostic.contains(data.to_str().unwrap()), "{diagnostic}");
        assert!(
            diagnostic.contains("Failed to create state directory"),
            "{diagnostic}"
        );
        assert_eq!(
            std::fs::read(&data).unwrap(),
            b"filesystem obstruction, not a database"
        );
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }
}

#[test]
fn top_level_resume_agent_incompatibility_has_no_entry_effects() {
    // The nonexistent agent is intentional: rejection must precede agent loading.
    let output = assert_no_effects(
        &["--resume", SESSION, "-a", "/nonexistent-entry-agent.md"],
        1,
        "--resume is incompatible with --agent-file.",
    );
    assert_eq!(
        output.stderr,
        b"Error: --resume is incompatible with --agent-file.\n"
    );
}

#[test]
fn shared_sha_ttl_and_resume_handshake_rejections_have_no_entry_effects() {
    for sha in ["a".repeat(63), "g".repeat(64)] {
        assert_no_effects(
            &[
                "session",
                "import-replace",
                SESSION,
                "--preimage-sha256",
                &sha,
            ],
            2,
            "invalid-argument",
        );
    }
    assert_no_effects(
        &["session", "pause-handshake", SESSION, "--ttl-ms", "600001"],
        2,
        "invalid-ttl",
    );
    assert_no_effects(
        &[
            "session",
            "resume-handshake",
            "not-a-uuid",
            "--token",
            "not-consumed",
        ],
        2,
        "invalid-session-id",
    );
}

#[test]
fn shared_resume_id_routes_reject_before_entry_effects() {
    for (id, message) in [
        (" ".to_owned(), "session id is required"),
        ("a\nb".to_owned(), "session id contains control characters"),
        (
            "a".repeat(513),
            "session id exceeds maximum length of 512 bytes",
        ),
    ] {
        for args in [
            vec!["resume", &id, "--prompt", "test"],
            vec!["resume", "--session-id", &id, "--prompt", "test"],
            vec!["repl", "--resume", &id],
            vec!["--resume", &id],
        ] {
            assert_no_effects(&args, 1, message);
        }
    }
}

#[test]
fn valid_shared_validator_boundaries_still_reach_owner_bootstrap() {
    let sha = "AB".repeat(32);
    for args in [
        vec![
            "session",
            "import-replace",
            SESSION,
            "--preimage-sha256",
            &sha,
        ],
        vec!["session", "pause-handshake", SESSION, "--ttl-ms", "0"],
        vec!["session", "pause-handshake", SESSION, "--ttl-ms", "600000"],
        vec![
            "session",
            "resume-handshake",
            SESSION,
            "--token",
            "handler-owned",
        ],
    ] {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("blocked-owner-directory");
        std::fs::write(&data, b"obstruction").unwrap();
        let output = entry(root.path(), &data, &args);
        assert_eq!(output.status.code(), Some(1), "{args:?}: {output:?}");
        assert!(output.stdout.is_empty(), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("Failed to create state directory"),
            "{output:?}"
        );
        assert_eq!(std::fs::read(&data).unwrap(), b"obstruction");
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }
}
