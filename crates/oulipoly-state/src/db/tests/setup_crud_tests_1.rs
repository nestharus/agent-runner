//! ## Declared roles
//!
//! - validator
//!
//! Role set: { validator }

use super::common::*;
use super::*;
#[test]
fn missing_provider_returns_none() {
    let db = test_db();
    assert!(db.get_provider("nonexistent", "missing").unwrap().is_none());
}

#[test]
fn upsert_and_list_cli_providers() {
    let db = test_db();
    db.upsert_cli_provider(&sample_provider()).unwrap();

    let mut p2 = sample_provider();
    p2.cli_name = "codex".to_string();
    p2.display_name = "OpenAI".to_string();
    db.upsert_cli_provider(&p2).unwrap();

    let providers = db.list_cli_providers().unwrap();
    assert_eq!(providers.len(), 2);
    assert_eq!(providers[0].cli_name, "claude");
    assert_eq!(providers[1].cli_name, "codex");
}

#[test]
fn upsert_cli_provider_updates_existing() {
    let db = test_db();
    db.upsert_cli_provider(&sample_provider()).unwrap();

    let mut updated = sample_provider();
    updated.version = Some("2.0.0".to_string());
    updated.last_synced = Some("2026-02-19T00:00:00Z".to_string());
    db.upsert_cli_provider(&updated).unwrap();

    let p = db.get_cli_provider("claude").unwrap().unwrap();
    assert_eq!(p.version.as_deref(), Some("2.0.0"));
    assert!(p.last_synced.is_some());
}

#[test]
fn get_cli_provider_missing() {
    let db = test_db();
    assert!(db.get_cli_provider("nonexistent").unwrap().is_none());
}

#[test]
fn insert_and_list_accounts() {
    let db = test_db();
    db.upsert_cli_provider(&sample_provider()).unwrap();

    let acct = AccountRecord {
        id: "work".to_string(),
        provider: "claude".to_string(),
        profile_name: "work-profile".to_string(),
        auth_method: AuthMethod::OAuth,
        auth_status: AuthStatus::Valid,
        created_at: "2026-02-19T00:00:00Z".to_string(),
    };
    db.insert_account(&acct).unwrap();

    let acct2 = AccountRecord {
        id: "personal".to_string(),
        provider: "claude".to_string(),
        profile_name: "personal-profile".to_string(),
        auth_method: AuthMethod::ApiKey {
            env_var: "ANTHROPIC_API_KEY".to_string(),
            config_path: None,
        },
        auth_status: AuthStatus::Unknown,
        created_at: "2026-02-19T00:00:00Z".to_string(),
    };
    db.insert_account(&acct2).unwrap();

    // List all
    let all = db.list_accounts(None).unwrap();
    assert_eq!(all.len(), 2);

    // List by provider
    let claude_accounts = db.list_accounts(Some("claude")).unwrap();
    assert_eq!(claude_accounts.len(), 2);
    assert_eq!(claude_accounts[0].id, "personal");
    assert_eq!(claude_accounts[1].id, "work");

    let empty = db.list_accounts(Some("codex")).unwrap();
    assert!(empty.is_empty());
}

#[test]
fn delete_account() {
    let db = test_db();
    db.upsert_cli_provider(&sample_provider()).unwrap();

    let acct = AccountRecord {
        id: "temp".to_string(),
        provider: "claude".to_string(),
        profile_name: "temp-profile".to_string(),
        auth_method: AuthMethod::ConfigFile {
            path: "~/.claude/config".to_string(),
        },
        auth_status: AuthStatus::NoAuth,
        created_at: "2026-02-19T00:00:00Z".to_string(),
    };
    db.insert_account(&acct).unwrap();
    assert_eq!(db.list_accounts(None).unwrap().len(), 1);

    let deleted = db.delete_account("temp", "claude").unwrap();
    assert!(deleted);
    assert!(db.list_accounts(None).unwrap().is_empty());

    // Deleting again returns false
    let deleted_again = db.delete_account("temp", "claude").unwrap();
    assert!(!deleted_again);
}

#[test]
fn auth_method_roundtrip() {
    let methods = vec![
        AuthMethod::OAuth,
        AuthMethod::ApiKey {
            env_var: "MY_KEY".to_string(),
            config_path: Some("/path/to/key".to_string()),
        },
        AuthMethod::ConfigFile {
            path: "~/.config/file".to_string(),
        },
    ];
    for method in methods {
        let serialized = method.to_db_string();
        let deserialized = AuthMethod::from_db_string(&serialized);
        assert_eq!(method, deserialized);
    }
}

#[test]
fn upsert_and_list_discovered_models() {
    let db = test_db();
    db.upsert_discovered_model(&sample_discovered_model("claude-opus-4", "claude"))
        .unwrap();
    db.upsert_discovered_model(&sample_discovered_model("claude-sonnet-4", "claude"))
        .unwrap();
    db.upsert_discovered_model(&sample_discovered_model("gpt-5.3", "codex"))
        .unwrap();

    // List all
    let all = db.list_discovered_models(None).unwrap();
    assert_eq!(all.len(), 3);

    // List by provider
    let claude_models = db.list_discovered_models(Some("claude")).unwrap();
    assert_eq!(claude_models.len(), 2);
    assert_eq!(claude_models[0].canonical_name, "claude-opus-4");
    assert_eq!(claude_models[1].canonical_name, "claude-sonnet-4");

    let codex_models = db.list_discovered_models(Some("codex")).unwrap();
    assert_eq!(codex_models.len(), 1);
    assert_eq!(codex_models[0].canonical_name, "gpt-5.3");

    let empty = db.list_discovered_models(Some("gemini")).unwrap();
    assert!(empty.is_empty());
}

#[test]
fn upsert_discovered_model_updates_existing() {
    let db = test_db();
    db.upsert_discovered_model(&sample_discovered_model("claude-opus-4", "claude"))
        .unwrap();

    let mut updated = sample_discovered_model("claude-opus-4", "claude");
    updated.cli_version = "2.0.0".to_string();
    updated.discovered_at = "2026-02-20T00:00:00Z".to_string();
    db.upsert_discovered_model(&updated).unwrap();

    let models = db.list_discovered_models(Some("claude")).unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].cli_version, "2.0.0");
    assert_eq!(models[0].discovered_at, "2026-02-20T00:00:00Z");
}

#[test]
fn delete_stale_models() {
    let db = test_db();
    db.upsert_discovered_model(&sample_discovered_model("model-a", "claude"))
        .unwrap();
    db.upsert_discovered_model(&sample_discovered_model("model-b", "claude"))
        .unwrap();

    let mut newer = sample_discovered_model("model-c", "claude");
    newer.cli_version = "2.0.0".to_string();
    db.upsert_discovered_model(&newer).unwrap();

    // Delete models with cli_version != "2.0.0"
    let deleted = db.delete_stale_models("claude", "2.0.0").unwrap();
    assert_eq!(deleted, 2);

    let remaining = db.list_discovered_models(Some("claude")).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].canonical_name, "model-c");
}

#[test]
fn delete_stale_models_different_provider() {
    let db = test_db();
    db.upsert_discovered_model(&sample_discovered_model("model-a", "claude"))
        .unwrap();
    db.upsert_discovered_model(&sample_discovered_model("model-b", "codex"))
        .unwrap();

    // Only delete stale models for "claude", "codex" should be untouched
    let deleted = db.delete_stale_models("claude", "2.0.0").unwrap();
    assert_eq!(deleted, 1);

    let codex = db.list_discovered_models(Some("codex")).unwrap();
    assert_eq!(codex.len(), 1);
}
