//! A registered external provider as the root's one harness.
//!
//! The request's `provider` names the provider's own executable and carries
//! what this entry never interprets: `settings` (the provider/v1
//! `policy.evaluate` params: `settings_id`, `mode`, `model`, `launch`), an
//! optional `config_root` and `env` (the provider's process environment and
//! `host.env` for its own operations). Beside them it carries Runner's own
//! policy for the root's tools: `bash_allow` or `bash_authority` and the
//! mediated Bash requester `agent_bash_bin`; and, when the root declares
//! child routes, `root_child_bin`, the child requester its exploration
//! offer names (required then, refused otherwise).
//!
//! Resolution, in order, each step only after the one before succeeded:
//!
//! 1. **Custody, before anything runs.** The executable path is walked from
//!    `/` by descriptor, never following a symlink: every directory and the
//!    file itself must be owned by root or this entry's user and writable by
//!    neither group nor others (a root-owned sticky directory may be
//!    world-writable: nobody else can replace what it holds). The file must
//!    be an executable regular file. The provider client then pins the file
//!    it will execute, and the pinned object's identity (device, inode,
//!    revision and bytes, as the client digests its retained handle) must be
//!    the assessed one's; otherwise nothing runs. Every operation below runs
//!    that one pinned object. The digest is reported for audit only; it is
//!    never compared with an earlier root's.
//! 2. **`describe`**, offering the resident-session and tool-mediation versions
//!    this entry supports through `host.env`, and exploration too for a
//!    parent offered child routes. The response is admitted by
//!    the provider contract crate's own registry and typed decoding, which
//!    tolerate advertisements this entry does not know (newer contract
//!    versions, capabilities) and require the preferred version to be a
//!    declared one. The contract, resident-session and tool-mediation versions
//!    (and exploration, for that parent) are then chosen by that crate's
//!    selection, from what the provider declared. A parent with child routes
//!    whose provider declares no exploration is refused here, before any
//!    preparation: it is not given Bash alone instead.
//! 3. **`policy.evaluate`** of opaque `settings`, with Runner's selected
//!    mediation policy inserted in `launch.env`, and the parent's
//!    exploration offer beside it. The evaluated env must preserve each
//!    exactly (and carry no offer where none was made), and exactly one
//!    schema-valid effective marker of each must agree with it: the
//!    exploration marker's routes and ingress with the offer's, its native
//!    tool among the mediation marker's `native_tools`. Native names are
//!    the provider's; this entry compares them, never supplies them.
//!    Its accepted argv and env are the resident launch template.
//! 4. **`resident.prepare`** of that template, with `host.data_root` the
//!    root's fresh `<launch_dir>/provider`. Its arguments, appended to the
//!    registered executable, are the root's harness on stdio.
//! 5. **Hand-over** (`host-root` only): the provider wrote its resident
//!    configuration and will keep its resident state under that data root,
//!    as root. The tree is given to the work identity, which serves the
//!    harness, by descriptor and without following links. Only directories
//!    and singly linked regular files are handed over; anything else is a
//!    setup failure. The launch directory itself stays the entry's, readable
//!    and traversable by the work identity's group, as for the embedded
//!    harnesses.
//!
//! `describe`, `policy.evaluate` and `resident.prepare` run as this entry
//! (host root under the packaged front door): trusted administrative
//! operations of a custody-checked asset. Only the harness, `resident.serve`
//! as the owner starts it, runs as the work identity, in the root's work
//! namespaces, with the root's environment and Bash ingress. The serve
//! process is started by the registered path, as the provider contract
//! intends (a compatible replacement there serves later relaunches); it runs
//! with the work identity's authority only.
//!
//! The provider's own operations have their own effects, which this entry
//! cannot see: after the first of them ran, no refusal or failure says that
//! nothing happened. Native argv, auth, session, model and tool translation
//! are the provider's. Logical session, ancestry, admission, scheduling and
//! mailbox authority stay with the owner and this Runner.

use std::collections::BTreeMap;
use std::ffi::{CString, OsStr};
use std::fs::File;
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use agent_provider_contract::SchemaRegistry as ContractRegistry;
use agent_provider_contract::exploration::{self, EffectiveExploration, Exploration, Limits};
use agent_provider_contract::generated::PolicyEvaluateResult;
use agent_provider_contract::negotiation::select_contract_version;
use agent_provider_contract::operations::Describe;
use agent_provider_contract::resident_session::{
    self, PREPARE_SUBCOMMAND, PROTOCOL, ResidentLaunchTemplate, ResidentPrepareParams,
    ResidentPrepareResult,
};
use agent_provider_contract::tool_mediation::{
    self, BashPolicy, EffectiveMediation, ToolMediation,
};
use oulipoly_provider::client::{ProviderClient, ProviderClientOptions};
use oulipoly_provider::generated::{CONTRACT_VERSION, EmptyParams, HostContext, RequestEnvelope};
use oulipoly_provider::resolver::ProviderArtifactRef;
use oulipoly_root_supervisor::bash::BashAuthority;
use oulipoly_root_supervisor::workload::Identity;
use oulipoly_runtime::provider_registry::ProviderClientFactory;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

/// Provider contract versions this entry speaks.
const SUPPORTED_CONTRACTS: &[&str] = &[CONTRACT_VERSION];

/// Resident-session extension versions this entry supports.
const RESIDENT_VERSIONS: &[u32] = &[1];

/// Exploration extension versions this entry offers.
const EXPLORATION_VERSIONS: &[u32] = &[1];

/// Where `resident.prepare` keeps the root's resident configuration and
/// state, under the launch directory.
const DATA_ROOT: &str = "provider";

/// The request's `provider`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Registration {
    /// Absolute path of the provider's executable.
    pub(super) executable: String,
    /// Opaque `policy.evaluate` params; Runner supplies selected launch.env mediation.
    pub(super) settings: Map<String, Value>,
    /// `host.config_root` of the provider's operations, unread here.
    #[serde(default)]
    pub(super) config_root: Option<String>,
    /// The provider's process environment and `host.env` for its own
    /// operations, unread here; never the root's environment.
    #[serde(default)]
    pub(super) env: BTreeMap<String, String>,
    /// The mediated Bash requester the provider's tools must use.
    pub(super) agent_bash_bin: String,
    /// The root-child requester a parent's exploration offer names: required
    /// when the root declares child routes, refused when it declares none.
    #[serde(default)]
    pub(super) root_child_bin: Option<String>,
    #[serde(default)]
    pub(super) bash_allow: Vec<String>,
    #[serde(default)]
    pub(super) bash_authority: Option<BashAuthority>,
}

/// A child route's registered provider (the request's
/// `children.routes.NAME.registered`): a [`Registration`] less its tool
/// policy, which is always the parent's, and less a child requester: nothing
/// here offers the child any route of its own.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ChildRegistration {
    executable: String,
    settings: Map<String, Value>,
    #[serde(default)]
    config_root: Option<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    agent_bash_bin: String,
}

impl ChildRegistration {
    /// The registration of a child that inherits `bash_allow` or
    /// `bash_authority` from its parent.
    pub(super) fn with_policy(
        &self,
        bash_allow: Vec<String>,
        bash_authority: Option<BashAuthority>,
    ) -> Registration {
        Registration {
            executable: self.executable.clone(),
            settings: self.settings.clone(),
            config_root: self.config_root.clone(),
            env: self.env.clone(),
            agent_bash_bin: self.agent_bash_bin.clone(),
            root_child_bin: None,
            bash_allow,
            bash_authority,
        }
    }
}

/// A refusal or failure of resolution, by whether the provider had run.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Failure {
    /// The provider ran (`operation` and possibly earlier ones); this entry
    /// made no setup.
    Provider {
        operation: &'static str,
        reason: String,
    },
    /// Setup construction failed after this entry's own setup began;
    /// `provider` says whether the provider had run.
    Setup { provider: bool, reason: String },
}

/// The registration's own checks, before anything is run.
pub(super) fn check(registration: &Registration) -> Result<(), String> {
    match (&registration.bash_allow[..], registration.bash_authority) {
        ([], None) => {
            return Err("provider: name bash_allow or bash_authority".to_owned());
        }
        ([_, ..], Some(_)) => {
            return Err("provider: bash_allow and bash_authority are exclusive".to_owned());
        }
        _ => {}
    }
    if registration
        .bash_allow
        .iter()
        .any(|command| command.trim().is_empty())
    {
        return Err("provider: bash_allow names an empty command".to_owned());
    }
    if !executable_file(&registration.agent_bash_bin) {
        return Err("provider: agent_bash_bin is not an absolute executable file".to_owned());
    }
    if let Some(requester) = &registration.root_child_bin
        && !executable_file(requester)
    {
        return Err("provider: root_child_bin is not an absolute executable file".to_owned());
    }
    // The host's offers travel in settings' launch.env; a caller's own
    // value there would stand in for (or forge) this entry's.
    if let Some(env) = registration
        .settings
        .get("launch")
        .and_then(|launch| launch.get("env"))
        .and_then(Value::as_object)
    {
        for name in [tool_mediation::ENV, exploration::ENV] {
            if env.contains_key(name) {
                return Err(format!(
                    "provider: settings launch.env sets {name}, this entry's to set"
                ));
            }
        }
    }
    for name in registration.env.keys() {
        if name.is_empty() || name.contains(['=', '\0']) {
            return Err(format!(
                "provider: env name {name:?} is not an environment name"
            ));
        }
        if name.starts_with("OULIPOLY_HOST_")
            || name == tool_mediation::ENV
            || name == exploration::ENV
        {
            return Err(format!("provider: env {name} is this entry's to set"));
        }
        if name == oulipoly_root_supervisor::bash::BASH_ENV
            || name == oulipoly_root_supervisor::SOCKET_ENV
        {
            return Err(format!("provider: env {name} is the owner's to set"));
        }
    }
    if registration.env.values().any(|value| value.contains('\0')) {
        return Err("provider: an env value is not an environment value".to_owned());
    }
    if let Some(root) = &registration.config_root
        && !root.starts_with('/')
    {
        return Err("provider: config_root must be absolute".to_owned());
    }
    Ok(())
}

fn executable_file(path: &str) -> bool {
    let path = Path::new(path);
    path.is_absolute()
        && std::fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.mode() & 0o111 != 0)
}

