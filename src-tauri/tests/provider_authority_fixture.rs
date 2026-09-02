#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub const FIXTURE_PROVIDER_INSTANCE_ID: &str = "historical-test-fixture-instance";

pub fn bind_session_authority(
    connection: &rusqlite::Connection,
    provider_name: &str,
    session_id: &str,
) {
    bind_session_authority_at(
        connection,
        provider_name,
        session_id,
        FIXTURE_PROVIDER_INSTANCE_ID,
        provider_name,
    );
}

pub fn bind_session_authority_at(
    connection: &rusqlite::Connection,
    provider_name: &str,
    session_id: &str,
    provider_instance_id: &str,
    settings_id: &str,
) {
    connection
        .execute(
            "INSERT OR IGNORE INTO session_chain_segment_provider_authority
                (segment_id, provider_instance_id, settings_id)
             SELECT id, ?3, ?4
             FROM session_chain_segments
             WHERE provider_name = ?1 AND session_id = ?2",
            rusqlite::params![provider_name, session_id, provider_instance_id, settings_id],
        )
        .unwrap();
}

pub fn bind_session_authority_with_cwd(
    connection: &rusqlite::Connection,
    provider_name: &str,
    session_id: &str,
    cwd: &Path,
) {
    bind_session_authority_with_cwd_at(
        connection,
        provider_name,
        session_id,
        FIXTURE_PROVIDER_INSTANCE_ID,
        provider_name,
        cwd,
    );
}

pub fn bind_session_authority_with_cwd_at(
    connection: &rusqlite::Connection,
    provider_name: &str,
    session_id: &str,
    provider_instance_id: &str,
    settings_id: &str,
    cwd: &Path,
) {
    bind_session_authority_at(
        connection,
        provider_name,
        session_id,
        provider_instance_id,
        settings_id,
    );
    connection
        .execute(
            "INSERT INTO imported_session_display_metadata (
                provider_name, provider_session_id, cwd, first_seen_at, last_seen_at
             ) VALUES (?1, ?2, ?3, '2026-09-02T00:00:00Z', '2026-09-02T00:00:00Z')
             ON CONFLICT(provider_name, provider_session_id) DO UPDATE SET
                cwd = excluded.cwd,
                last_seen_at = excluded.last_seen_at",
            rusqlite::params![provider_name, session_id, cwd.display().to_string()],
        )
        .unwrap();
}

pub fn write_with_explicit_provider_authority(path: impl AsRef<Path>, contents: &str) {
    std::fs::write(path, with_explicit_provider_authority(contents)).unwrap();
}

pub fn with_explicit_provider_authority(contents: &str) -> String {
    with_explicit_provider_authority_for_prompt_acceptance(contents, &[])
}

pub fn with_explicit_provider_authority_for_prompt_acceptance(
    contents: &str,
    prompt_acceptance_accounts: &[&str],
) -> String {
    let Ok(mut providers) = contents.parse::<toml::Table>() else {
        return contents.to_string();
    };
    let mut accounts = providers
        .iter()
        .filter_map(|(name, value)| {
            value
                .as_table()
                .filter(|entry| !entry.contains_key("implementation"))
                .map(|entry| {
                    (
                        name.to_string(),
                        FixtureProviderAdapter::from_account(
                            name,
                            entry,
                            prompt_acceptance_accounts.contains(&name.as_str()),
                        ),
                    )
                })
        })
        .collect::<Vec<_>>();
    accounts.sort_unstable_by(|left, right| left.0.cmp(&right.0));

    for (account, adapter) in accounts {
        let family = adapter.family();
        let executable = provider_endpoint_path(&adapter);
        configure_account_authority(&mut providers, &account, &family, &executable);
    }
    toml::to_string(&providers).unwrap()
}

pub fn with_explicit_provider_authority_at(
    contents: &str,
    family: &str,
    executable: &Path,
) -> String {
    let Ok(mut providers) = contents.parse::<toml::Table>() else {
        return contents.to_string();
    };
    let mut accounts = providers
        .iter()
        .filter_map(|(name, value)| {
            value
                .as_table()
                .filter(|entry| !entry.contains_key("implementation"))
                .map(|_| name.to_string())
        })
        .collect::<Vec<_>>();
    accounts.sort_unstable();

    for account in accounts {
        configure_account_authority(&mut providers, &account, family, executable);
    }
    toml::to_string(&providers).unwrap()
}

