//! A registered external provider as the root's one harness receiver.
//!
//! The request's `provider` (`{"executable": PATH}`) names the provider's
//! own executable, absolute, owned by this process's user or root and
//! writable by nobody else. Before any setup effect this entry asks it to
//! `describe` itself over the provider contract, with the client and
//! schema validation the Runner's provider registry uses, and agrees a
//! contract version that both declare. The provider, not this entry,
//! would then prepare its resident ACP v2 harness: native argv, auth,
//! policy and model translation are the provider's. The harness would go
//! to the owner as any other [`HarnessSpec`](oulipoly_root_supervisor::HarnessSpec),
//! under the owner's ACP v2 negotiation, logical authority and recipient
//! session fence, labelled with the provider's declared id.
//!
//! No provider contract this entry speaks can declare that harness yet.
//! `oulipoly.provider/v1` describe capabilities are a closed set with no
//! resident harness among them, and v1 has no operation that prepares a
//! harness launch. So a registered provider is refused once described,
//! naming what it declared and what is missing. It is never taken through
//! the embedded OpenCode or Claude setup instead.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use oulipoly_provider::client::ProviderClientOptions;
use oulipoly_provider::generated::{
    CONTRACT_VERSION, DescribeResult, EmptyParams, HostContext, RequestEnvelope,
};
use oulipoly_provider::resolver::ProviderArtifactRef;
use oulipoly_runtime::provider_registry::ProviderClientFactory;
use serde::Deserialize;
use serde_json::{Value, json};

const DESCRIBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Provider contract versions this entry speaks, most preferred first.
const SUPPORTED_CONTRACTS: &[&str] = &[CONTRACT_VERSION];

/// What the root needs a registered provider to declare, and no
/// supported contract can.
pub(super) const RESIDENT_HARNESS: &str = "resident-acp-v2-harness";

/// The request's `provider`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Registration {
    /// Absolute path of the provider's executable.
    pub(super) executable: String,
}

/// What a described provider declared, and the contract agreed with it.
#[derive(Debug)]
pub(super) struct Declared {
    pub(super) provider_id: String,
    pub(super) display_name: String,
    pub(super) contract_versions: Vec<String>,
    pub(super) preferred_contract: String,
    pub(super) contract: &'static str,
}

impl Declared {
    /// The `provider-described` entry line.
    pub(super) fn entry(&self) -> Value {
        json!({
            "entry": "provider-described",
            "provider_id": self.provider_id,
            "display_name": self.display_name,
            "contract_versions": self.contract_versions,
            "preferred_contract": self.preferred_contract,
            "agreed_contract": self.contract,
            "missing": [RESIDENT_HARNESS],
        })
    }

    /// Why the root cannot take this provider's harness.
    pub(super) fn unsupported(&self) -> String {
        format!(
            "provider: {} declares no {RESIDENT_HARNESS}: {} has no such capability \
             or harness preparation; not substituted by an embedded harness",
            self.provider_id, self.contract
        )
    }
}

/// The registration's own checks, before anything is run.
pub(super) fn check(registration: &Registration) -> Result<PathBuf, String> {
    let path = Path::new(&registration.executable);
    if !path.is_absolute() {
        return Err("provider.executable must be absolute".to_owned());
    }
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("provider.executable: {error}"))?;
    let meta =
        std::fs::metadata(&canonical).map_err(|error| format!("provider.executable: {error}"))?;
    // SAFETY: geteuid has no preconditions.
    let euid = unsafe { libc::geteuid() };
    if !meta.is_file() || meta.mode() & 0o111 == 0 {
        return Err("provider.executable is not an executable file".to_owned());
    }
    if (meta.uid() != euid && meta.uid() != 0) || meta.mode() & 0o022 != 0 {
        return Err(
            "provider.executable must be owned by this user or root and writable by no one else"
                .to_owned(),
        );
    }
    Ok(canonical)
}

/// Describes the provider and agrees a contract with it.
pub(super) fn describe(executable: PathBuf) -> Result<Declared, String> {
    let options = ProviderClientOptions::default().with_timeout(DESCRIBE_TIMEOUT);
    let client = ProviderClientFactory::new(options)
        .client_for(ProviderArtifactRef::Path { path: executable });
    let result = client
        .invoke_typed::<DescribeResult, _>("describe", describe_request(), [])
        .map_err(|error| format!("provider: describe failed: {error}"))?;
    let contract = agree(&result)?;
    Ok(Declared {
        provider_id: result.provider_id,
        display_name: result.display_name,
        contract_versions: result.contract_versions,
        preferred_contract: result.preferred_contract,
        contract,
    })
}

