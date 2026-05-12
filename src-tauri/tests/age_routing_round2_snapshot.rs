#![cfg(unix)]

use rusqlite::Connection;
use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const LIVE_CONFIG_DIR: &str = "/home/nes/.config/oulipoly-agent-runner";
const LIVE_DATA_DIR: &str = "/home/nes/.local/share/oulipoly-agent-runner";
const EXPECTED_PROVIDER: &str = "claude6";

#[derive(Clone, Copy)]
enum QuotaMode {
    MissingExplicitScripts,
    StubbedExplicitScripts,
}

struct SnapshotFixture {
    _temp: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    app_config_dir: PathBuf,
    data_app_dir: PathBuf,
    scripts_dir: PathBuf,
    home_dir: PathBuf,
    marker: PathBuf,
    workspace: PathBuf,
}

struct ResumeCandidate {
    provider_name: String,
    session_id: String,
}

impl SnapshotFixture {
    fn new(mode: QuotaMode) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let config_home = temp.path().join("config");
        let data_home = temp.path().join("data");
        let app_config_dir = config_home.join("oulipoly-agent-runner");
        let data_app_dir = data_home.join("oulipoly-agent-runner");
        let scripts_dir = temp.path().join("scripts");
        let home_dir = temp.path().join("home");
        let marker = temp.path().join("selected-provider.txt");
        let workspace = temp.path().join("workspace");

        copy_dir(Path::new(LIVE_CONFIG_DIR), &app_config_dir);
        fs::create_dir_all(&data_app_dir).unwrap();
        for suffix in ["", "-shm", "-wal"] {
            let source = Path::new(LIVE_DATA_DIR).join(format!("state.db{suffix}"));
            if source.exists() {
                fs::copy(&source, data_app_dir.join(format!("state.db{suffix}"))).unwrap();
            }
        }
        fs::create_dir_all(&scripts_dir).unwrap();
        fs::create_dir_all(&home_dir).unwrap();
        fs::create_dir_all(&workspace).unwrap();

