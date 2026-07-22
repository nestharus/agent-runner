#![cfg(unix)]

mod age153_support;

use age153_support::{Age153Fixture, toml_string};
use rusqlite::params;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

const MODEL: &str = "provider-turn-recovery";
const PROVIDER: &str = "fixture-recovery-provider";
const DIAGNOSTICS_MODEL: &str = "provider-turn-recovery-diagnostics";
const DIAGNOSTICS_PROVIDER: &str = "fixture-recovery-diagnostics-provider";
const FORCE_KIND: &str = "OULIPOLY_AGE153_FORCE_TERMINAL_SIGNAL_KIND";

struct RecoveryFixture {
    base: Age153Fixture,
    completion_marker: PathBuf,
    diagnostics_marker: PathBuf,
}

impl RecoveryFixture {
    fn new(turn_mode: &str) -> Self {
        let base = Age153Fixture::new();
        let completion_marker = base.dir.path().join("completion-mode");
        let diagnostics_marker = base.dir.path().join("diagnostics-ran");
        let provider = base.write_script(
            "recovery-provider.sh",
            &provider_script(&completion_marker, turn_mode),
        );
        let diagnostics = base.write_script(
            "recovery-diagnostics.sh",
            &format!(
                "printf ran > {}\nprintf 'unknown\\n'",
                shell_path(&diagnostics_marker)
            ),
        );
        let turn_script = base.write_script(
            "recovery-turns.sh",
            &turn_script(&completion_marker, turn_mode),
        );
        write_fixture_config(&base, &provider, &diagnostics, &turn_script);
        Self {
            base,
            completion_marker,
            diagnostics_marker,
        }
    }

    fn run(&self, forced_kind: Option<&str>) -> Output {
        let envs = forced_kind
            .map(|kind| vec![(FORCE_KIND, kind)])
            .unwrap_or_default();
        self.base.run_one_shot_with_env(MODEL, &envs)
    }

