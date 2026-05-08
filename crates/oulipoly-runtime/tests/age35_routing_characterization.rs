#![cfg(unix)]

use chrono::Utc;
use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, ProviderEntry, ProvidersConfig, SessionSourceEntry,
    SessionsConfig,
};
use oulipoly_runtime::balancer::{BalanceContext, select_provider};
use oulipoly_runtime::quota::InFlight;
use oulipoly_state::StateDb;
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

struct ScriptFixture {
    _dir: tempfile::TempDir,
}

impl ScriptFixture {
    fn new() -> Self {
        Self {
            _dir: tempfile::tempdir().unwrap(),
        }
    }

    fn write_script(&self, name: &str, body: &str) -> PathBuf {
        let path = self._dir.path().join(name);
        fs::write(
            &path,
            format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}\n"),
        )
        .unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
        path
    }
}

fn two_provider_model() -> ModelConfig {
    ModelConfig {
        name: "age35-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![
            ProviderConfig::model_provider("age35-a", vec![]),
            ProviderConfig::model_provider("age35-b", vec![]),
        ],
        inputs: vec![],
    }
}

fn quota_script(fixture: &ScriptFixture, provider: &str, used_percent: u32) -> String {
    let resets_at = (Utc::now() + chrono::Duration::hours(24)).to_rfc3339();
    let script = fixture.write_script(
        &format!("{provider}-quota.sh"),
        &format!(
            r#"printf '%s\n' '{{"windows":[{{"used_percent":{used_percent},"resets_at":"{resets_at}"}}]}}'"#
        ),
    );
    script.display().to_string()
}

fn providers_config(fixture: &ScriptFixture) -> ProvidersConfig {
    let mut entries = HashMap::new();
    for (provider, used_percent) in [("age35-a", 10), ("age35-b", 20)] {
        entries.insert(
            provider.to_string(),
            ProviderEntry {
                quota_script: Some(quota_script(fixture, provider, used_percent)),
                ..ProviderEntry::default()
            },
        );
    }
    ProvidersConfig { entries }
}

fn session_script(fixture: &ScriptFixture, provider: &str) -> String {
    let script = fixture.write_script(
        &format!("{provider}-sessions.sh"),
        &format!(
            r#"printf '%s\n' '{{"session_id":"{provider}-session","turn_id":"turn-1","timestamp":"2026-04-17T08:00:00Z","role":"assistant"}}'"#
        ),
    );
    script.display().to_string()
}

fn sessions_config(fixture: &ScriptFixture) -> SessionsConfig {
    let mut entries = HashMap::new();
    for provider in ["age35-a", "age35-b"] {
        entries.insert(
            provider.to_string(),
            SessionSourceEntry {
                turn_script: session_script(fixture, provider),
                transcript_locator: None,
                state_dir: Some(
                    fixture
                        ._dir
                        .path()
                        .join(format!("{provider}-session-state")),
                ),
            },
        );
    }
    SessionsConfig { entries }
}

#[test]
fn age_35_select_provider_with_balance_context_refreshes_stale_quotas_and_scans_sessions() {
    let fixture = ScriptFixture::new();
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();
    let providers_cfg = providers_config(&fixture);
    let sessions_cfg = sessions_config(&fixture);
    let in_flight = InFlight::new();
    let ctx = BalanceContext {
        providers_cfg: &providers_cfg,
        sessions_cfg: &sessions_cfg,
        in_flight: &in_flight,
    };

    let selected = select_provider(&model, &db, Some(&ctx));

    assert!(selected < model.providers.len());
    for provider in ["age35-a", "age35-b"] {
        let quota = db
            .get_quota(provider)
            .unwrap()
            .unwrap_or_else(|| panic!("missing refreshed quota row for {provider}"));
        assert_eq!(
            quota.calls_since_refresh, 0,
            "quota refresh should reset calls_since_refresh for {provider}"
        );
        assert_eq!(
            db.get_windows(provider).unwrap().len(),
            1,
            "stale quota refresh should persist one window for {provider}"
        );
        assert_eq!(
            db.count_assistant_turns_since(provider, None).unwrap(),
            1,
            "select_provider(Some(ctx)) should scan session turns for {provider}"
        );
    }
}