/// A parent's exploration offer: exactly the root's configured child
/// `routes` (the owner's route authority, nothing added or filtered), the
/// registration's `root_child_bin` and the owner's Bash ingress, which that
/// requester reads. `limits` are the root's own ceilings as the owner
/// enforces them, for the agent's information; the owner may refuse sooner
/// (used route slots). No routes, no offer. Checked before anything runs.
pub(super) fn offer(
    registration: &Registration,
    routes: Vec<String>,
    limits: Limits,
) -> Result<Option<Exploration>, String> {
    let requester = match (&registration.root_child_bin, routes.is_empty()) {
        (None, true) => return Ok(None),
        (Some(_), true) => {
            return Err(
                "provider: root_child_bin names a child requester, but the root declares no child routes"
                    .to_owned(),
            );
        }
        (None, false) => {
            return Err(
                "provider: the root declares child routes; name root_child_bin, the child requester its exploration offer names"
                    .to_owned(),
            );
        }
        (Some(requester), false) => requester.clone(),
    };
    let offer = Exploration {
        protocol: exploration::PROTOCOL.to_owned(),
        routes,
        requester,
        ingress_env: oulipoly_root_supervisor::bash::BASH_ENV.to_owned(),
        limits: Some(limits),
    };
    let value = serde_json::to_value(&offer).map_err(|error| error.to_string())?;
    exploration::validate("Exploration", &value)
        .map_err(|error| format!("children: routes cannot be offered: {error}"))?;
    Ok(Some(offer))
}

/// A custody-checked registered provider, pinned for every operation.
pub(super) struct Admitted<'a> {
    registration: &'a Registration,
    client: ProviderClient,
    /// The executed object's identity, for audit.
    pub(super) identity: String,
    /// The parent's exploration offer; a child or a parent without child
    /// routes has none, and then no exploration is selected either.
    pub(super) offer: Option<Exploration>,
}

/// Checks the executable's custody and pins it, running nothing. `offer`
/// is the parent's (see [`offer`]); a child's admission passes none.
pub(super) fn admit(
    registration: &Registration,
    offer: Option<Exploration>,
) -> Result<Admitted<'_>, String> {
    let path = Path::new(&registration.executable);
    let assessed = assess(path)?;
    let client = pin(path, &assessed)?;
    Ok(Admitted {
        registration,
        client,
        identity: assessed,
        offer,
    })
}

/// A provider client pinned to `path`, provided the object it pinned is
/// the `assessed` one; it runs nothing.
fn pin(path: &Path, assessed: &str) -> Result<ProviderClient, String> {
    let factory = ProviderClientFactory::new(ProviderClientOptions::default());
    let configured = factory.client_for(ProviderArtifactRef::Path {
        path: path.to_owned(),
    });
    let client = configured
        .fork_from_pinned(configured.options().clone())
        .map_err(|error| format!("provider.executable: {error}"))?;
    let pinned = client
        .pinned_executable_identity_sha256()
        .map_err(|error| format!("provider.executable: {error}"))?;
    if pinned != assessed {
        return Err(
            "custody: provider.executable changed between its custody check and its pin; \
             nothing was run"
                .to_owned(),
        );
    }
    Ok(client)
}

/// The executable's custody, by descriptor from `/`, and the identity of
/// the file it names.
fn assess(path: &Path) -> Result<String, String> {
    if !path.is_absolute() {
        return Err("provider.executable must be absolute".to_owned());
    }
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(part) => parts.push(part),
            _ => return Err("provider.executable must be a normalized absolute path".to_owned()),
        }
    }
    let leaf = parts.pop().ok_or("provider.executable names no file")?;
    let mut shown = PathBuf::from("/");
    let mut dir = open_at(None, OsStr::new("/"), libc::O_PATH | libc::O_DIRECTORY)
        .map_err(|error| format!("custody: /: {error}"))?;
    trusted(
        &File::from(dir.try_clone().map_err(|e| e.to_string())?),
        &shown,
        true,
    )?;
    for part in parts {
        shown.push(part);
        let next = open_at(Some(&dir), part, libc::O_PATH | libc::O_NOFOLLOW)
            .map_err(|error| format!("custody: {}: {error}", shown.display()))?;
        trusted(
            &File::from(next.try_clone().map_err(|e| e.to_string())?),
            &shown,
            true,
        )?;
        dir = next;
    }
    shown.push(leaf);
    let file = match open_at(Some(&dir), leaf, libc::O_RDONLY | libc::O_NOFOLLOW) {
        Ok(fd) => File::from(fd),
        Err(error) if error.raw_os_error() == Some(libc::ELOOP) => {
            return Err(format!("custody: {} is a symlink", shown.display()));
        }
        Err(error) => return Err(format!("custody: {}: {error}", shown.display())),
    };
    trusted(&file, &shown, false)?;
    identity(&file).map_err(|error| format!("custody: {}: {error}", shown.display()))
}

fn open_at(dir: Option<&OwnedFd>, name: &OsStr, flags: libc::c_int) -> std::io::Result<OwnedFd> {
    let name = CString::new(name.as_bytes()).map_err(std::io::Error::other)?;
    let dirfd = dir.map_or(libc::AT_FDCWD, AsRawFd::as_raw_fd);
    // SAFETY: a valid C string and descriptor; the result is checked.
    let fd = unsafe { libc::openat(dirfd, name.as_ptr(), flags | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: a fresh descriptor this function owns.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Owned by root or this user, writable by no one else (a root-owned sticky
/// directory excepted); a directory where one is expected, else an
/// executable regular file.
fn trusted(file: &File, shown: &Path, directory: bool) -> Result<(), String> {
    let meta = file
        .metadata()
        .map_err(|error| format!("custody: {}: {error}", shown.display()))?;
    if meta.file_type().is_symlink() {
        return Err(format!("custody: {} is a symlink", shown.display()));
    }
    // SAFETY: geteuid has no preconditions.
    let euid = unsafe { libc::geteuid() };
    if meta.uid() != 0 && meta.uid() != euid {
        return Err(format!(
            "custody: {} is owned by neither root nor this user",
            shown.display()
        ));
    }
    if directory {
        if !meta.is_dir() {
            return Err(format!("custody: {} is not a directory", shown.display()));
        }
        let sticky_root = meta.mode() & libc::S_ISVTX != 0 && meta.uid() == 0;
        if meta.mode() & 0o022 != 0 && !sticky_root {
            return Err(format!(
                "custody: {} is writable by group or others",
                shown.display()
            ));
        }
    } else {
        if !meta.is_file() || meta.mode() & 0o111 == 0 {
            return Err(format!(
                "custody: {} is not an executable file",
                shown.display()
            ));
        }
        if meta.mode() & 0o022 != 0 {
            return Err(format!(
                "custody: {} is writable by group or others",
                shown.display()
            ));
        }
    }
    Ok(())
}

/// The identity the provider client digests from its retained handle:
/// SHA-256 over the file's metadata stamp (device, inode, length,
/// modification and change times) and then its bytes. If the client's
/// digest ever differs in form, every registration is refused, never
/// admitted.
fn identity(file: &File) -> Result<String, String> {
    let stamp = |file: &File| -> Result<Vec<u8>, String> {
        let meta = file.metadata().map_err(|error| error.to_string())?;
        serde_json::to_vec(&(
            meta.dev(),
            meta.ino(),
            meta.len(),
            meta.mtime(),
            meta.mtime_nsec(),
            meta.ctime(),
            meta.ctime_nsec(),
        ))
        .map_err(|error| error.to_string())
    };
    let before = stamp(file)?;
    let mut digest = Sha256::new();
    digest.update(&before);
    let mut reader = file;
    let mut bytes = vec![0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut bytes).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        digest.update(&bytes[..read]);
    }
    if stamp(file)? != before {
        return Err("changed while it was read".to_owned());
    }
    Ok(format!("{:x}", digest.finalize()))
}

/// What a described provider declared, and what was agreed with it.
#[derive(Debug)]
pub(super) struct Declared {
    pub(super) provider_id: String,
    display_name: String,
    contract_versions: Vec<String>,
    preferred_contract: String,
    pub(super) contract: String,
    pub(super) resident_session: u32,
    pub(super) tool_mediation: u32,
    /// Selected only for an offering parent.
    pub(super) exploration: Option<u32>,
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
            "resident_session": self.resident_session,
            "tool_mediation": self.tool_mediation,
            "exploration": self.exploration,
        })
    }
}

/// A prepared resident harness.
#[derive(Debug)]
pub(super) struct Prepared {
    pub(super) argv: Vec<String>,
    pub(super) result: ResidentPrepareResult,
    pub(super) data_root: PathBuf,
    pub(super) template_env: Vec<String>,
    pub(super) effective_mediation: EffectiveMediation,
    pub(super) effective_exploration: Option<EffectiveExploration>,
}

/// The provider's evaluated template and its reported configuration.
/// These markers are not evidence that a live native CLI enforces them.
#[derive(Debug, Clone)]
pub(super) struct Evaluated {
    pub(super) launch: ResidentLaunchTemplate,
    pub(super) effective: EffectiveMediation,
    pub(super) exploration: Option<EffectiveExploration>,
}

impl Registration {
    pub(super) fn mediation(&self) -> ToolMediation {
        ToolMediation {
            protocol: tool_mediation::PROTOCOL.to_owned(),
            bash: match self.bash_authority {
                Some(BashAuthority::TrustedTask) => BashPolicy::Authority {
                    authority: tool_mediation::TRUSTED_TASK.to_owned(),
                },
                None => BashPolicy::Allow {
                    allow: self.bash_allow.clone(),
                },
            },
            requester: self.agent_bash_bin.clone(),
            ingress_env: oulipoly_root_supervisor::bash::BASH_ENV.to_owned(),
        }
    }
}

/// Only recognized structural codes may leave the private provider payload.
/// Code-shaped arbitrary text is still private; this is not a character filter.
fn public_refusal_code(code: &str) -> Option<&str> {
    match code {
        "missing_prompt" | "invalid_resident_argv" | "invalid_request" => Some(code),
        _ => None,
    }
}