pub fn with_explicit_account_authority_at(
    contents: &str,
    account: &str,
    family: &str,
    executable: &Path,
) -> String {
    let mut providers = contents.parse::<toml::Table>().unwrap();
    configure_account_authority(&mut providers, account, family, executable);
    toml::to_string(&providers).unwrap()
}

fn configure_account_authority(
    providers: &mut toml::Table,
    account_name: &str,
    family: &str,
    executable: &Path,
) {
    let account = providers
        .get_mut(account_name)
        .and_then(toml::Value::as_table_mut)
        .expect("provider account table");
    account
        .entry("settings_id".to_string())
        .or_insert_with(|| toml::Value::String(account_name.to_string()));
    let mut implementation = toml::Table::new();
    implementation.insert(
        "family".to_string(),
        toml::Value::String(family.to_string()),
    );
    implementation.insert(
        "executable".to_string(),
        toml::Value::String(executable.display().to_string()),
    );
    account.insert(
        "implementation".to_string(),
        toml::Value::Table(implementation),
    );
}

#[derive(Clone, Copy)]
struct FixtureProviderCapabilities {
    launch: bool,
    policy: bool,
    quota: bool,
    session: bool,
    session_enumerate: bool,
    terminal: bool,
}

struct FixtureProviderAdapter {
    capabilities: FixtureProviderCapabilities,
    account_name: String,
    quota_script: Option<String>,
    auth_refresh_command: Option<String>,
    prompt_acceptance_patterns: Vec<String>,
    resume_args: Vec<String>,
    session_storage: Option<serde_json::Value>,
    session_capture_args: Vec<String>,
    session_capture_kind: Option<String>,
    session_capture_event_type: Option<String>,
    session_capture_event_id_path: Option<String>,
    session_capture_flag: Option<String>,
}

impl FixtureProviderAdapter {
    fn from_account(
        account_name: &str,
        account: &toml::Table,
        prompt_acceptance_enabled: bool,
    ) -> Self {
        let resume = account.get("resume").and_then(toml::Value::as_table);
        let resume_args =
            match resume.and_then(|table| table.get("kind").and_then(toml::Value::as_str)) {
                Some("flag") => resume
                    .and_then(|table| table.get("flag"))
                    .and_then(toml::Value::as_str)
                    .map(|flag| vec![flag.to_string()])
                    .unwrap_or_default(),
                Some("subcommand") => resume
                    .and_then(|table| table.get("subcommand"))
                    .and_then(toml::Value::as_array)
                    .map(|values| string_array(values))
                    .unwrap_or_default(),
                _ => Vec::new(),
            };
        let session_capture = account
            .get("session_capture")
            .and_then(toml::Value::as_table);
        let session_capture_args = session_capture
            .and_then(|table| table.get("json_args"))
            .and_then(toml::Value::as_array)
            .map(|values| string_array(values))
            .or_else(|| {
                session_capture
                    .and_then(|table| table.get("json_flag"))
                    .and_then(toml::Value::as_str)
                    .map(|flag| vec![flag.to_string()])
            })
            .unwrap_or_default();
        let prompt_acceptance_patterns = if prompt_acceptance_enabled {
            account
                .get("resume_acceptance")
                .and_then(toml::Value::as_table)
                .and_then(|table| table.get("accepted_output_patterns"))
                .and_then(toml::Value::as_array)
                .map(|values| string_array(values))
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        Self {
            capabilities: FixtureProviderCapabilities::from_account(account_name, account),
            account_name: account_name.to_string(),
            quota_script: account
                .get("quota_script")
                .and_then(toml::Value::as_str)
                .map(str::to_string),
            auth_refresh_command: account
                .get("auth_refresh_command")
                .and_then(toml::Value::as_str)
                .map(str::to_string),
            prompt_acceptance_patterns,
            resume_args,
            session_storage: account
                .get("session_storage")
                .and_then(|value| serde_json::to_value(value).ok()),
            session_capture_args,
            session_capture_kind: table_string(session_capture, "kind"),
            session_capture_event_type: table_string(session_capture, "event_type"),
            session_capture_event_id_path: table_string(session_capture, "event_id_path"),
            session_capture_flag: table_string(session_capture, "flag"),
        }
    }

    fn config_json(&self) -> String {
        serde_json::json!({
            "account_name": self.account_name,
            "quota_script": self.quota_script,
            "auth_refresh_command": self.auth_refresh_command,
            "prompt_acceptance_patterns": self.prompt_acceptance_patterns,
            "resume_args": self.resume_args,
            "session_storage": self.session_storage,
            "session_capture": {
                "args": self.session_capture_args,
                "kind": self.session_capture_kind,
                "event_type": self.session_capture_event_type,
                "event_id_path": self.session_capture_event_id_path,
                "flag": self.session_capture_flag,
            },
        })
        .to_string()
    }

    fn family(&self) -> String {
        let config_hash = sha256_hex(self.config_json().as_bytes());
        format!(
            "historical-test-fixture-{}-{}",
            self.capabilities.slug(),
            &config_hash[..12]
        )
    }
}

fn string_array(values: &[toml::Value]) -> Vec<String> {
    values
        .iter()
        .filter_map(toml::Value::as_str)
        .map(str::to_string)
        .collect()
}

fn table_string(table: Option<&toml::Table>, key: &str) -> Option<String> {
    table?
        .get(key)
        .and_then(toml::Value::as_str)
        .map(str::to_string)
}

impl FixtureProviderCapabilities {
    fn from_account(account_name: &str, account: &toml::Table) -> Self {
        let launch = account.contains_key("command");
        let policy = launch
            || account.contains_key("system_prompt_override")
            || account.contains_key("tool_restrictions");
        let quota =
            account.contains_key("quota_script") || account.contains_key("auth_refresh_command");
        let session = [
            "interactive_args",
            "resume",
            "session_capture",
            "session_storage",
            "resume_acceptance",
        ]
        .iter()
        .any(|key| account.contains_key(*key));
        Self {
            launch,
            policy,
            quota,
            session,
            session_enumerate: account.contains_key("session_storage"),
            terminal: account.contains_key("resume_acceptance")
                || account_name.starts_with("opencode"),
        }
    }

