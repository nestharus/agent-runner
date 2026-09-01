#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub fn write_with_explicit_provider_authority(path: impl AsRef<Path>, contents: &str) {
    std::fs::write(path, with_explicit_provider_authority(contents)).unwrap();
}

pub fn with_explicit_provider_authority(contents: &str) -> String {
    let Ok(providers) = contents.parse::<toml::Table>() else {
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
                        name.as_str(),
                        FixtureProviderCapabilities::from_account(entry),
                    )
                })
        })
        .collect::<Vec<_>>();
    accounts.sort_unstable_by_key(|(name, _)| *name);

    let mut migrated = contents.trim_end().to_string();
    for (account, capabilities) in accounts {
        let family = format!("historical-test-fixture-{}", capabilities.slug());
        let executable = provider_endpoint_path(capabilities);
        migrated.push_str(&format!(
            "\n\n[{}.implementation]\nfamily = {:?}\nexecutable = {:?}",
            toml::Value::String(account.to_string()),
            family,
            executable.display().to_string(),
        ));
    }
    migrated.push('\n');
    migrated
}

pub fn with_explicit_provider_authority_at(
    contents: &str,
    family: &str,
    executable: &Path,
) -> String {
    let Ok(providers) = contents.parse::<toml::Table>() else {
        return contents.to_string();
    };
    let mut accounts = providers
        .iter()
        .filter_map(|(name, value)| {
            value
                .as_table()
                .filter(|entry| !entry.contains_key("implementation"))
                .map(|_| name.as_str())
        })
        .collect::<Vec<_>>();
    accounts.sort_unstable();

    let mut migrated = contents.trim_end().to_string();
    for account in accounts {
        migrated.push_str(&format!(
            "\n\n[{}.implementation]\nfamily = {:?}\nexecutable = {:?}",
            toml::Value::String(account.to_string()),
            family,
            executable.display().to_string(),
        ));
    }
    migrated.push('\n');
    migrated
}

pub fn with_explicit_account_authority_at(
    contents: &str,
    account: &str,
    family: &str,
    executable: &Path,
) -> String {
    let mut providers = contents.parse::<toml::Table>().unwrap();
    let account = providers
        .get_mut(account)
        .and_then(toml::Value::as_table_mut)
        .expect("provider account table");
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
    toml::to_string(&providers).unwrap()
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

impl FixtureProviderCapabilities {
    fn from_account(account: &toml::Table) -> Self {
        let launch = account.contains_key("command");
        let policy = account.contains_key("system_prompt_override")
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
            terminal: account.contains_key("resume_acceptance"),
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

fn provider_endpoint_path(capabilities: FixtureProviderCapabilities) -> PathBuf {
    static ENDPOINT_WRITE: OnceLock<Mutex<()>> = OnceLock::new();

    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/provider-authority-endpoint.py");
    let root = std::env::temp_dir().join(format!(
        "oulipoly-provider-authority-fixtures-{}",
        std::process::id()
    ));
    let endpoint = root.join(format!(
        "provider-authority-{}-endpoint.py",
        capabilities.slug()
    ));
    let _guard = ENDPOINT_WRITE
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("provider fixture endpoint write lock");
    std::fs::create_dir_all(&root).unwrap();
    let source = std::fs::read_to_string(source).unwrap();
    std::fs::write(
        &endpoint,
        source.replace("__OULIPOLY_FIXTURE_PROFILE__", &capabilities.slug()),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = std::fs::metadata(&endpoint).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&endpoint, permissions).unwrap();
    }
    endpoint
}