/// The provider's preferred contract if this entry speaks it, else this
/// entry's most preferred one that the provider declares.
fn agree(result: &DescribeResult) -> Result<&'static str, String> {
    SUPPORTED_CONTRACTS
        .iter()
        .find(|supported| **supported == result.preferred_contract)
        .or_else(|| {
            SUPPORTED_CONTRACTS
                .iter()
                .find(|supported| result.contract_versions.iter().any(|v| v == *supported))
        })
        .copied()
        .ok_or_else(|| {
            format!(
                "provider: {} declares no contract this entry speaks ({})",
                result.provider_id,
                SUPPORTED_CONTRACTS.join(", ")
            )
        })
}

fn describe_request() -> Value {
    serde_json::to_value(RequestEnvelope {
        contract: CONTRACT_VERSION.to_owned(),
        request_id: "native-root-describe".to_owned(),
        provider_instance_id: None,
        host: HostContext {
            app: "oulipoly-agent-runner".to_owned(),
            app_version: None,
            platform: None,
            working_directory: None,
            config_root: None,
            data_root: None,
            env: Default::default(),
            deadline_unix_ms: None,
        },
        params: EmptyParams {},
    })
    .expect("a describe request serializes")
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A deterministic external provider: answers `describe` with `result`
    /// and records each operation it was asked for in `<dir>/calls`.
    pub(in crate::commands::native_root) fn fake_provider(dir: &Path, result: &Value) -> PathBuf {
        let path = dir.join("fake-provider");
        let response = json!({
            "contract": CONTRACT_VERSION,
            "request_id": "native-root-describe",
            "ok": true,
            "result": result,
        });
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$1\" >> '{}'\ncat > /dev/null\nprintf '%s\\n' '{}'\n",
                dir.join("calls").display(),
                response
            ),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    pub(in crate::commands::native_root) fn described(provider_id: &str) -> Value {
        json!({
            "provider_id": provider_id,
            "display_name": "Fake external provider",
            "contract_versions": [CONTRACT_VERSION],
            "preferred_contract": CONTRACT_VERSION,
            "capabilities": {
                "launch": true, "policy": true, "quota": false, "session": true,
                "terminal": true, "rotation": false, "discovery": false,
                "settings": false, "setup_brain": false, "setup": false,
                "migration": false,
            },
        })
    }

    fn result(preferred: &str, versions: &[&str]) -> DescribeResult {
        let mut value = described("p");
        value["preferred_contract"] = json!(preferred);
        value["contract_versions"] = json!(versions);
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn agreement_takes_a_common_contract_or_refuses() {
        assert_eq!(
            agree(&result(CONTRACT_VERSION, &[CONTRACT_VERSION])),
            Ok(CONTRACT_VERSION)
        );
        // A newer preference this entry does not speak still agrees on a
        // declared common version.
        assert_eq!(
            agree(&result(
                "oulipoly.provider/v2",
                &["oulipoly.provider/v2", CONTRACT_VERSION]
            )),
            Ok(CONTRACT_VERSION)
        );
        let none = agree(&result("oulipoly.provider/v2", &["oulipoly.provider/v2"])).unwrap_err();
        assert!(none.contains("no contract this entry speaks"), "{none}");
    }

    #[test]
    fn registration_names_an_absolute_trusted_executable() {
        let dir = tempfile::tempdir().unwrap();
        let fake = fake_provider(dir.path(), &described("p"));
        let check_path = |path: &str| {
            check(&Registration {
                executable: path.to_owned(),
            })
        };
        assert_eq!(
            check_path(fake.to_str().unwrap()).unwrap(),
            fake.canonicalize().unwrap()
        );
        assert!(
            check_path("fake-provider")
                .unwrap_err()
                .contains("absolute")
        );
        assert!(check_path(dir.path().join("absent").to_str().unwrap()).is_err());
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o775)).unwrap();
        let writable = check_path(fake.to_str().unwrap()).unwrap_err();
        assert!(writable.contains("writable by no one else"), "{writable}");
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(check_path(fake.to_str().unwrap()).is_err());
    }

    /// The provider is really asked, over the provider contract, and what
    /// it declares is what the refusal names.
    #[test]
    fn describe_reaches_the_external_provider_and_names_what_it_declares() {
        let dir = tempfile::tempdir().unwrap();
        let fake = fake_provider(dir.path(), &described("fake-external"));
        let declared = describe(fake).unwrap();
        assert_eq!(declared.provider_id, "fake-external");
        assert_eq!(declared.contract, CONTRACT_VERSION);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("calls")).unwrap(),
            "describe\n"
        );
        let reason = declared.unsupported();
        assert!(
            reason.contains("fake-external declares no resident-acp-v2-harness"),
            "{reason}"
        );
        assert_eq!(declared.entry()["missing"], json!([RESIDENT_HARNESS]));
        // A describe answer outside the contract is a describe failure.
        let mut bad = described("fake-external");
        bad["capabilities"]["resident_acp_v2_harness"] = json!(true);
        let dir = tempfile::tempdir().unwrap();
        let fake = fake_provider(dir.path(), &bad);
        let failed = describe(fake).unwrap_err();
        assert!(failed.contains("describe failed"), "{failed}");
    }
}
