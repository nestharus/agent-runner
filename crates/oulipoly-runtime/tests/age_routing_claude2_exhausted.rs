#![cfg(unix)]

use chrono::{Duration, Utc};
use oulipoly_runtime::repl_default_provider::{RuntimeServices, run_repl_with_default_provider};
use oulipoly_runtime::services::ProductionRoutingService;
use oulipoly_state::QuotaWindowInput;
use oulipoly_state::StateDb;
use oulipoly_state::repositories::ProductionStateDbOpener;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn write_executable(path: &Path, body: &str) {
    fs::write(
        path,
        format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}\n"),
    )
    .unwrap();
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

fn quote_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

#[test]
fn default_provider_new_refreshes_stale_family_quota_before_launching() {
    let temp = tempfile::tempdir().unwrap();
    let config_root = temp.path().join("config");
    fs::create_dir_all(&config_root).unwrap();
    let state_path = temp.path().join("state.db");
    let state = StateDb::open(&state_path).unwrap();
    let marker = temp.path().join("launched-provider.txt");
    let claude = temp.path().join("claude-fixture.sh");
    let claude2 = temp.path().join("claude2-fixture.sh");
    write_executable(
        &claude,
        &format!("printf 'claude\\n' > \"{}\"\n", quote_path(&marker)),
    );
    write_executable(
        &claude2,
        &format!("printf 'claude2\\n' > \"{}\"\n", quote_path(&marker)),
    );
    fs::write(
        config_root.join("config.toml"),
        r#"default_provider = "claude""#,
    )
    .unwrap();

    let fresh_reset = (Utc::now() + Duration::hours(48)).to_rfc3339();
    fs::write(
        config_root.join("providers.toml"),
        format!(
            r#"[claude]
command = "{}"
interactive_args = ["launch"]
quota_script = "printf '%s' '{{\"windows\":[{{\"used_percent\":20,\"resets_at\":\"{fresh_reset}\"}}]}}'"

[claude2]
command = "{}"
interactive_args = ["launch"]
quota_script = "printf '%s' '{{\"windows\":[{{\"used_percent\":100,\"resets_at\":\"{fresh_reset}\"}}]}}'"
"#,
            quote_path(&claude),
            quote_path(&claude2)
        ),
    )
    .unwrap();

    state
        .upsert_quota_refresh(
            "claude",
            &[QuotaWindowInput {
                used_percent: 0.90,
                resets_at: Utc::now() + Duration::hours(48),
            }],
        )
        .unwrap();
    state
        .upsert_quota_refresh(
            "claude2",
            &[QuotaWindowInput {
                used_percent: 0.10,
                resets_at: Utc::now() + Duration::hours(48),
            }],
        )
        .unwrap();
    let stale_at = Utc::now() - Duration::seconds(31);
    state
        .set_refreshed_at_for_test("claude", &stale_at)
        .unwrap();
    state
        .set_refreshed_at_for_test("claude2", &stale_at)
        .unwrap();
    drop(state);

    let exit_code = run_repl_with_default_provider(RuntimeServices {
        config_root,
        state_db_path: Some(PathBuf::from(&state_path)),
        working_dir: None,
        state_db_opener: ProductionStateDbOpener,
        routing_service: Arc::new(ProductionRoutingService),
    })
    .unwrap();

    assert_eq!(exit_code, 0);
    assert_eq!(fs::read_to_string(&marker).unwrap(), "claude\n");
    let state = StateDb::open(&state_path).unwrap();
    assert!(
        state
            .get_windows("claude2")
            .unwrap()
            .iter()
            .any(|window| window.used_percent >= 1.0),
        "the stale claude2 cache should be refreshed to exhausted before the --new route is selected"
    );
}