    fn latest_invocation(&self) -> (String, i64, Option<String>) {
        self.base
            .conn()
            .query_row(
                "SELECT status, exit_code, terminal_reason
                 FROM invocations
                 WHERE provider_name = ?1
                 ORDER BY id DESC
                 LIMIT 1",
                params![PROVIDER],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap()
    }
}

#[test]
fn same_session_new_stop_recovers_logically_and_retains_physical_failure() {
    let fixture = RecoveryFixture::new("stop");

    let output = fixture.run(None);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let envelope = result_envelope(&output);
    assert_eq!(envelope["success"], true);
    assert_eq!(envelope["exit_code"], 1);
    assert_eq!(envelope["terminal_reason"], "exit_nonzero");
    assert_eq!(
        fixture.latest_invocation(),
        ("succeeded".to_string(), 1, Some("exit_nonzero".to_string()))
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("\"kind\":\"NonzeroExit\""), "{stderr}");
    assert!(
        !fixture.diagnostics_marker.exists(),
        "recovered completion must bypass failure diagnostics"
    );
    assert!(fixture.completion_marker.exists());
}

#[test]
fn stale_missing_wrong_session_and_non_stop_completion_remain_failed() {
    for mode in [
        "stale",
        "missing",
        "partial",
        "error",
        "wrong-session",
        "baseline-missing",
        "degraded",
        "new-stop-then-error",
    ] {
        let fixture = RecoveryFixture::new(mode);
        let output = fixture.run(None);
        assert_ne!(output.status.code(), Some(0), "mode={mode} {output:?}");
        let envelope = result_envelope(&output);
        assert_eq!(envelope["success"], false, "mode={mode}");
        assert_eq!(envelope["exit_code"], 1, "mode={mode}");
        assert_eq!(fixture.latest_invocation().0, "failed", "mode={mode}");
    }
}

#[test]
fn typed_terminal_failures_cannot_be_recovered_by_new_stop() {
    for kind in [
        "QuotaExhaustedInband",
        "RateLimited",
        "ProlongedSilence",
        "SignalExit",
        "Unknown",
    ] {
        let fixture = RecoveryFixture::new("stop");
        let output = fixture.run(Some(kind));
        assert_ne!(output.status.code(), Some(0), "kind={kind} {output:?}");
        assert_eq!(fixture.latest_invocation().0, "failed", "kind={kind}");
    }
}

fn write_fixture_config(
    fixture: &Age153Fixture,
    provider: &Path,
    diagnostics: &Path,
    turn_script: &Path,
) {
    fs::write(
        fixture.models_dir.join(format!("{MODEL}.toml")),
        format!("[[providers]]\nname = {PROVIDER:?}\nargs = []\n"),
    )
    .unwrap();
    fs::write(
        fixture.models_dir.join(format!("{DIAGNOSTICS_MODEL}.toml")),
        format!("[[providers]]\nname = {DIAGNOSTICS_PROVIDER:?}\nargs = []\n"),
    )
    .unwrap();
    fs::write(
        fixture.app_config_dir.join("config.toml"),
        format!("diagnostics_model = {DIAGNOSTICS_MODEL:?}\n"),
    )
    .unwrap();
    fs::write(
        fixture.app_config_dir.join("providers.toml"),
        format!(
            r#"[{PROVIDER}]
command = {}
args = []
prompt_mode = "arg"

[{PROVIDER}.session_capture]
kind = "forced_flag_verified"
flag = "--session-id"

[{DIAGNOSTICS_PROVIDER}]
command = {}
args = []
prompt_mode = "arg"
"#,
            toml_string(&provider.display().to_string()),
            toml_string(&diagnostics.display().to_string()),
        ),
    )
    .unwrap();
    fs::write(
        fixture.app_config_dir.join("sessions.toml"),
        format!(
            "[{PROVIDER}]\nturn_script = {}\n",
            toml_string(&turn_script.display().to_string())
        ),
    )
    .unwrap();
}

fn provider_script(marker: &Path, turn_mode: &str) -> String {
    format!(
        r#"session_id=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--session-id" ]; then
    session_id="$2"
    shift 2
  else
    shift
  fi
done
printf '%s' {turn_mode:?} > {}
printf '{{"type":"system","subtype":"init","session_id":"%s"}}\n' "$session_id"
printf 'earlier provider error evidence retained\n' >&2
exit 1"#,
        shell_path(marker)
    )
}

fn turn_script(marker: &Path, turn_mode: &str) -> String {
    let baseline = if turn_mode == "baseline-missing" {
        String::new()
    } else {
        r#"printf '{"session_id":"%s","turn_id":"old-stop","timestamp":"2026-07-21T00:00:00Z","role":"assistant","completion_outcome":"stop"}\n' "$SESSION_ID""#.to_string()
    };
    format!(
        r#"{baseline}
if [ ! -f {} ]; then
  exit 0
fi
mode="$(cat {})"
case "$mode" in
  stale) ;;
  missing)
    printf '{{"session_id":"%s","turn_id":"new-missing","timestamp":"2026-07-21T00:00:01Z","role":"assistant"}}\n' "$SESSION_ID"
    ;;
  partial)
    printf '{{"session_id":"%s","turn_id":"new-partial","timestamp":"2026-07-21T00:00:01Z","role":"assistant","completion_outcome":"tool-calls"}}\n' "$SESSION_ID"
    ;;
  error)
    printf '{{"session_id":"%s","turn_id":"new-error","timestamp":"2026-07-21T00:00:01Z","role":"assistant","completion_outcome":"error"}}\n' "$SESSION_ID"
    ;;
  wrong-session)
    printf '{{"session_id":"%s-other","turn_id":"new-stop","timestamp":"2026-07-21T00:00:01Z","role":"assistant","completion_outcome":"stop"}}\n' "$SESSION_ID"
    ;;
  degraded)
    printf '{{"session_id":"%s","turn_id":"new-stop","timestamp":"2026-07-21T00:00:01Z","role":"assistant","completion_outcome":"stop"}}\n' "$SESSION_ID"
    printf '{{"degraded":true,"count":2}}\n'
    ;;
  new-stop-then-error)
    printf '{{"session_id":"%s","turn_id":"new-stop","timestamp":"2026-07-21T00:00:01Z","role":"assistant","completion_outcome":"stop"}}\n' "$SESSION_ID"
    printf '{{"session_id":"%s","turn_id":"new-error","timestamp":"2026-07-21T00:00:02Z","role":"assistant","completion_outcome":"error"}}\n' "$SESSION_ID"
    ;;
  *)
    printf '{{"session_id":"%s","turn_id":"new-stop","timestamp":"2026-07-21T00:00:01Z","role":"assistant","completion_outcome":"stop"}}\n' "$SESSION_ID"
    ;;
esac"#,
        shell_path(marker),
        shell_path(marker),
    )
}

fn result_envelope(output: &Output) -> Value {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("OULIPOLY_RESULT="))
        .map(|line| serde_json::from_str(line).unwrap())
        .unwrap_or_else(|| panic!("missing result envelope: {output:?}"))
}

fn shell_path(path: &Path) -> String {
    format!("'{}'", path.display())
}