impl Admitted<'_> {
    fn host(&self, data_root: Option<&Path>, selectors: bool) -> HostContext {
        let mut env = self.registration.env.clone();
        if selectors {
            env.extend(resident_session::FAMILY.host_selectors(RESIDENT_VERSIONS));
            env.extend(tool_mediation::FAMILY.host_selectors(tool_mediation::SUPPORTED_VERSIONS));
            if self.offer.is_some() {
                env.extend(exploration::FAMILY.host_selectors(EXPLORATION_VERSIONS));
            }
        }
        HostContext {
            app: "oulipoly-agent-runner".to_owned(),
            app_version: None,
            platform: Some(std::env::consts::OS.to_owned()),
            working_directory: None,
            config_root: self.registration.config_root.clone(),
            data_root: data_root.map(|root| root.to_string_lossy().into_owned()),
            env,
            deadline_unix_ms: None,
        }
    }

    fn invoke<P: serde::Serialize>(
        &self,
        operation: &'static str,
        host: HostContext,
        params: P,
    ) -> Result<Value, String> {
        self.envelope(operation, host, params)
            .map(|envelope| envelope["result"].clone())
    }

    /// The provider's success envelope for `operation`.
    fn envelope<P: serde::Serialize>(
        &self,
        operation: &'static str,
        host: HostContext,
        params: P,
    ) -> Result<Value, String> {
        let request = serde_json::to_value(RequestEnvelope {
            contract: CONTRACT_VERSION.to_owned(),
            request_id: format!("native-root-{operation}"),
            provider_instance_id: None,
            host,
            params,
        })
        .map_err(|error| error.to_string())?;
        let env: Vec<(String, String)> = self
            .registration
            .env
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        self.client
            .invoke_json(operation, request, env)
            .map_err(|error| {
                format!(
                    "provider: {operation} failed: {}{} (private details withheld)",
                    error.transport_kind(),
                    error
                        .provider_error_code()
                        .and_then(public_refusal_code)
                        .map(|code| format!("; endpoint code: {code}"))
                        .unwrap_or_default()
                )
            })
    }

    /// Describes the provider and selects the contract and resident-session
    /// versions with it.
    pub(super) fn describe(&self) -> Result<Declared, Failure> {
        let failed = |reason| Failure::Provider {
            operation: "describe",
            reason,
        };
        let envelope = self
            .envelope("describe", self.host(None, true), EmptyParams {})
            .map_err(failed)?;
        let bytes = serde_json::to_vec(&envelope).map_err(|error| failed(error.to_string()))?;
        let described = ContractRegistry::new()
            .decode_response::<Describe>(&bytes)
            .map_err(|error| failed(format!("provider: describe result: {error}")))?
            .into_inner()
            .result;
        let contract = select_contract_version(
            SUPPORTED_CONTRACTS,
            &described.contract_versions,
            &described.preferred_contract,
        )
        .map_err(|error| {
            failed(format!(
                "provider: {}: no contract version this entry speaks: {error}",
                described.provider_id
            ))
        })?;
        // Every advertisement, known or not, as the provider declared it.
        let capabilities = match serde_json::to_value(&described.capabilities) {
            Ok(Value::Object(capabilities)) => capabilities,
            _ => return Err(failed("provider: describe capabilities".to_owned())),
        };
        let resident_session =
            resident_session::select(RESIDENT_VERSIONS, &capabilities).map_err(|error| {
                failed(format!(
                    "provider: {} declares no resident session this entry supports ({error}); \
                     not substituted by an embedded harness",
                    described.provider_id
                ))
            })?;
        let tool_mediation = tool_mediation::select(tool_mediation::SUPPORTED_VERSIONS, &capabilities)
            .map_err(|error| failed(format!(
                "provider: {} declares no tool mediation this entry supports ({error}); tool authority refused",
                described.provider_id
            )))?;
        let exploration = match self.offer {
            Some(_) => Some(
                exploration::select(EXPLORATION_VERSIONS, &capabilities).map_err(|error| {
                    failed(format!(
                        "provider: {} declares no exploration this entry offers ({error}); \
                         the root declares child routes, so it is refused, not given Bash alone",
                        described.provider_id
                    ))
                })?,
            ),
            None => None,
        };
        Ok(Declared {
            provider_id: described.provider_id,
            display_name: described.display_name,
            contract_versions: described.contract_versions,
            preferred_contract: described.preferred_contract,
            contract,
            resident_session,
            tool_mediation,
            exploration,
        })
    }

    /// Evaluates opaque settings with requester-owned mediation in launch.env.
    /// Accepted settings alone never establish effective tool configuration.
    pub(super) fn template(&self) -> Result<Evaluated, Failure> {
        let failed = |reason| Failure::Provider {
            operation: "policy.evaluate",
            reason,
        };
        let tools = self.registration.mediation();
        let mut settings = Value::Object(self.registration.settings.clone());
        let launch = settings
            .get_mut("launch")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| failed("provider: settings launch must be an object".to_owned()))?;
        let env = launch.entry("env").or_insert_with(|| json!({}));
        if env.is_null() {
            *env = json!({});
        }
        let env = env
            .as_object_mut()
            .ok_or_else(|| failed("provider: settings launch.env must be an object".to_owned()))?;
        // The registration's tool authority owns these variables, not opaque
        // settings ([`check`] refused them before anything ran).
        for name in [tool_mediation::ENV, exploration::ENV] {
            if env.contains_key(name) {
                return Err(failed(format!(
                    "provider: settings sets {name}, this entry's to set"
                )));
            }
        }
        env.insert(tool_mediation::ENV.to_owned(), json!(tools.encode()));
        if let Some(offer) = &self.offer {
            env.insert(exploration::ENV.to_owned(), json!(offer.encode()));
        }
        let result = self
            .invoke("policy.evaluate", self.host(None, true), settings.clone())
            .map_err(failed)?;
        let policy: PolicyEvaluateResult = serde_json::from_value(result)
            .map_err(|_| failed("provider: policy result invalid (payload withheld)".to_owned()))?;
        let template =
            resident_session::template_from_policy(&settings, &policy).map_err(|error| {
                failed(match error {
                    resident_session::TemplateRefusal::NotAccepted(_) => {
                        format!(
                            "provider: policy refused the settings; endpoint codes: {:?} (diagnostic text withheld)",
                            policy.diagnostics.iter().filter_map(|d| d.code.as_deref().and_then(public_refusal_code)).collect::<Vec<_>>()
                        )
                    }
                    resident_session::TemplateRefusal::MissingArgv => {
                        "provider: policy named no argv".to_owned()
                    }
                    resident_session::TemplateRefusal::Unhonourable(field) => {
                        format!("provider: resident template cannot honour {field} transform")
                    }
                    resident_session::TemplateRefusal::Invalid(_) => {
                        "provider: invalid resident launch template (payload withheld)".to_owned()
                    }
                })
            })?;
        let env = template
            .launch
            .env
            .as_ref()
            .expect("shared helper supplies env");
        let echoed = ToolMediation::from_env(Some(&env)).map_err(|_| {
            failed("provider: evaluated mediation invalid (payload withheld)".to_owned())
        })?;
        if echoed.as_ref() != Some(&tools) {
            return Err(failed(
                "provider: evaluated env omits or changes requester-owned mediation".to_owned(),
            ));
        }
        if env.contains_key(&tools.ingress_env)
            || env.contains_key(oulipoly_root_supervisor::SOCKET_ENV)
        {
            return Err(failed(
                "provider: evaluated env overrides an owner endpoint".to_owned(),
            ));
        }
        let markers: Vec<_> = policy
            .markers
            .iter()
            .filter(|marker| marker.name == tool_mediation::MARKER)
            .collect();
        if markers.len() != 1 {
            return Err(failed(
                "provider: policy must report exactly one effective mediation marker".to_owned(),
            ));
        }
        tool_mediation::validate("EffectiveMediation", &markers[0].value).map_err(|_| {
            failed("provider: effective mediation marker invalid (payload withheld)".to_owned())
        })?;
        let effective: EffectiveMediation = serde_json::from_value(markers[0].value.clone())
            .map_err(|_| {
                failed("provider: effective mediation marker invalid (payload withheld)".to_owned())
            })?;
        if effective.protocol != tools.protocol
            || effective.bash != tools.bash
            || effective.ingress_env != tools.ingress_env
            || !effective.native_tools.contains(&effective.tool)
        {
            return Err(failed("provider: effective mediation contradicts requester-owned policy or its tool inventory".to_owned()));
        }
        let explored = self
            .explored(&env, &policy.markers, &effective)
            .map_err(failed)?;
        Ok(Evaluated {
            launch: template.launch,
            effective,
            exploration: explored,
        })
    }

    /// Prepares the resident harness under `<launch_dir>/provider`, which
    /// this creates; `launch_dir` must not exist.
    pub(super) fn prepare(
        &self,
        launch_dir: &Path,
        evaluated: Evaluated,
    ) -> Result<Prepared, Failure> {
        let template = evaluated.launch;
        let data_root = make_launch(launch_dir).map_err(|reason| Failure::Setup {
            provider: true,
            reason,
        })?;
        let failed = |reason| Failure::Setup {
            provider: true,
            reason,
        };
        let template_env = template
            .env
            .as_ref()
            .map_or_else(Vec::new, |env| env.keys().cloned().collect());
        let params = ResidentPrepareParams {
            protocol: PROTOCOL.to_owned(),
            launch: template,
        };
        let result = self
            .invoke(
                PREPARE_SUBCOMMAND,
                self.host(Some(&data_root), true),
                params,
            )
            .map_err(failed)?;
        let result = resident_session::decode_prepare_result(&result).map_err(|_| {
            failed("provider: resident.prepare result invalid (payload withheld)".to_owned())
        })?;
        let endpoint = agent_provider_contract::acp::resident::PreparedEndpoint::agree(&result)
            .map_err(|_| failed("provider: resident.prepare agreement refused".to_owned()))?;
        let argv = endpoint.argv(&self.registration.executable);
        Ok(Prepared {
            argv,
            result,
            data_root,
            template_env,
            effective_mediation: evaluated.effective,
            effective_exploration: evaluated.exploration,
        })
    }

    /// The evaluated env's exploration offer and the provider's marker for
    /// it, against this entry's offer: the same offer echoed (none where
    /// none was made), and one schema-valid marker naming the offered
    /// routes and ingress and a native tool the provider's own mediation
    /// inventory lists. A marker where nothing was offered is refused.
    fn explored(
        &self,
        env: &BTreeMap<String, String>,
        markers: &[agent_provider_contract::generated::Marker],
        mediation: &EffectiveMediation,
    ) -> Result<Option<EffectiveExploration>, String> {
        let echoed = Exploration::from_env(Some(env))
            .map_err(|_| "provider: evaluated exploration invalid (payload withheld)".to_owned())?;
        if echoed != self.offer {
            return Err(match self.offer {
                Some(_) => {
                    "provider: evaluated env omits or changes this entry's exploration offer"
                }
                None => {
                    "provider: evaluated env carries an exploration offer this entry did not make"
                }
            }
            .to_owned());
        }
        let reported: Vec<_> = markers
            .iter()
            .filter(|marker| marker.name == exploration::MARKER)
            .collect();
        let Some(offer) = &self.offer else {
            if reported.is_empty() {
                return Ok(None);
            }
            return Err("provider: policy reports exploration this entry did not offer".to_owned());
        };
        let [marker] = reported[..] else {
            return Err(
                "provider: policy must report exactly one effective exploration marker".to_owned(),
            );
        };
        exploration::validate("EffectiveExploration", &marker.value).map_err(|_| {
            "provider: effective exploration marker invalid (payload withheld)".to_owned()
        })?;
        let effective: EffectiveExploration = serde_json::from_value(marker.value.clone())
            .map_err(|_| {
                "provider: effective exploration marker invalid (payload withheld)".to_owned()
            })?;
        let routes = |routes: &[String]| {
            routes
                .iter()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>()
        };
        if effective.protocol != offer.protocol
            || routes(&effective.routes) != routes(&offer.routes)
            || effective.ingress_env != offer.ingress_env
            || !mediation.native_tools.contains(&effective.tool)
        {
            return Err("provider: effective exploration contradicts the offer or the provider's own tool inventory".to_owned());
        }
        Ok(Some(effective))
    }
}

