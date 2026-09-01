#![allow(dead_code)]

use std::path::{Path, PathBuf};

pub fn write_with_explicit_provider_authority(path: impl AsRef<Path>, contents: &str) {
    std::fs::write(path, with_explicit_provider_authority(contents)).unwrap();
}

pub fn with_explicit_provider_authority(contents: &str) -> String {
    with_explicit_provider_authority_at(
        contents,
        "historical-test-fixture",
        &provider_endpoint_path(),
    )
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

pub fn provider_endpoint_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/provider-authority-endpoint.py")
}