        let fixture = Self {
            _temp: temp,
            config_home,
            data_home,
            app_config_dir,
            data_app_dir,
            scripts_dir,
            home_dir,
            marker,
            workspace,
        };
        fixture.write_blocking_quota_helpers();
        fixture.write_session_adapter_stubs();
        fixture.write_marker_commands();
        fixture.rewrite_quota_fields(mode);
        fixture
    }

    fn state_path(&self) -> PathBuf {
        self.data_app_dir.join("state.db")
    }

    fn conn(&self) -> Connection {
        Connection::open(self.state_path()).unwrap()
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(agent_binary());
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env("HOME", &self.home_dir);
        cmd.env("PATH", sanitized_path(&self.scripts_dir));
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        cmd.current_dir(&self.workspace);
        cmd
    }

    fn run_new(&self) -> Output {
        self.command().arg("--new").output().unwrap()
    }

    fn run_resume(&self, session_id: &str) -> Output {
        self.command()
            .arg("--resume")
            .arg(session_id)
            .output()
            .unwrap()
    }

    fn selected_provider(&self) -> Option<String> {
        fs::read_to_string(&self.marker)
            .ok()
            .map(|value| value.trim().to_string())
    }

    fn clear_marker(&self) {
        let _ = fs::remove_file(&self.marker);
    }

    fn write_marker_commands(&self) {
        let providers_path = self.app_config_dir.join("providers.toml");
        let mut root = read_toml_table(&providers_path);
        let provider_names = root.keys().cloned().collect::<Vec<_>>();

        for provider_name in provider_names {
            let Some(provider) = root
                .get_mut(&provider_name)
                .and_then(toml::Value::as_table_mut)
            else {
                continue;
            };
            if !provider.contains_key("command") {
                continue;
            }
            let script = self
                .scripts_dir
                .join(format!("{provider_name}-marker-command.sh"));
            write_executable(
                &script,
                &format!(
                    "printf '%s\\n' '{}' > {}\nprintf '%s\\n' '{{\"session_id\":\"fixture\"}}'\n",
                    provider_name,
                    shell_quote(&self.marker)
                ),
            );
            provider.insert(
                "command".to_string(),
                toml::Value::String(script.to_string_lossy().into_owned()),
            );
        }

        write_toml_table(&providers_path, &root);
    }

    fn write_blocking_quota_helpers(&self) {
        for helper in ["anthropic-usage", "chatgpt-usage", "zai-usage"] {
            write_executable(
                &self.scripts_dir.join(helper),
                "printf '%s\\n' 'quota helper intentionally stubbed in round2 snapshot test' >&2\nexit 64\n",
            );
        }
    }

    fn write_session_adapter_stubs(&self) {
        let workspace_json = serde_json::to_string(&self.workspace.to_string_lossy()).unwrap();
        write_executable(
            &self.scripts_dir.join("claude-code-cwd"),
            &format!(
                r#"base="${{1:?}}"
session="${{2:-${{SESSION_ID:-}}}}"
if [ -z "$session" ]; then
  printf '%s\n' '{{"found":false,"error":"missing session"}}'
  exit 0
fi
if find "$base" -name "$session.jsonl" -type f -print -quit | grep -q .; then
  printf '%s\n' '{{"found":true,"cwd":{workspace_json}}}'
else
  printf '%s\n' '{{"found":false}}'
fi
"#
            ),
        );
        write_executable(
            &self.scripts_dir.join("claude-code-locate-transcript"),
            r#"base="${1:?}"
session="${SESSION_ID:?}"
path="$(find "$base" -name "$session.jsonl" -type f -print -quit)"
if [ -z "$path" ]; then
  printf 'session not found: %s\n' "$session" >&2
  exit 1
fi
realpath "$path"
"#,
        );
        write_executable(&self.scripts_dir.join("claude-code-turns"), "exit 0\n");
        write_executable(&self.scripts_dir.join("codex-cwd"), "printf '%s\\n' '{}'\n");
        write_executable(
            &self.scripts_dir.join("codex-locate-transcript"),
            "exit 1\n",
        );
        write_executable(&self.scripts_dir.join("codex-turns"), "exit 0\n");
    }

    fn rewrite_quota_fields(&self, mode: QuotaMode) {
        let providers_path = self.app_config_dir.join("providers.toml");
        let mut root = read_toml_table(&providers_path);
        for (provider_name, value) in root.iter_mut() {
            let Some(provider) = value.as_table_mut() else {
                continue;
            };
            provider.remove("quota_script");
            provider.remove("auth_refresh_command");
            if matches!(mode, QuotaMode::StubbedExplicitScripts)
                && quota_provider_family(provider_name).is_some()
            {
                let quota_script = self.write_quota_script(provider_name);
                provider.insert(
                    "quota_script".to_string(),
                    toml::Value::String(quota_script.to_string_lossy().into_owned()),
                );
                if provider_name.starts_with("claude") {
                    provider.insert(
                        "auth_refresh_command".to_string(),
                        toml::Value::String("true".to_string()),
                    );
                }
            }
        }
        write_toml_table(&providers_path, &root);
    }

    fn write_quota_script(&self, provider_name: &str) -> PathBuf {
        let script = self.scripts_dir.join(format!("{provider_name}-quota.sh"));
        let windows = match provider_name {
            "claude" | "claude3" | "claude5" => {
                r#"[{"used_percent":100,"resets_at":"2099-01-01T00:00:00Z"}]"#
            }
            "claude2" => r#"[{"used_percent":98,"resets_at":"2099-01-01T00:00:00Z"}]"#,
            "claude4" => r#"[{"used_percent":92,"resets_at":"2099-01-01T00:00:00Z"}]"#,
            "claude6" => {
                r#"[{"used_percent":40,"resets_at":"2099-01-01T00:00:00Z"},{"used_percent":8,"resets_at":"2099-01-01T00:00:00Z"}]"#
            }
            "codex" => r#"[{"used_percent":85,"resets_at":"2099-01-01T00:00:00Z"}]"#,
            "codex2" => r#"[{"used_percent":0,"resets_at":"2099-01-01T00:00:00Z"}]"#,
            "codex3" => r#"[{"used_percent":1,"resets_at":"2099-01-01T00:00:00Z"}]"#,
            _ => r#"[{"used_percent":50,"resets_at":"2099-01-01T00:00:00Z"}]"#,
        };
        write_executable(
            &script,
            &format!(
                "printf '%s\\n' '{}'\n",
                format!(r#"{{"windows":{windows}}}"#)
            ),
        );
        script
    }

    fn find_resume_candidate(&self) -> ResumeCandidate {
        let conn = self.conn();
        let mut stmt = conn
            .prepare(
                r#"
                SELECT s.provider_name, s.session_id
                FROM session_chain_segments s
                JOIN session_chains c USING(chain_id)
                WHERE s.ended_at IS NULL
                  AND s.provider_name IN ('claude', 'claude3', 'claude5')
                  AND s.session_id IN (
                    SELECT session_id
                    FROM session_chain_segments
                    WHERE ended_at IS NULL
                    GROUP BY session_id
                    HAVING COUNT(*) = 1
                  )
                ORDER BY c.last_used_at DESC
                LIMIT 1
                "#,
            )
            .unwrap();
        stmt.query_row([], |row| {
            Ok(ResumeCandidate {
                provider_name: row.get(0)?,
                session_id: row.get(1)?,
            })
        })
        .expect("snapshot should contain an active exhausted Claude session")
    }

    fn seed_resume_transcript(&self, candidate: &ResumeCandidate) {
        let account_dir = self.home_dir.join(format!(".{}", candidate.provider_name));
        let project_dir = account_dir
            .join("projects")
            .join(claude_project_dir_name(&self.workspace));
        fs::create_dir_all(&project_dir).unwrap();
        let transcript = project_dir.join(format!("{}.jsonl", candidate.session_id));
        fs::write(
            transcript,
            format!(
                r#"{{"type":"assistant","uuid":"turn-1","timestamp":"2026-05-11T23:59:00+00:00","sessionId":"{}","message":{{"content":"fixture"}}}}"#,
                candidate.session_id
            ),
        )
        .unwrap();
    }
}