/// Makes a fresh directory above child slots, private to this entry and,
/// when the work runs as another identity, traversable by its group.
pub(super) fn make_slot_base(path: &Path, identity: Option<&Identity>) -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(path)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if let Some(identity) = identity {
        std::os::unix::fs::lchown(path, None, Some(identity.gid))
            .and_then(|()| std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o750)))
            .map_err(|error| format!("{}: {error}", path.display()))?;
    }
    Ok(())
}

/// Makes the fresh launch directory and its provider data root, both
/// private to this entry.
fn make_launch(launch_dir: &Path) -> Result<PathBuf, String> {
    use std::os::unix::fs::DirBuilderExt;
    let mut builder = std::fs::DirBuilder::new();
    builder.mode(0o700);
    builder
        .create(launch_dir)
        .map_err(|error| format!("launch_dir: {error}"))?;
    let data_root = launch_dir.join(DATA_ROOT);
    builder
        .create(&data_root)
        .map_err(|error| format!("{}: {error}", data_root.display()))?;
    Ok(data_root)
}

/// Gives the provider's data root to the work identity (see the module
/// docs) and lets the identity's group traverse the launch directory.
/// Returns how many entries were handed over.
pub(super) fn hand_over(
    launch_dir: &Path,
    data_root: &Path,
    identity: &Identity,
) -> Result<u64, String> {
    let root = open_at(
        None,
        data_root.as_os_str(),
        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
    )
    .map_err(|error| format!("{}: {error}", data_root.display()))?;
    let count = hand_tree(&root, data_root, identity)?;
    chown_fd(&root, identity.uid, identity.gid)
        .map_err(|error| format!("{}: {error}", data_root.display()))?;
    let launch = open_at(
        None,
        launch_dir.as_os_str(),
        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
    )
    .map_err(|error| format!("{}: {error}", launch_dir.display()))?;
    // SAFETY: a valid descriptor; results are checked.
    let changed = unsafe {
        libc::fchown(launch.as_raw_fd(), u32::MAX, identity.gid) == 0
            && libc::fchmod(launch.as_raw_fd(), 0o750) == 0
    };
    if !changed {
        return Err(format!(
            "{}: {}",
            launch_dir.display(),
            std::io::Error::last_os_error()
        ));
    }
    Ok(count + 1)
}

fn hand_tree(dir: &OwnedFd, shown: &Path, identity: &Identity) -> Result<u64, String> {
    let listing = File::from(dir.try_clone().map_err(|error| error.to_string())?);
    let names: Vec<_> = std::fs::read_dir(format!("/proc/self/fd/{}", listing.as_raw_fd()))
        .map_err(|error| format!("{}: {error}", shown.display()))?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<Result<_, _>>()
        .map_err(|error| format!("{}: {error}", shown.display()))?;
    let mut count = 0;
    for name in names {
        let path = shown.join(&name);
        let entry = open_at(Some(dir), &name, libc::O_PATH | libc::O_NOFOLLOW)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let meta = File::from(entry.try_clone().map_err(|error| error.to_string())?)
            .metadata()
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if meta.is_dir() {
            let child = open_at(
                Some(dir),
                &name,
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
            )
            .map_err(|error| format!("{}: {error}", path.display()))?;
            count += hand_tree(&child, &path, identity)?;
            chown_fd(&child, identity.uid, identity.gid)
                .map_err(|error| format!("{}: {error}", path.display()))?;
        } else if meta.is_file() && meta.nlink() == 1 {
            let file = open_at(Some(dir), &name, libc::O_RDONLY | libc::O_NOFOLLOW)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            chown_fd(&file, identity.uid, identity.gid)
                .map_err(|error| format!("{}: {error}", path.display()))?;
        } else {
            return Err(format!(
                "{}: only directories and singly linked files are handed to the work identity",
                path.display()
            ));
        }
        count += 1;
    }
    Ok(count)
}