    fn slug(self) -> String {
        let mut slug = String::new();
        for (enabled, code) in [
            (self.launch, 'l'),
            (self.policy, 'p'),
            (self.quota, 'q'),
            (self.session, 's'),
            (self.session_enumerate, 'e'),
            (self.terminal, 't'),
        ] {
            if enabled {
                slug.push(code);
            }
        }
        if slug.is_empty() {
            slug.push('n');
        }
        slug
    }
}

fn provider_endpoint_path(adapter: &FixtureProviderAdapter) -> PathBuf {
    static ENDPOINT_WRITE: OnceLock<Mutex<()>> = OnceLock::new();

    let capabilities = adapter.capabilities;
    let config_json = adapter.config_json();
    let source_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/provider-authority-endpoint.py");
    let source = std::fs::read_to_string(source_path).unwrap();
    let endpoint_hash = sha256_hex(format!("{config_json}\0{source}").as_bytes());
    let root = std::env::temp_dir().join(format!(
        "oulipoly-provider-authority-fixtures-{}",
        std::process::id()
    ));
    let endpoint = root.join(format!(
        "provider-authority-{}-{}-endpoint.py",
        capabilities.slug(),
        &endpoint_hash[..12]
    ));
    let _guard = ENDPOINT_WRITE
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("provider fixture endpoint write lock");
    std::fs::create_dir_all(&root).unwrap();
    if endpoint.exists() {
        return endpoint;
    }
    let source = source
        .replace("__OULIPOLY_FIXTURE_PROFILE__", &capabilities.slug())
        .replace(
            "__OULIPOLY_FIXTURE_CONFIG_HEX__",
            &hex_bytes(config_json.as_bytes()),
        );
    let temporary = root.join(format!(".provider-authority-{}.tmp", uuid::Uuid::new_v4()));
    std::fs::write(&temporary, source).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = std::fs::metadata(&temporary).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&temporary, permissions).unwrap();
    }
    std::fs::rename(temporary, &endpoint).unwrap();
    endpoint
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};

    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn hex_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
