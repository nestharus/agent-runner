#![cfg(unix)]

use oulipoly_config::{SessionSourceEntry, SessionsConfig};
use oulipoly_runtime::sessions::scan_provider;
use oulipoly_state::StateDb;
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const INCIDENT_SESSION_ID: &str = "ses_0a151bb2cffese7DKbhwifCVXI";

struct LegacyScanFixture {
    _dir: tempfile::TempDir,
    db: StateDb,
    sessions: SessionsConfig,
    default_data_home: PathBuf,
    native_command_log: PathBuf,
}

impl LegacyScanFixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temporary provider storage");
        let default_data_home = dir.path().join("default-data");
        let profile3_data_home = dir.path().join("profile-3-data");
        let default_provider_root = default_data_home.join("opencode");
        let profile3_provider_root = profile3_data_home.join("opencode");
        let fake_bin_dir = dir.path().join("bin");
        let state_dir = dir.path().join("adapter-state");
        let native_command_log = dir.path().join("native-command.log");
        fs::create_dir_all(&default_provider_root).expect("default provider root");
        fs::create_dir_all(&profile3_provider_root).expect("profile-3 provider root");
        fs::create_dir_all(&fake_bin_dir).expect("fake native command directory");
        write_fake_opencode(&fake_bin_dir.join("opencode"));

        let adapter = repo_script_path("opencode-turns");
        assert!(adapter.is_file(), "source-controlled OpenCode adapter");
        let inherited_path = std::env::var_os("PATH").unwrap_or_default();
        let native_path = std::env::join_paths(
            std::iter::once(fake_bin_dir.clone()).chain(std::env::split_paths(&inherited_path)),
        )
        .expect("isolated native command PATH");
        let turn_script = format!(
            "env -u OPENCODE_BIN {} {} {} {} {} {}",
            shell_assignment("XDG_DATA_HOME", &profile3_data_home),
            shell_assignment("EXPECTED_PROFILE3_ROOT", &profile3_data_home),
            shell_assignment("FAKE_OPENCODE_LOG", &native_command_log),
            shell_assignment("PATH", Path::new(&native_path)),
            shell_quote_path(&adapter),
            shell_quote_path(&default_provider_root),
        );
        let sessions = SessionsConfig {
            entries: HashMap::from([(
                "opencode".to_string(),
                SessionSourceEntry {
                    turn_script,
                    transcript_locator: None,
                    state_dir: Some(state_dir),
                },
            )]),
        };
        let db = StateDb::open(&dir.path().join("state.db")).expect("isolated runner state");

        Self {
            _dir: dir,
            db,
            sessions,
            default_data_home,
            native_command_log,
        }
    }

    fn wrong_owner_state(&self) -> (u64, bool) {
        let turns = self
            .db
            .count_session_turns("opencode", INCIDENT_SESSION_ID)
            .expect("count wrong-owner turns")
            .total;
        let chain_exists = self
            .db
            .session_chain_segment_exists_for_provider_session("opencode", INCIDENT_SESSION_ID)
            .expect("query wrong-owner chain");
        (turns, chain_exists)
    }

    fn native_command_log(&self) -> String {
        fs::read_to_string(&self.native_command_log).expect("native command log")
    }
}

#[test]
fn unsuffixed_opencode_scan_does_not_mint_inherited_profile_session_owner() {
    let fixture = LegacyScanFixture::new();

    let report = scan_provider("opencode", &fixture.sessions, &fixture.db);

    assert_eq!(
        fixture.wrong_owner_state(),
        (0, false),
        "an opencode scan must not persist or mint profile-3 transcript data under opencode"
    );
    assert_eq!(report.errors, Vec::<String>::new());
    assert_eq!(
        fixture.native_command_log(),
        format!(
            "opencode|{}|session list --json\n",
            fixture.default_data_home.display()
        ),
        "the scan must call only the default command with the configured root's data home"
    );
}

fn repo_script_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("scripts")
        .join(name)
}

fn shell_assignment(name: &str, value: &Path) -> String {
    shell_quote(&format!("{name}={}", value.display()))
}

fn shell_quote_path(path: &Path) -> String {
    shell_quote(&path.display().to_string())
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn write_fake_opencode(path: &Path) {
    fs::write(
        path,
        format!(
            r#"#!/bin/sh
set -eu
printf '%s|%s|%s\n' "${{0##*/}}" "${{XDG_DATA_HOME:-}}" "$*" >>"$FAKE_OPENCODE_LOG"

if [ "${{XDG_DATA_HOME:-}}" != "$EXPECTED_PROFILE3_ROOT" ]; then
    printf '[]\n'
    exit 0
fi

if [ "$*" = "session list --json" ]; then
    printf '[{{"id":"{session_id}"}}]\n'
    exit 0
fi

if [ "$*" = "export {session_id}" ]; then
    cat <<'JSON'
{{
  "info": {{"id": "{session_id}", "time": {{"created": 1782000000000}}}},
  "messages": [
    {{
      "info": {{
        "id": "msg_profile3_only",
        "sessionID": "{session_id}",
        "role": "assistant",
        "time": {{"created": 1782000010000}}
      }},
      "parts": [{{"type": "text", "text": "profile-3 only"}}]
    }}
  ]
}}
JSON
    exit 0
fi

printf 'unexpected argv: %s\n' "$*" >&2
exit 64
"#,
            session_id = INCIDENT_SESSION_ID,
        ),
    )
    .expect("write fake OpenCode native command");
    let mut permissions = fs::metadata(path)
        .expect("fake OpenCode metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("make fake OpenCode executable");
}