fn chown_fd(fd: &OwnedFd, uid: u32, gid: u32) -> std::io::Result<()> {
    // SAFETY: a valid descriptor; the result is checked.
    if unsafe { libc::fchown(fd.as_raw_fd(), uid, gid) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A deterministic external provider (Python): answers `describe`
    /// (offering `resident_session_v1`, `tool_mediation_v1` and
    /// `exploration_v1` only when selected, unless `script` says otherwise),
    /// `policy.evaluate` (echoing the offers, reporting a marker for each,
    /// refusing an offer its request did not select, as the contract's
    /// `exploration::admit` would) and `resident.prepare`, and records each
    /// operation, its euid and its request in `<dir>/calls`. Its native tool
    /// names are its own stand-ins.
    pub(in crate::commands::native_root) fn fake_provider(dir: &Path, script: &str) -> PathBuf {
        let path = dir.join("fake-provider");
        std::fs::write(&path, fake_source(dir, script)).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    pub(in crate::commands::native_root) fn fake_source(dir: &Path, script: &str) -> String {
        format!(
            r#"#!/usr/bin/python3
import hashlib, json, os, sys
CALLS = {calls:?}
op = sys.argv[1]
request = json.loads(sys.stdin.read()) if op != "resident.serve" else None
with open(CALLS, "a") as f:
    f.write(json.dumps({{"op": op, "euid": os.geteuid(), "argv": sys.argv[1:], "request": request}}) + "\n")
def answer(result):
    print(json.dumps({{"contract": "oulipoly.provider/v1", "request_id": request["request_id"], "ok": True, "result": result}}))
selected = request and request["host"].get("env", {{}}).get("OULIPOLY_HOST_RESIDENT_SESSION_V1") == "1"
caps = {{"launch": True, "policy": True, "quota": False, "session": False, "terminal": False,
        "rotation": False, "discovery": False, "settings": False, "setup_brain": False,
        "setup": False, "migration": False}}
if selected:
    caps["resident_session_v1"] = True
mediation_env = request and request["params"].get("launch", {{}}).get("env", {{}}).get("OULIPOLY_TOOL_MEDIATION_V1")
mediation = json.loads(mediation_env) if mediation_env else None
marker = dict(mediation or {{}})
marker.pop("requester", None)
marker.update(tool="fake_mediated_bash", native_tools=["fake_mediated_bash"])
markers = [{{"name": "oulipoly.tool_mediation/v1", "value": marker}}]
if request and request["host"].get("env", {{}}).get("OULIPOLY_HOST_TOOL_MEDIATION_V1") == "1":
    caps["tool_mediation_v1"] = True
explore_selected = bool(request) and request["host"].get("env", {{}}).get("OULIPOLY_HOST_EXPLORATION_V1") == "1"
if explore_selected:
    caps["exploration_v1"] = True
exploration_env = request and request["params"].get("launch", {{}}).get("env", {{}}).get("OULIPOLY_EXPLORATION_V1")
exploration = json.loads(exploration_env) if exploration_env else None
if exploration:
    explore_marker = {{"protocol": exploration["protocol"], "routes": exploration["routes"],
                      "tool": "fake_explore", "ingress_env": exploration["ingress_env"]}}
    markers.append({{"name": "oulipoly.exploration/v1", "value": explore_marker}})
    marker["native_tools"].append("fake_explore")
{script}
if op == "policy.evaluate" and exploration_env and not explore_selected:
    answer({{"accepted": False, "stdin": None, "prompt": None, "markers": [],
            "diagnostics": [{{"code": "exploration", "message": "offer not selected", "severity": "error"}}]}})
elif op == "describe":
    answer({{"provider_id": "fake-external", "display_name": "Fake external provider",
            "contract_versions": ["oulipoly.provider/v1"], "preferred_contract": "oulipoly.provider/v1",
            "capabilities": caps}})
elif op == "policy.evaluate":
    policy_env = {{"FAKE_POLICY": "1", "OULIPOLY_TOOL_MEDIATION_V1": mediation_env}}
    if exploration_env:
        policy_env["OULIPOLY_EXPLORATION_V1"] = exploration_env
    answer({{"accepted": True, "argv": ["fake-native", "--model", request["params"]["model"]["name"]],
            "env": policy_env, "stdin": None, "prompt": None, "diagnostics": [], "markers": markers}})
elif op == "resident.prepare":
    data = request["host"]["data_root"]
    config = json.dumps(request["params"]["launch"], sort_keys=True).encode()
    digest = hashlib.sha256(config).hexdigest()
    os.makedirs(os.path.join(data, "configs"), mode=0o700)
    path = os.path.join(data, "configs", digest + ".json")
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    os.write(fd, config)
    os.close(fd)
    answer({{"protocol": "oulipoly.resident_session/v1",
            "invocation": {{"args": ["resident.serve", "--config", path], "endpoint": "stdio"}},
            "acp": {{"protocol_version": 2, "schema": "schema-v2.0.0-alpha.7", "dedup_contract": 1}},
            "config_sha256": digest,
            "operations": ["initialize", "session/new", "session/resume", "session/prompt",
                           "session/cancel", "session/close", "session/list"]}})
else:
    sys.exit(3)
"#,
            calls = dir.join("calls").to_string_lossy()
        )
    }

    pub(in crate::commands::native_root) fn calls(dir: &Path) -> Vec<Value> {
        std::fs::read_to_string(dir.join("calls"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    pub(in crate::commands::native_root) fn registration(
        dir: &Path,
        executable: &Path,
    ) -> Registration {
        let requester = dir.join("agent-bash");
        if !requester.exists() {
            std::fs::write(&requester, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&requester, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        serde_json::from_value(json!({
            "executable": executable,
            "settings": {
                "settings_id": "fake-settings",
                "mode": "arg",
                "model": { "name": "fake-model", "provider_args": [],
                           "inputs": { "prompt": null, "named": {} } },
                "launch": {},
            },
            "env": { "FAKE_PROVIDER_ENV": "1" },
            "agent_bash_bin": requester,
            "bash_authority": "trusted-task",
        }))
        .unwrap()
    }

    fn scratch() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        dir
    }

    #[test]
    fn custody_walks_every_ancestor_and_refuses_symlinks_and_writable_entries() {
        let dir = scratch();
        let fake = fake_provider(dir.path(), "");
        let assessed = assess(&fake).unwrap();
        assert_eq!(assessed.len(), 64);
        assert!(
            assess(Path::new("fake-provider"))
                .unwrap_err()
                .contains("absolute")
        );
        let dotted = dir.path().join("..").join("fake-provider");
        assert!(
            assess(&dotted).unwrap_err().contains("normalized"),
            "{dotted:?}"
        );
        // A symlink to a trusted file, as the leaf or as an ancestor.
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&fake, &link).unwrap();
        assert!(assess(&link).unwrap_err().contains("is a symlink"));
        let linked_dir = dir.path().join("linked-dir");
        std::os::unix::fs::symlink(dir.path(), &linked_dir).unwrap();
        let through = assess(&linked_dir.join("fake-provider")).unwrap_err();
        assert!(through.contains("is a symlink"), "{through}");
        // A group-writable ancestor directory, then the file itself.
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::copy(&fake, sub.join("p")).unwrap();
        std::fs::set_permissions(sub.join("p"), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(assess(&sub.join("p")).is_ok());
        std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o775)).unwrap();
        let writable = assess(&sub.join("p")).unwrap_err();
        assert!(
            writable.contains("sub is writable by group or others"),
            "{writable}"
        );
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o775)).unwrap();
        assert!(
            assess(&fake)
                .unwrap_err()
                .contains("writable by group or others")
        );
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            assess(&fake)
                .unwrap_err()
                .contains("not an executable file")
        );
    }

    /// The assessed identity is the one the provider client pins and runs:
    /// the binding holds for an unchanged file and refuses a replacement
    /// made between the custody check and the pin.
    #[test]
    fn pinned_object_must_be_the_assessed_object() {
        let dir = scratch();
        let fake = fake_provider(dir.path(), "");
        let registered = registration(dir.path(), &fake);
        let admitted = admit(&registered, None).unwrap();
        assert_eq!(admitted.identity, assess(&fake).unwrap());
        assert!(calls(dir.path()).is_empty(), "admission runs nothing");
        // A same-bytes replacement after the custody check is a different
        // object: it is refused, and nothing runs.
        let assessed = assess(&fake).unwrap();
        let replacement = dir.path().join("replacement");
        std::fs::copy(&fake, &replacement).unwrap();
        std::fs::rename(&replacement, &fake).unwrap();
        let refused = pin(&fake, &assessed).unwrap_err();
        assert!(
            refused.contains("changed between its custody check and its pin"),
            "{refused}"
        );
        assert!(calls(dir.path()).is_empty());
        // Assessed afresh, the replacement is admitted.
        pin(&fake, &assess(&fake).unwrap()).unwrap();
    }

    #[test]
    fn registration_checks_its_policy_and_names_before_anything_runs() {
        let dir = scratch();
        let fake = fake_provider(dir.path(), "");
        let good = registration(dir.path(), &fake);
        check(&good).unwrap();
        let refuse = |edit: &dyn Fn(&mut Value), fragment: &str| {
            let mut value = json!({
                "executable": fake,
                "settings": {},
                "agent_bash_bin": good.agent_bash_bin,
                "bash_authority": "trusted-task",
            });
            edit(&mut value);
            let registration: Registration = serde_json::from_value(value).unwrap();
            let reason = check(&registration).unwrap_err();
            assert!(reason.contains(fragment), "{reason}");
        };
        refuse(
            &|v| {
                v.as_object_mut().unwrap().remove("bash_authority");
            },
            "name bash_allow",
        );
        refuse(&|v| v["bash_allow"] = json!(["ls"]), "exclusive");
        refuse(
            &|v| v["agent_bash_bin"] = json!("agent-bash"),
            "agent_bash_bin",
        );
        refuse(
            &|v| v["env"] = json!({ "OULIPOLY_HOST_RESIDENT_SESSION_V1": "1" }),
            "this entry's",
        );
        refuse(
            &|v| v["env"] = json!({ tool_mediation::ENV: "{}" }),
            "this entry's",
        );
        for name in [
            oulipoly_root_supervisor::bash::BASH_ENV,
            oulipoly_root_supervisor::SOCKET_ENV,
        ] {
            refuse(
                &|v| v["env"] = json!({ name: "/wrong-endpoint" }),
                "owner's to set",
            );
        }
        refuse(&|v| v["config_root"] = json!("relative"), "config_root");
    }

    /// Describe offers the resident session, selects the provider's
    /// declared versions through the contract crate, and a provider that
    /// does not advertise it is a provider refusal (it ran), never an
    /// embedded substitute.
    #[test]
    fn describe_selects_the_declared_resident_session_or_refuses_after_running() {
        let dir = scratch();
        let fake = fake_provider(dir.path(), "");
        let registered = registration(dir.path(), &fake);
        let admitted = admit(&registered, None).unwrap();
        let declared = admitted.describe().unwrap();
        assert_eq!(declared.provider_id, "fake-external");
        assert_eq!(declared.contract, CONTRACT_VERSION);
        assert_eq!(declared.resident_session, 1);
        let seen = calls(dir.path());
        assert_eq!(seen[0]["op"], "describe");
        let host = &seen[0]["request"]["host"];
        assert_eq!(host["env"]["OULIPOLY_HOST_RESIDENT_SESSION_V1"], "1");
        assert_eq!(host["env"]["FAKE_PROVIDER_ENV"], "1");

        let dir = scratch();
        let fake = fake_provider(dir.path(), "caps.pop('resident_session_v1', None)");
        let registered = registration(dir.path(), &fake);
        let failure = admit(&registered, None).unwrap().describe().unwrap_err();
        let Failure::Provider { operation, reason } = failure else {
            panic!("{failure:?}")
        };
        assert_eq!(operation, "describe");
        assert!(reason.contains("declares no resident session"), "{reason}");
        assert!(
            reason.contains("not substituted by an embedded harness"),
            "{reason}"
        );
        assert_eq!(calls(dir.path()).len(), 1);
    }

    /// The provider's describe line, edited before it is answered.
    fn described_with(edit: &str) -> Result<Declared, Failure> {
        let dir = scratch();
        let script = format!(
            "if op == 'describe':\n    {}\n    sys.exit(0)",
            edit.replace('\n', "\n    ")
        );
        let fake = fake_provider(dir.path(), &script);
        let registered = registration(dir.path(), &fake);
        let declared = admit(&registered, None).unwrap().describe();
        assert_eq!(calls(dir.path()).len(), 1, "describe only");
        declared
    }

    const DESCRIBED: &str = "result = {'provider_id': 'fake-external', 'display_name': 'Fake', \
        'contract_versions': VERSIONS, 'preferred_contract': PREFERRED, 'capabilities': caps}\n\
        answer(result)";

    /// Newer advertisements than this entry knows (a v2 contract, preferred;
    /// a resident session v2; an unknown structured capability) pass the
    /// contract crate's admission on the wire and select what both sides
    /// support. The selected version and the known fields stay strict.
    #[test]
    fn describe_admits_future_advertisements_and_selects_the_common_versions() {
        let future = described_with(&format!(
            "caps['resident_session_v2'] = True\ncaps['future_capability'] = {{'shape': [1, 2]}}\n\
             VERSIONS = ['oulipoly.provider/v2', 'oulipoly.provider/v1']\nPREFERRED = 'oulipoly.provider/v2'\n{DESCRIBED}"
        ))
        .unwrap();
        assert_eq!(future.contract, CONTRACT_VERSION);
        assert_eq!(future.preferred_contract, "oulipoly.provider/v2");
        assert_eq!(future.resident_session, 1);

        let refused = |edit: &str, fragment: &str| {
            let failure = described_with(&format!("{edit}\n{DESCRIBED}")).unwrap_err();
            let Failure::Provider { operation, reason } = &failure else {
                panic!("{failure:?}")
            };
            assert_eq!(*operation, "describe");
            assert!(reason.contains(fragment), "{fragment}: {reason}");
        };
        // A preference the provider did not declare.
        refused(
            "VERSIONS = ['oulipoly.provider/v1']\nPREFERRED = 'oulipoly.provider/v2'",
            "preferred_contract must belong to contract_versions",
        );
        // A known capability of the wrong type.
        refused(
            "caps['launch'] = 'yes'\nVERSIONS = ['oulipoly.provider/v1']\nPREFERRED = 'oulipoly.provider/v1'",
            "describe",
        );
        // Only versions this entry does not speak.
        refused(
            "VERSIONS = ['oulipoly.provider/v2']\nPREFERRED = 'oulipoly.provider/v2'",
            "no contract version this entry speaks",
        );
        refused(
            "caps.pop('resident_session_v1')\ncaps['resident_session_v2'] = True\n\
             VERSIONS = ['oulipoly.provider/v1']\nPREFERRED = 'oulipoly.provider/v1'",
            "declares no resident session",
        );
    }

    /// Policy and prepare: the template is the policy's argv and env with
    /// Runner's tool policy, and the harness is the registered executable
    /// with the prepared arguments.
    #[test]
    fn prepare_maps_the_policy_template_to_the_registered_executable() {
        let dir = scratch();
        let fake = fake_provider(dir.path(), "");
        let registered = registration(dir.path(), &fake);
        let admitted = admit(&registered, None).unwrap();
        admitted.describe().unwrap();
        let tools = serde_json::to_value(registered.mediation()).unwrap();
        let template = admitted.template().unwrap();
        assert_eq!(
            template.launch.argv,
            ["fake-native", "--model", "fake-model"]
        );
        let env = template.launch.env.clone().unwrap();
        assert_eq!(env["FAKE_POLICY"], "1");
        assert_eq!(
            serde_json::from_str::<Value>(&env[tool_mediation::ENV]).unwrap(),
            tools
        );
        let launch = dir.path().join("launch");
        let prepared = admitted.prepare(&launch, template).unwrap();
        assert_eq!(prepared.argv[0], fake.to_string_lossy());
        assert_eq!(prepared.argv[1..3], ["resident.serve", "--config"]);
        let config = Path::new(&prepared.argv[3]);
        assert!(config.starts_with(launch.join("provider")));
        assert_eq!(prepared.result.acp.protocol_version, 2);
        let seen = calls(dir.path());
        let ops: Vec<&str> = seen.iter().map(|c| c["op"].as_str().unwrap()).collect();
        assert_eq!(ops, ["describe", "policy.evaluate", "resident.prepare"]);
        assert_eq!(
            seen[2]["request"]["host"]["data_root"],
            launch.join("provider").to_string_lossy().as_ref()
        );
        assert_eq!(
            seen[2]["request"]["params"]["launch"]["env"][tool_mediation::ENV],
            registered.mediation().encode()
        );
        // A second prepare cannot reuse the launch directory.
        let again = admitted.template().unwrap();
        assert!(matches!(
            admitted.prepare(&launch, again),
            Err(Failure::Setup { provider: true, .. })
        ));
    }

    /// Synthetic policy/prepare fixture only: bypass ancestor custody, which
    /// is tested separately. The mandated planning path has a writable
    /// ancestor; these tests certify no production artifact admission.
    fn synthetic_admitted(registration: &Registration) -> Admitted<'_> {
        let path = Path::new(&registration.executable);
        let assessed = identity(&File::open(path).unwrap()).unwrap();
        Admitted {
            registration,
            client: pin(path, &assessed).unwrap(),
            identity: assessed,
            offer: None,
        }
    }

    #[test]
    fn shared_template_keeps_host_mediation_and_endpoint_guards() {
        for edit in [
            "markers = []",
            "markers.append(markers[0])",
            "marker['bash'] = {'allow': ['other-command']}",
            "mediation['requester'] = '/other-requester'; mediation_env = json.dumps(mediation)",
            "marker['native_tools'] = ['unrelated-tool']",
            "answer({'accepted':True,'argv':['fake-native'],'env':{'OULIPOLY_TOOL_MEDIATION_V1':mediation_env,'OULIPOLY_ROOT_BASH_V1':'PRIVATE-ENDPOINT'},'stdin':None,'prompt':None,'diagnostics':[],'markers':markers}); sys.exit(0)",
        ] {
            let dir = scratch();
            let fake = fake_provider(
                dir.path(),
                &format!("if op == 'policy.evaluate':\n    {edit}"),
            );
            let registered = registration(dir.path(), &fake);
            let admitted = synthetic_admitted(&registered);
            admitted.describe().unwrap();
            assert!(
                matches!(
                    admitted.template(),
                    Err(Failure::Provider {
                        operation: "policy.evaluate",
                        ..
                    })
                ),
                "{edit}"
            );
            assert_eq!(calls(dir.path()).len(), 2, "no prepare after host refusal");
        }
    }

    /// An echoed input can be removed from the resident template; a changed
    /// input cannot. Neither case bypasses Runner's mediation admission.
    #[test]
    fn shared_template_accepts_echo_and_refuses_transform_without_private_text() {
        for (answer_prompt, accepted) in [
            ("request['params']['model']['inputs']['prompt']", true),
            ("'PRIVATE-TRANSFORMED-PROMPT'", false),
        ] {
            let dir = scratch();
            let fake = fake_provider(
                dir.path(),
                &format!(
                    "if op == 'policy.evaluate':\n    answer({{'accepted': True, 'argv': ['fake-native'], 'env': {{'OULIPOLY_TOOL_MEDIATION_V1': mediation_env}}, 'stdin': None, 'prompt': {answer_prompt}, 'diagnostics': [], 'markers': markers}})\n    sys.exit(0)"
                ),
            );
            let mut registered = registration(dir.path(), &fake);
            registered.settings["model"]["inputs"]["prompt"] = json!("PRIVATE-ORIGINAL-PROMPT");
            let admitted = synthetic_admitted(&registered);
            admitted.describe().unwrap();
            let template = admitted.template();
            assert_eq!(template.is_ok(), accepted);
            if let Err(failure) = template {
                let text = format!("{failure:?}");
                assert!(text.contains("cannot honour prompt transform"));
                assert!(!text.contains("PRIVATE-"));
            }
            assert_eq!(calls(dir.path()).len(), 2, "no preparation after refusal");
        }
    }

    /// Adapter refusal text may contain the person's prompt. The operation
    /// is still attributed as having run, while public output withholds it.
    #[test]
    fn prepare_rpc_refusal_withholds_private_payload() {
        let dir = scratch();
        let fake = fake_provider(
            dir.path(),
            "if op == 'resident.prepare':\n    print(json.dumps({'contract':'oulipoly.provider/v1','request_id':request['request_id'],'ok':False,'error':{'code':'invalid_resident_argv','category':'invalid_request','message':'PRIVATE-PROMPT-PAYLOAD','retryable':False}}))\n    sys.exit(0)",
        );
        let registered = registration(dir.path(), &fake);
        let admitted = synthetic_admitted(&registered);
        admitted.describe().unwrap();
        let template = admitted.template().unwrap();
        let failure = admitted
            .prepare(&dir.path().join("launch"), template)
            .unwrap_err();
        assert!(matches!(
            &failure,
            Failure::Setup { provider: true, reason }
                if reason.contains("resident.prepare failed: provider_capability")
        ));
        assert!(format!("{failure:?}").contains("endpoint code: invalid_resident_argv"));
        assert!(!format!("{failure:?}").contains("PRIVATE-PROMPT-PAYLOAD"));
        assert_eq!(calls(dir.path()).len(), 3);
    }

    #[test]
    fn structural_refusal_codes_are_attributed_but_arbitrary_codes_stay_private() {
        for code in [
            "missing_prompt",
            "invalid_resident_argv",
            "PRIVATE-CODE-PAYLOAD",
        ] {
            let dir = scratch();
            let fake = fake_provider(
                dir.path(),
                &format!(
                    "if op == 'policy.evaluate':\n    answer({{'accepted':False,'stdin':None,'prompt':None,'diagnostics':[{{'code':{code:?},'message':'PRIVATE-PROMPT-PAYLOAD','severity':'error'}}],'markers':[]}})\n    sys.exit(0)"
                ),
            );
            let registered = registration(dir.path(), &fake);
            let admitted = admit(&registered, None).unwrap();
            admitted.describe().unwrap();
            let failure = admitted.template().unwrap_err();
            let text = format!("{failure:?}");
            assert_eq!(
                text.contains(code),
                code != "PRIVATE-CODE-PAYLOAD",
                "{text}"
            );
            assert!(!text.contains("PRIVATE-PROMPT-PAYLOAD"));
            assert_eq!(calls(dir.path()).len(), 2);
        }
    }

    #[test]
    fn refused_policy_and_bad_prepare_results_say_the_provider_ran() {
        let dir = scratch();
        let fake = fake_provider(
            dir.path(),
            "if op == 'policy.evaluate':\n    answer({'accepted': False, 'stdin': None, 'prompt': None, \
             'diagnostics': [{'code': 'x', 'message': 'no such route', 'severity': 'error'}], 'markers': []})\n    sys.exit(0)",
        );
        let registered = registration(dir.path(), &fake);
        let admitted = admit(&registered, None).unwrap();
        admitted.describe().unwrap();
        let failure = admitted.template().unwrap_err();
        assert!(
            matches!(&failure, Failure::Provider { operation: "policy.evaluate", reason } if reason.contains("policy refused")),
            "{failure:?}"
        );

        let dir = scratch();
        let fake = fake_provider(
            dir.path(),
            "if op == 'resident.prepare':\n    answer({'protocol': 'oulipoly.resident_session/v1'})\n    sys.exit(0)",
        );
        let registered = registration(dir.path(), &fake);
        let admitted = admit(&registered, None).unwrap();
        admitted.describe().unwrap();
        let template = admitted.template().unwrap();
        let failure = admitted
            .prepare(&dir.path().join("launch"), template)
            .unwrap_err();
        assert!(
            matches!(&failure, Failure::Setup { provider: true, reason } if reason.contains("resident.prepare result")),
            "{failure:?}"
        );
    }

    #[test]
    fn mediation_negotiation_refuses_absent_false_wrong_type_or_future_only() {
        for edit in [
            "caps.pop('tool_mediation_v1')",
            "caps['tool_mediation_v1'] = False",
            "caps['tool_mediation_v1'] = 'true'",
            "caps.pop('tool_mediation_v1'); caps['tool_mediation_v2'] = True",
        ] {
            let dir = scratch();
            let fake = fake_provider(dir.path(), edit);
            let registered = registration(dir.path(), &fake);
            let failure = admit(&registered, None).unwrap().describe().unwrap_err();
            assert!(
                matches!(
                    failure,
                    Failure::Provider {
                        operation: "describe",
                        ..
                    }
                ),
                "{edit}: {failure:?}"
            );
            assert_eq!(
                calls(dir.path()).len(),
                1,
                "no policy or prepare after refusal"
            );
            assert!(!dir.path().join("launch").exists());
        }
        let dir = scratch();
        let fake = fake_provider(dir.path(), "caps['tool_mediation_v2'] = True");
        let registered = registration(dir.path(), &fake);
        let declared = admit(&registered, None).unwrap().describe().unwrap();
        assert_eq!(declared.tool_mediation, 1);
        assert_eq!(
            calls(dir.path())[0]["request"]["host"]["env"]["OULIPOLY_HOST_TOOL_MEDIATION_V1"],
            "1"
        );
    }

    #[test]
    fn accepted_policy_without_matching_mediation_is_refused_before_setup() {
        for edit in [
            "markers = []",
            "markers.append(markers[0])",
            "marker['protocol'] = 'oulipoly.tool_mediation/v2'",
            "marker['bash'] = {'allow': ['other-command']}",
            "marker['ingress_env'] = 'OTHER_INGRESS'",
            "marker['native_tools'] = []",
            "marker['native_tools'] = ['unrelated-tool']",
            "marker['tool'] = ''",
            "marker['extra'] = True",
            "mediation_env = None",
            "mediation['requester'] = '/other-requester'; mediation_env = json.dumps(mediation)",
            "mediation['bash'] = {'allow': ['other-command']}; mediation_env = json.dumps(mediation)",
        ] {
            let dir = scratch();
            let fake = fake_provider(
                dir.path(),
                &format!("if op == 'policy.evaluate':\n    {edit}"),
            );
            let registered = registration(dir.path(), &fake);
            let admitted = admit(&registered, None).unwrap();
            admitted.describe().unwrap();
            let failure = admitted.template().unwrap_err();
            assert!(
                matches!(
                    failure,
                    Failure::Provider {
                        operation: "policy.evaluate",
                        ..
                    }
                ),
                "{edit}: {failure:?}"
            );
            assert_eq!(calls(dir.path()).len(), 2, "{edit}: no prepare");
            assert!(!dir.path().join("launch").exists());
        }
    }

    #[test]
    fn requester_policy_precedes_evaluation_and_marker_preserves_provider_inventory() {
        for bash in [
            BashPolicy::Allow {
                allow: vec!["printf allowed".to_owned()],
            },
            BashPolicy::Authority {
                authority: tool_mediation::TRUSTED_TASK.to_owned(),
            },
        ] {
            let dir = scratch();
            let fake = fake_provider(
                dir.path(),
                "marker['native_tools'].append('provider-file-tool')",
            );
            let mut registered = registration(dir.path(), &fake);
            match &bash {
                BashPolicy::Allow { allow } => {
                    registered.bash_authority = None;
                    registered.bash_allow = allow.clone();
                }
                BashPolicy::Authority { .. } => {}
            }
            let original = registered.settings.clone();
            let admitted = admit(&registered, None).unwrap();
            admitted.describe().unwrap();
            let evaluated = admitted.template().unwrap();
            assert_eq!(evaluated.effective.bash, bash);
            assert_eq!(
                evaluated.effective.native_tools,
                ["fake_mediated_bash", "provider-file-tool"]
            );
            let prepared = admitted
                .prepare(&dir.path().join("launch"), evaluated)
                .unwrap();
            assert_eq!(prepared.effective_mediation.bash, bash);
            assert_eq!(
                registered.settings, original,
                "opaque settings are not mutated"
            );
            let seen = calls(dir.path());
            for call in &seen {
                assert_eq!(
                    call["request"]["host"]["env"]["OULIPOLY_HOST_TOOL_MEDIATION_V1"],
                    "1"
                );
            }
            let input = &seen[1]["request"]["params"];
            let supplied = ToolMediation::decode(
                input["launch"]["env"][tool_mediation::ENV]
                    .as_str()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(supplied, registered.mediation());
            assert_eq!(
                seen[2]["request"]["params"]["launch"]["env"][tool_mediation::ENV],
                input["launch"]["env"][tool_mediation::ENV]
            );
            assert!(supplied.ingress_env == oulipoly_root_supervisor::bash::BASH_ENV);
        }
    }

    #[test]
    fn evaluated_environment_cannot_replace_owner_endpoint_values() {
        for name in [
            oulipoly_root_supervisor::bash::BASH_ENV,
            oulipoly_root_supervisor::SOCKET_ENV,
        ] {
            let dir = scratch();
            let fake = fake_provider(
                dir.path(),
                &format!(
                    "if op == 'policy.evaluate':\n    answer({{'accepted': True, 'argv': ['native'], 'env': {{'OULIPOLY_TOOL_MEDIATION_V1': mediation_env, {name:?}: '/wrong-endpoint'}}, 'stdin': None, 'prompt': None, 'diagnostics': [], 'markers': markers}})\n    sys.exit(0)"
                ),
            );
            let registered = registration(dir.path(), &fake);
            let admitted = admit(&registered, None).unwrap();
            admitted.describe().unwrap();
            let failure = admitted.template().unwrap_err();
            assert!(
                matches!(failure, Failure::Provider { operation: "policy.evaluate", ref reason } if reason.contains("owner endpoint")),
                "{failure:?}"
            );
            assert_eq!(calls(dir.path()).len(), 2);
            assert!(!dir.path().join("launch").exists());
        }
    }

    /// A parent registration naming a child requester, and its offer for
    /// `routes`.
    fn offering(dir: &Path, fake: &Path, routes: &[&str]) -> (Registration, Exploration) {
        let mut registered = registration(dir, fake);
        let requester = dir.join("root-child");
        std::fs::write(&requester, "#!/bin/sh\nexit 69\n").unwrap();
        std::fs::set_permissions(&requester, std::fs::Permissions::from_mode(0o755)).unwrap();
        registered.root_child_bin = Some(requester.to_string_lossy().into_owned());
        let limits = Limits {
            max_starts: Some(3),
            max_concurrent: Some(1),
        };
        let routes = routes.iter().map(|route| (*route).to_owned()).collect();
        let offer = offer(&registered, routes, limits).unwrap().unwrap();
        (registered, offer)
    }

    /// The offer is the configured routes, the named requester and the
    /// owner's ingress; a requester without routes, routes without a
    /// requester and a route name the contract cannot carry are refused
    /// before anything runs.
    #[test]
    fn offer_is_the_configured_routes_with_the_requester_only_they_need() {
        let dir = scratch();
        let fake = fake_provider(dir.path(), "");
        let (registered, made) = offering(dir.path(), &fake, &["luna-max", "fixed.1"]);
        assert_eq!(made.routes, ["luna-max", "fixed.1"]);
        assert_eq!(Some(&made.requester), registered.root_child_bin.as_ref());
        assert_eq!(made.ingress_env, oulipoly_root_supervisor::bash::BASH_ENV);
        assert_eq!(
            made.limits,
            Some(Limits {
                max_starts: Some(3),
                max_concurrent: Some(1)
            })
        );
        check(&registered).unwrap();
        let limits = || Limits {
            max_starts: Some(1),
            max_concurrent: Some(1),
        };
        let unneeded = offer(&registered, Vec::new(), limits()).unwrap_err();
        assert!(unneeded.contains("declares no child routes"), "{unneeded}");
        let plain = registration(dir.path(), &fake);
        assert_eq!(offer(&plain, Vec::new(), limits()), Ok(None));
        let missing = offer(&plain, vec!["luna".to_owned()], limits()).unwrap_err();
        assert!(missing.contains("name root_child_bin"), "{missing}");
        let label = offer(&registered, vec!["-flag".to_owned()], limits()).unwrap_err();
        assert!(label.contains("cannot be offered"), "{label}");
        let mut relative = registered;
        relative.root_child_bin = Some("oulipoly-root-child".to_owned());
        assert!(check(&relative).unwrap_err().contains("root_child_bin"));
        assert!(calls(dir.path()).is_empty(), "nothing ran");
    }

    /// A parent offered routes selects exploration in every operation,
    /// offers it beside the mediation policy, and keeps the provider's
    /// reported native tool, listed in its own inventory, through prepare.
    #[test]
    fn offering_parent_selects_exploration_and_keeps_the_offer_through_prepare() {
        let dir = scratch();
        let fake = fake_provider(dir.path(), "");
        let (registered, made) = offering(dir.path(), &fake, &["luna"]);
        let admitted = admit(&registered, Some(made.clone())).unwrap();
        let declared = admitted.describe().unwrap();
        assert_eq!(declared.exploration, Some(1));
        assert_eq!(declared.entry()["exploration"], 1);
        let evaluated = admitted.template().unwrap();
        let effective = evaluated.exploration.clone().unwrap();
        assert_eq!(effective.routes, made.routes);
        assert_eq!(effective.ingress_env, made.ingress_env);
        assert!(evaluated.effective.native_tools.contains(&effective.tool));
        let launch = dir.path().join("launch");
        let prepared = admitted.prepare(&launch, evaluated).unwrap();
        assert_eq!(prepared.effective_exploration, Some(effective));
        let seen = calls(dir.path());
        for call in &seen {
            assert_eq!(
                call["request"]["host"]["env"]["OULIPOLY_HOST_EXPLORATION_V1"], "1",
                "{}",
                call["op"]
            );
        }
        for call in &seen[1..] {
            let carried = call["request"]["params"]["launch"]["env"][exploration::ENV]
                .as_str()
                .unwrap();
            assert_eq!(Exploration::decode(carried).unwrap(), made);
        }
        // The prepared configuration carries the same offer.
        let config: Value =
            serde_json::from_slice(&std::fs::read(&prepared.argv[3]).unwrap()).unwrap();
        assert_eq!(
            Exploration::decode(config["env"][exploration::ENV].as_str().unwrap()).unwrap(),
            made
        );
    }

    /// Child routes need common exploration support: a provider that does
    /// not declare v1 is refused after describe alone, nothing prepared,
    /// rather than given Bash alone. An additional unknown version beside
    /// v1 is tolerated.
    #[test]
    fn offering_parent_without_declared_exploration_is_refused_after_describe() {
        for edit in [
            "caps.pop('exploration_v1', None)",
            "caps['exploration_v1'] = False",
            "caps['exploration_v1'] = '1'",
            "caps.pop('exploration_v1', None); caps['exploration_v2'] = True",
        ] {
            let dir = scratch();
            let fake = fake_provider(dir.path(), edit);
            let (registered, made) = offering(dir.path(), &fake, &["luna"]);
            let failure = admit(&registered, Some(made))
                .unwrap()
                .describe()
                .unwrap_err();
            let Failure::Provider { operation, reason } = &failure else {
                panic!("{edit}: {failure:?}")
            };
            assert_eq!(*operation, "describe");
            assert!(reason.contains("declares no exploration"), "{reason}");
            assert!(reason.contains("not given Bash alone"), "{reason}");
            assert_eq!(calls(dir.path()).len(), 1, "{edit}: describe only");
            assert!(!dir.path().join("launch").exists());
        }
        let dir = scratch();
        let fake = fake_provider(
            dir.path(),
            "caps['exploration_v2'] = True\ncaps['exploration_future'] = {'shape': 1}",
        );
        let (registered, made) = offering(dir.path(), &fake, &["luna"]);
        let declared = admit(&registered, Some(made)).unwrap().describe().unwrap();
        assert_eq!(declared.exploration, Some(1));
    }

    /// Without an offer (a child, or a parent with no routes) nothing is
    /// selected or offered, and a provider that puts an offer in the
    /// evaluated env or reports exploration anyway is refused before
    /// preparation.
    #[test]
    fn no_offer_selects_nothing_and_refuses_an_offer_it_did_not_make() {
        let dir = scratch();
        let fake = fake_provider(dir.path(), "");
        let registered = registration(dir.path(), &fake);
        let admitted = admit(&registered, None).unwrap();
        assert_eq!(admitted.describe().unwrap().exploration, None);
        let evaluated = admitted.template().unwrap();
        assert_eq!(evaluated.exploration, None);
        assert!(
            !evaluated
                .launch
                .env
                .as_ref()
                .unwrap()
                .contains_key(exploration::ENV)
        );
        let prepared = admitted
            .prepare(&dir.path().join("launch"), evaluated)
            .unwrap();
        assert_eq!(prepared.effective_exploration, None);
        for call in calls(dir.path()) {
            let text = call["request"].to_string();
            assert!(!text.contains("EXPLORATION"), "{text}");
        }
        let forged = r#"{"protocol": "oulipoly.exploration/v1", "routes": ["luna"], "requester": "/r", "ingress_env": "OULIPOLY_ROOT_BASH_V1"}"#;
        for (edit, fragment) in [
            (
                // A provider injecting an offer of its own into an accepted policy.
                format!("if op == 'policy.evaluate':\n    exploration_env = {forged:?}\n    explore_selected = True"),
                "did not make",
            ),
            (
                "if op == 'policy.evaluate':\n    markers.append({'name': 'oulipoly.exploration/v1', 'value': {'protocol': 'oulipoly.exploration/v1', 'routes': ['luna'], 'tool': 'fake_mediated_bash', 'ingress_env': 'OULIPOLY_ROOT_BASH_V1'}})".to_owned(),
                "did not offer",
            ),
        ] {
            let dir = scratch();
            let fake = fake_provider(dir.path(), &edit);
            let registered = registration(dir.path(), &fake);
            let admitted = admit(&registered, None).unwrap();
            admitted.describe().unwrap();
            let failure = admitted.template().unwrap_err();
            assert!(
                matches!(&failure, Failure::Provider { operation: "policy.evaluate", reason } if reason.contains(fragment)),
                "{edit}: {failure:?}"
            );
            assert_eq!(calls(dir.path()).len(), 2, "{edit}: no prepare");
        }
    }

    /// The host's offers are this entry's: a parent's or a child's opaque
    /// settings or operation env naming them is refused before anything
    /// runs, not overwritten.
    #[test]
    fn caller_supplied_offers_are_refused_before_anything_runs() {
        let dir = scratch();
        let fake = fake_provider(dir.path(), "");
        let child: ChildRegistration = serde_json::from_value(json!({
            "executable": fake,
            "settings": { "launch": {} },
            "agent_bash_bin": registration(dir.path(), &fake).agent_bash_bin,
        }))
        .unwrap();
        let parent = |dir: &Path| offering(dir, &fake, &["luna"]).0;
        let child = |_: &Path| child.with_policy(Vec::new(), Some(BashAuthority::TrustedTask));
        let builders: [&dyn Fn(&Path) -> Registration; 2] = [&parent, &child];
        for build in builders {
            for name in [exploration::ENV, tool_mediation::ENV] {
                let mut forged = build(dir.path());
                forged.settings["launch"] = json!({ "env": { name: "{}" } });
                let reason = check(&forged).unwrap_err();
                assert!(reason.contains("this entry's to set"), "{reason}");
                let mut forged = build(dir.path());
                forged.env.insert(name.to_owned(), "{}".to_owned());
                let reason = check(&forged).unwrap_err();
                assert!(reason.contains("this entry's to set"), "{reason}");
            }
        }
        // A child registration cannot name a requester of its own.
        let named: Result<ChildRegistration, _> = serde_json::from_value(json!({
            "executable": fake, "settings": {}, "agent_bash_bin": "/b",
            "root_child_bin": "/r",
        }));
        assert!(named.unwrap_err().to_string().contains("unknown field"));
        assert!(calls(dir.path()).is_empty(), "nothing ran");
    }

    /// The evaluated offer and the provider's exploration marker must agree
    /// with the offer and with the provider's own mediation inventory;
    /// anything else is refused before preparation.
    #[test]
    fn effective_exploration_must_agree_with_the_offer_and_provider_inventory() {
        for edit in [
            "markers.pop()",
            "markers.append(markers[-1])",
            "explore_marker['routes'] = ['other']",
            "explore_marker['routes'] = ['luna']",
            "explore_marker['ingress_env'] = 'OTHER_INGRESS'",
            "explore_marker['tool'] = 'unlisted_explore'",
            "explore_marker['protocol'] = 'oulipoly.exploration/v2'",
            "explore_marker['extra'] = True",
            "exploration_env = None",
            "exploration['requester'] = '/other'; exploration_env = json.dumps(exploration)",
            "exploration['routes'] = ['luna']; exploration_env = json.dumps(exploration)",
            "exploration_env = '{\"protocol\": \"oulipoly.exploration/v1\"}'",
        ] {
            let dir = scratch();
            let fake = fake_provider(
                dir.path(),
                &format!("if op == 'policy.evaluate':\n    {edit}"),
            );
            let (registered, made) = offering(dir.path(), &fake, &["luna", "sol"]);
            let admitted = admit(&registered, Some(made)).unwrap();
            admitted.describe().unwrap();
            let failure = admitted.template().unwrap_err();
            assert!(
                matches!(
                    failure,
                    Failure::Provider {
                        operation: "policy.evaluate",
                        ..
                    }
                ),
                "{edit}: {failure:?}"
            );
            assert_eq!(calls(dir.path()).len(), 2, "{edit}: no prepare");
            assert!(!dir.path().join("launch").exists());
        }
        // The provider's own order of the same routes is agreement.
        let dir = scratch();
        let fake = fake_provider(
            dir.path(),
            "if op == 'policy.evaluate':\n    explore_marker['routes'] = ['sol', 'luna']",
        );
        let (registered, made) = offering(dir.path(), &fake, &["luna", "sol"]);
        let admitted = admit(&registered, Some(made)).unwrap();
        admitted.describe().unwrap();
        assert!(admitted.template().unwrap().exploration.is_some());
    }

    /// Published adapters, administrative operations only: no native CLI starts.
    #[test]
    #[ignore = "needs published OULIPOLY_REAL_{CLAUDE,CODEX}_ADAPTER and OULIPOLY_REAL_CODEX_MODELS"]
    fn real_published_adapters_evaluate_and_prepare_selected_requester_policy() {
        for (variable, id) in [
            ("OULIPOLY_REAL_CLAUDE_ADAPTER", "claude"),
            ("OULIPOLY_REAL_CODEX_ADAPTER", "codex"),
        ] {
            let adapter = PathBuf::from(std::env::var(variable).expect(variable));
            for allow in [false, true] {
                let dir = scratch();
                let native = dir.path().join("never-native");
                std::fs::write(&native, "#!/bin/sh\nexit 99\n").unwrap();
                std::fs::set_permissions(&native, std::fs::Permissions::from_mode(0o755)).unwrap();
                let mut registered = registration(dir.path(), &adapter);
                if allow {
                    registered.bash_authority = None;
                    registered.bash_allow = vec!["printf allowed".to_owned()];
                }
                registered.env = BTreeMap::from([(
                    "HOME".to_owned(),
                    dir.path().to_string_lossy().into_owned(),
                )]);
                let settings = if id == "claude" {
                    json!({"settings_id": "claude-witness", "mode": "headless",
                        "model": {"name": "claude-opus", "provider_args": ["--model", "opus"], "inputs": {"prompt": null, "named": {}}},
                        "launch": {"command": native, "prompt_mode": "stdin", "env": {"SENTINEL": "opaque-env"}}})
                } else {
                    let config = dir.path().join("config/agent-runner-codex");
                    std::fs::create_dir_all(&config).unwrap();
                    let mcp = dir.path().join("mcp.ts");
                    std::fs::write(&mcp, "// never started\n").unwrap();
                    let prompt = dir.path().join("system.md");
                    std::fs::write(&prompt, "system sentinel\n").unwrap();
                    std::fs::copy(
                        std::env::var("OULIPOLY_REAL_CODEX_MODELS").unwrap(),
                        dir.path().join("models.json"),
                    )
                    .unwrap();
                    let b = native.to_string_lossy();
                    std::fs::write(config.join("config.toml"), format!(
                        "codex_bin = {b:?}\nbun_bin = {b:?}\nbash_mcp_path = {:?}\nsystem_prompt_file = {:?}\nagent_bash_bin = {b:?}\nagent_runner_bin = {b:?}\n", mcp.to_string_lossy(), prompt.to_string_lossy())).unwrap();
                    registered.config_root =
                        Some(dir.path().join("config").to_string_lossy().into_owned());
                    json!({"settings_id": "codex2", "mode": "stdin",
                        "model": {"name": "gpt-astra-high", "provider_args": ["-m", "gpt-6-astra", "-c", "model_reasoning_effort=\"high\""], "inputs": {"prompt": "prepare sentinel", "named": {}}},
                        "launch": {"argv": ["codex2", "exec", "--dangerously-bypass-approvals-and-sandbox", "-m", "gpt-6-astra", "-c", "model_reasoning_effort=\"high\""], "env": {"SENTINEL": "opaque-env"}}})
                };
                registered.settings = settings.as_object().unwrap().clone();
                check(&registered).unwrap();
                let admitted = admit(&registered, None).unwrap();
                let declared = admitted.describe().unwrap();
                assert_eq!(declared.provider_id, id);
                assert_eq!(declared.tool_mediation, 1);
                let evaluated = admitted.template().unwrap();
                assert_eq!(evaluated.effective.bash, registered.mediation().bash);
                assert_eq!(
                    evaluated.launch.env.as_ref().unwrap()["SENTINEL"],
                    "opaque-env"
                );
                let prepared = admitted
                    .prepare(&dir.path().join("launch"), evaluated)
                    .unwrap();
                assert_eq!(prepared.result.invocation.endpoint, "stdio");
                let config_path = &prepared.argv[3];
                let config: Value =
                    serde_json::from_slice(&std::fs::read(config_path).unwrap()).unwrap();
                assert_eq!(
                    ToolMediation::decode(
                        config["launch"]["env"][tool_mediation::ENV]
                            .as_str()
                            .unwrap()
                    )
                    .unwrap(),
                    registered.mediation()
                );
                println!(
                    "published adapter={id} allow={allow} declared={} marker={} prepared={}",
                    declared.entry(),
                    serde_json::to_value(prepared.effective_mediation).unwrap(),
                    serde_json::to_value(prepared.result).unwrap()
                );
            }
        }
    }

    /// Hand-over to oneself needs no privilege, so the walk itself is
    /// checked here: directories and singly linked files only.
    #[test]
    fn hand_over_takes_directories_and_singly_linked_files_only() {
        let dir = scratch();
        let launch = dir.path().join("launch");
        let data = make_launch(&launch).unwrap();
        std::fs::create_dir(data.join("configs")).unwrap();
        std::fs::write(data.join("configs/c.json"), "{}").unwrap();
        let me = Identity {
            user: "self".to_owned(),
            // SAFETY: no preconditions.
            uid: unsafe { libc::geteuid() },
            gid: unsafe { libc::getegid() },
            groups: Vec::new(),
        };
        assert_eq!(hand_over(&launch, &data, &me).unwrap(), 3);
        let mode = std::fs::metadata(&launch).unwrap().mode() & 0o7777;
        assert_eq!(mode, 0o750);
        std::fs::hard_link(data.join("configs/c.json"), data.join("linked")).unwrap();
        let refused = hand_over(&launch, &data, &me).unwrap_err();
        assert!(refused.contains("singly linked"), "{refused}");
        std::fs::remove_file(data.join("linked")).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", data.join("link")).unwrap();
        assert!(
            hand_over(&launch, &data, &me)
                .unwrap_err()
                .contains("singly linked")
        );
    }
}