#[test]
#[ignore = "snapshot test uses the user's live Oulipoly config/state copies"]
fn round2_snapshot_missing_quota_scripts_routes_new_and_resume_to_claude6() {
    assert_snapshot_routes(QuotaMode::MissingExplicitScripts);
}

#[test]
#[ignore = "snapshot test uses the user's live Oulipoly config/state copies"]
fn round2_snapshot_restored_quota_scripts_routes_new_and_resume_to_claude6() {
    assert_snapshot_routes(QuotaMode::StubbedExplicitScripts);
}

fn assert_snapshot_routes(mode: QuotaMode) {
    let new_fixture = SnapshotFixture::new(mode);
    let new_output = new_fixture.run_new();
    assert_command_success("--new", &new_output);
    assert_eq!(
        new_fixture.selected_provider().as_deref(),
        Some(EXPECTED_PROVIDER),
        "--new should route to the best cached Claude account\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&new_output.stdout),
        String::from_utf8_lossy(&new_output.stderr)
    );

    let resume_fixture = SnapshotFixture::new(mode);
    let candidate = resume_fixture.find_resume_candidate();
    resume_fixture.seed_resume_transcript(&candidate);
    resume_fixture.clear_marker();
    let resume_output = resume_fixture.run_resume(&candidate.session_id);
    assert_command_success("--resume", &resume_output);
    assert_eq!(
        resume_fixture.selected_provider().as_deref(),
        Some(EXPECTED_PROVIDER),
        "--resume {} from {} should migrate to the best cached Claude account\nstdout:\n{}\nstderr:\n{}",
        candidate.session_id,
        candidate.provider_name,
        String::from_utf8_lossy(&resume_output.stdout),
        String::from_utf8_lossy(&resume_output.stderr)
    );
}

fn assert_command_success(label: &str, output: &Output) {
    assert!(
        output.status.success(),
        "{label} failed\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn agent_binary() -> PathBuf {
    if let Ok(path) = env::var("ROUND2_AGENT_BIN") {
        return PathBuf::from(path);
    }
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri should have a workspace parent");
    let tauri_release = workspace_root
        .join("src-tauri")
        .join("target")
        .join("release")
        .join("oulipoly-agent-runner");
    if tauri_release.exists() {
        return tauri_release;
    }
    let release = workspace_root
        .join("target")
        .join("release")
        .join("oulipoly-agent-runner");
    if release.exists() {
        return release;
    }
    PathBuf::from(
        option_env!("CARGO_BIN_EXE_oulipoly-agent-runner")
            .unwrap_or("target/debug/oulipoly-agent-runner"),
    )
}

fn sanitized_path(fixture_scripts_dir: &Path) -> std::ffi::OsString {
    env::join_paths([
        fixture_scripts_dir.to_path_buf(),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
    ])
    .unwrap()
}

fn quota_provider_family(provider_name: &str) -> Option<&'static str> {
    if provider_name.starts_with("claude") {
        Some("claude")
    } else if provider_name.starts_with("codex") {
        Some("codex")
    } else {
        None
    }
}

fn read_toml_table(path: &Path) -> toml::Table {
    fs::read_to_string(path)
        .unwrap()
        .parse::<toml::Table>()
        .unwrap()
}

fn write_toml_table(path: &Path, table: &toml::Table) {
    fs::write(path, toml::to_string_pretty(table).unwrap()).unwrap();
}

fn copy_dir(source: &Path, target: &Path) {
    fs::create_dir_all(target).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if source_path.is_dir() {
            copy_dir(&source_path, &target_path);
        } else {
            fs::copy(&source_path, &target_path).unwrap();
        }
    }
}

fn write_executable(path: &Path, body: &str) {
    fs::write(
        path,
        format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}"),
    )
    .unwrap();
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', r#"'\''"#))
}

fn claude_project_dir_name(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|ch| match ch {
            '/' | '\\' => '-',
            c if (c.is_ascii() && c.is_alphanumeric()) || c == '-' => c,
            _ => '-',
        })
        .collect()
}
