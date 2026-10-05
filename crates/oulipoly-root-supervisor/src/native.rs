//! Owner-side setup of one native OpenCode host for a root (Linux).
//!
//! [`provision_opencode`] writes a fresh private launch directory and
//! returns the harness `argv` that starts OpenCode from it. That directory
//! holds everything the host loads: the ACP v2 endpoint plugin
//! (`native/opencode/acp-v2-endpoint.ts`), the native permission gate as
//! the `bash` tool (`native/opencode/bash-policy-tool.ts`), the matching
//! agent-bash tool it delegates to, the locked package files with the
//! caller's installed dependencies, and the root's own native permission
//! policy. The `argv` is kept in the root's durable intent, so every
//! launch and relaunch of that harness reads the same directory.
//!
//! * **Policy.** Every native tool other than `bash` is denied (and so
//!   hidden). What `bash` may run is the caller's explicit choice, one of:
//!   - `bash_allow` (the default form): only the commands the caller
//!     named, each matched as the whole command string; anything else is a
//!     native denial, with no permission request to the owner. Entries with
//!     `*`, `?` or `\` are refused, since native matching would treat them
//!     as globs.
//!   - `bash_authority: "trusted-task"`: the caller trusts this task with
//!     any `bash` command, as once-per-task authority. Every command still
//!     goes through the agent-bash tool and so the root's own Bash ingress
//!     (attributed, durably recorded, killed on cancel); nothing else is
//!     widened. Never selected implicitly, and not with `bash_allow`.
//!
//!   Native OpenCode enforces either; the owner grants nothing. The
//!   workdir and delivery are not part of a decision. The launch receipt
//!   reports the effective policy, including the native permission config
//!   exactly as written.
//! * **Isolation.** HOME and every XDG directory point into the launch
//!   directory, project config is disabled, and inherited
//!   `OPENCODE_CONFIG`, `OPENCODE_CONFIG_DIR`, `OPENCODE_CONFIG_CONTENT` and
//!   `OPENCODE_PERMISSION` are removed, so no personal or project config
//!   joins the policy. OpenCode's system managed config directory
//!   (`/etc/opencode`) is still read: an administrator's layer, trusted.
//!   Everything in the launch directory is trusted code and config.
//! * **Dependencies.** The caller's `deps` directory must hold the
//!   installed dependencies of this crate's `native/opencode/package.json`
//!   (`npm ci --ignore-scripts`) beside exactly this crate's lockfile; the
//!   launch directory links its `node_modules`, so OpenCode's own
//!   dependency install there has nothing to do and fetches nothing.
//! * Nothing here launches or models: a model and provider are the
//!   caller's (`model`, `provider`). Without `auth`, nothing authenticates:
//!   the native data directory starts empty, OpenCode's built-in plugins
//!   stay disabled and its loopback HTTP server has no password.
//! * **Authentication** (`auth`, opt-in per request, with a model). The
//!   caller names one private OpenCode `auth.json` (provider id to one
//!   `oauth`, `api` or `wellknown` entry; the model's provider must have
//!   one). Its required fields are checked before any setup writes;
//!   optional field types and the full native schema are not validated.
//!   It is read once and refused for invalid required fields,
//!   with no part of its contents in a reason, and written as the native
//!   data directory's `opencode/auth.json` (`0600`). Never into
//!   `opencode.json`, argv, an event or the root's environment. Both
//!   secrets below are reachable by the work identity, the principal the
//!   host and every in-root Bash run as: this keeps them out of group-
//!   readable config, argv and setup events, not hidden from that user.
//!   Setup adds neither value to the root's declared environment; caller-
//!   supplied credential variables can still reach mediated Bash.
//!   OpenCode may rewrite its own `auth.json` (a refresh, if the entry
//!   allows one).
//!   Such a launch also:
//!   - enables OpenCode's built-in plugins, all of them: the release has
//!     one switch, and its OpenAI (Codex subscription) auth is one of them;
//!   - gets a fresh random loopback server password, written only to
//!     `secret/server-password` (`0600`) and read from there into the host's
//!     environment as it starts (`OPENCODE_SERVER_PASSWORD`). Its native
//!     agent-bash requester inherits that password in the host lineage;
//!     owner-spawned mediated Bash has the separate root environment.
//!     With the launch password, the
//!     host's HTTP API refuses requests without it and its internal clients
//!     (the endpoint plugin's included) send it. No argv carries the value.
//! * **Explorer tool** (`explore`, only when the root allows registered
//!   children): `tool/explore.ts` asks the root's owner for one child
//!   through the shared `explore-client.mjs`, from inside the host process
//!   (so the request is attributed to this host's work); `explore.json`
//!   names its routes and limits, which the owner enforces. The native
//!   permission config allows `explore` by name and nothing else besides
//!   `bash`. A child's own launch never has this tool.
//! * **Work identity.** Without one, everything is the caller's (`0700` /
//!   `0600`), as for an unprivileged root. With a `host-root` work identity
//!   (the caller is host root), the fresh tree is handed over before it is
//!   opened up: HOME and the XDG data, cache and state directories become
//!   the identity's (`0700`), as do `secret` and the native data
//!   `opencode` directory (`0700`) and their files (`0600`) when `auth` is
//!   given; the launch, XDG and XDG config directories,
//!   every written file and the `node_modules` link stay the caller's with
//!   the identity's primary group (`0750` / `0640`), so the host reads its
//!   code, config and policy but cannot change them; the native config
//!   directory itself is `1770` (sticky), so the host may add its own
//!   files there (OpenCode writes `.gitignore`) but not replace or remove
//!   the caller's. Only paths created here are changed. Other members of
//!   that primary group get the same read access (a known limit). The
//!   caller's `deps`, agent-bash tool and binary are only read and must be
//!   readable by the identity; nothing here changes them.

use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::workload::Identity;

const ENDPOINT: &str = include_str!("../native/opencode/acp-v2-endpoint.ts");
const GATE: &str = include_str!("../native/opencode/bash-policy-tool.ts");
const EXPLORE: &str = include_str!("../native/opencode/explore-tool.ts");
/// The registered-children client shared with the native Claude receiver.
pub(crate) const EXPLORE_CLIENT: &str = include_str!("../native/explore-client.mjs");
const PACKAGE: &str = include_str!("../native/opencode/package.json");
const LOCK: &str = include_str!("../native/opencode/package-lock.json");

/// The OpenCode binary inside the installed dependencies.
const OPENCODE: &str = "node_modules/opencode-linux-x64/bin/opencode";

/// Inherited variables that would add config or credentials beside the
/// launch's own: auth comes only from the launch's data directory, and a
/// server password only from its own `secret` file.
/// Removal applies to the native host, not to the caller's declared root
/// environment or mediated Bash. A declared default-plugin override is
/// also not unset by auth opt-in.
pub const REMOVED_ENV: [&str; 7] = [
    "OPENCODE_CONFIG",
    "OPENCODE_CONFIG_DIR",
    "OPENCODE_CONFIG_CONTENT",
    "OPENCODE_PERMISSION",
    "OPENCODE_AUTH_CONTENT",
    "OPENCODE_SERVER_PASSWORD",
    "OPENCODE_SERVER_USERNAME",
];

/// Largest accepted `auth` file.
const AUTH_LIMIT: u64 = 64 * 1024;

/// Reads the server password from the file named by `$0` into the host's
/// environment, then execs the host (`"$@"`): the value is in no argv.
const WITH_PASSWORD: &str = r#"IFS= read -r OPENCODE_SERVER_PASSWORD < "$0" && export OPENCODE_SERVER_PASSWORD && exec "$@""#;

/// What the caller chooses for one native OpenCode host.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenCodeSetup {
    /// Absolute path of the launch directory to create; must not exist.
    pub dir: String,
    /// Absolute path of the installed dependencies (see module docs).
    pub deps: String,
    /// The matching agent-bash `integrations/opencode/tools/bash.ts`.
    pub agent_bash_tool: String,
    /// That source's `agent-bash` binary.
    pub agent_bash_bin: String,
    /// The only commands `bash` may run, each the whole command string.
    /// Required unless `bash_authority` is given.
    #[serde(default)]
    pub bash_allow: Vec<String>,
    /// An explicit wider `bash` authority instead of `bash_allow`.
    #[serde(default)]
    pub bash_authority: Option<BashAuthority>,
    /// Native model, `provider/model`.
    #[serde(default)]
    pub model: Option<String>,
    /// Native provider config (`provider` in OpenCode's config).
    #[serde(default)]
    pub provider: Option<Map<String, Value>>,
    /// Absolute path of a private OpenCode `auth.json` for this launch
    /// (see the module docs); needs `model`.
    #[serde(default)]
    pub auth: Option<String>,
    /// The `explore` tool, when the root allows registered children.
    #[serde(default)]
    pub explore: Option<ExploreTool>,
}

/// What a parent's `explore` tool describes (the owner enforces it).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExploreTool {
    pub routes: Vec<String>,
    pub max_starts: u32,
    pub max_concurrent: u32,
}

impl ExploreTool {
    pub(crate) fn check(&self) -> Result<(), String> {
        if self.routes.is_empty() || self.routes.iter().any(String::is_empty) {
            return Err("explore names no route".to_owned());
        }
        Ok(())
    }
}

/// A `bash` authority wider than a named-command list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BashAuthority {
    /// Any command, for this task (see the module docs).
    TrustedTask,
}

/// The effective native policy of a launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    /// `None`: `trusted-task`; else the named whole commands.
    pub bash_allow: Option<Vec<String>>,
    /// The `explore` tool's routes, when it is offered.
    pub explore: Option<Vec<String>>,
    /// The native permission config as written to `opencode.json`.
    pub native: String,
}

impl Policy {
    /// The policy `setup` selects, refused unless exactly one form is given.
    pub fn of(setup: &OpenCodeSetup) -> Result<Self, String> {
        let mut policy = match setup.bash_authority {
            Some(BashAuthority::TrustedTask) if !setup.bash_allow.is_empty() => {
                return Err("bash_allow and bash_authority are exclusive".to_owned());
            }
            Some(BashAuthority::TrustedTask) => Self {
                bash_allow: None,
                explore: None,
                native: TRUSTED_TASK_PERMISSION.to_owned(),
            },
            None => Self {
                native: permission(&setup.bash_allow)?,
                bash_allow: Some(setup.bash_allow.clone()),
                explore: None,
            },
        };
        if let Some(explore) = &setup.explore {
            explore.check()?;
            // Named after `*`: the last matching rule wins natively.
            let native = policy.native.strip_suffix('}').expect("json object");
            policy.native = format!(r#"{native},"explore":"allow"}}"#);
            policy.explore = Some(explore.routes.clone());
        }
        Ok(policy)
    }

    pub fn to_json(&self) -> Value {
        let mut value = json!({
            "bash": match &self.bash_allow {
                None => json!("trusted-task"),
                Some(commands) => json!({ "allow": commands }),
            },
            "other": "deny",
            "native": self.native,
        });
        if let Some(routes) = &self.explore {
            value["explore"] = json!({ "routes": routes });
        }
        value
    }
}

/// A provisioned launch: the harness `argv` and what it sets.
#[derive(Debug, Clone)]
pub struct OpenCodeLaunch {
    pub argv: Vec<String>,
    /// Variables the `argv` sets for the host, in order.
    pub env: Vec<(String, String)>,
    /// The native config directory (`$XDG_CONFIG_HOME/opencode`).
    pub config_dir: PathBuf,
    /// Where an authenticated launch's secrets are (paths, never values).
    pub auth: Option<AuthPlacement>,
    /// The effective native policy written for the host.
    pub policy: Policy,
}

/// An authenticated launch's secret files.
#[derive(Debug, Clone)]
pub struct AuthPlacement {
    /// The native data directory's `opencode/auth.json`.
    pub auth_file: PathBuf,
    /// The loopback server password the host reads as it starts.
    pub password_file: PathBuf,
}

impl OpenCodeLaunch {
    pub fn to_json(&self) -> Value {
        let env: Map<String, Value> = self
            .env
            .iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect();
        json!({
            "argv": self.argv,
            "endpoint": "unix-socket",
            "env": env,
            "removed_env": REMOVED_ENV,
            "config_dir": self.config_dir,
            "policy": self.policy.to_json(),
            "auth": self.auth.as_ref().map(|auth| json!({
                "auth_file": auth.auth_file,
                "server_password_file": auth.password_file,
                "default_plugins": "enabled",
            })),
        })
    }
}

fn absolute(name: &str, path: &str) -> Result<PathBuf, String> {
    if !path.starts_with('/') {
        return Err(format!("{name} must be absolute"));
    }
    Ok(PathBuf::from(path))
}

fn utf8(path: &Path) -> Result<String, String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("{path:?} is not UTF-8"))
}

/// The native permission config for `trusted-task`: every other tool
/// denied, `bash` allowed for any command (`*` is a native glob).
pub const TRUSTED_TASK_PERMISSION: &str = r#"{"*":"deny","bash":{"*":"allow"}}"#;

/// The root's native permission config for `bash_allow`, as JSON text.
/// Native evaluation takes the last matching rule in config order, so the
/// order is written explicitly (`*` first), not left to a JSON map.
pub fn permission(bash_allow: &[String]) -> Result<String, String> {
    if bash_allow.is_empty() {
        return Err("bash_allow names no command".to_owned());
    }
    let mut bash = String::from(r#"{"*":"deny""#);
    for command in bash_allow {
        if command.trim().is_empty() {
            return Err("bash_allow has an empty command".to_owned());
        }
        if command.contains(['*', '?', '\\']) {
            return Err(format!(
                "bash_allow {command:?}: `*`, `?` and `\\` are native glob syntax"
            ));
        }
        if command.bytes().all(|byte| byte.is_ascii_digit()) {
            // A JavaScript object puts integer-like keys before `*`.
            return Err(format!("bash_allow {command:?}: integer-like keys reorder"));
        }
        if command.starts_with("handle ") {
            return Err(format!(
                "bash_allow {command:?}: handle polls are not commands"
            ));
        }
        bash.push(',');
        bash.push_str(&Value::String(command.clone()).to_string());
        bash.push_str(r#":"allow""#);
    }
    bash.push('}');
    Ok(format!(r#"{{"*":"deny","bash":{bash}}}"#))
}

fn write_new(path: &Path, contents: &str) -> Result<(), String> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .and_then(|mut file| file.write_all(contents.as_bytes()))
        .map_err(|error| format!("{}: {error}", path.display()))
}

/// Why setup did not return a provisioned launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenCodeSetupError {
    /// Input or prerequisites were refused before any setup writes.
    InputInvalid(String),
    /// Construction was attempted; a partly written directory may remain.
    /// The caller must inspect effects rather than automatically replay.
    ConstructionFailed(String),
}

impl std::fmt::Display for OpenCodeSetupError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InputInvalid(reason) | Self::ConstructionFailed(reason) => {
                formatter.write_str(reason)
            }
        }
    }
}

impl std::error::Error for OpenCodeSetupError {}

/// The checks [`provision_opencode`] makes before any write, alone: what a
/// caller can confirm before effects of its own (no write happens here).
pub fn check_opencode(setup: &OpenCodeSetup) -> Result<(), String> {
    setup_inputs(setup).map(drop)
}

/// Creates the launch directory for `setup` and returns its launch.
/// [`OpenCodeSetupError::InputInvalid`] means no setup writes occurred.
/// [`OpenCodeSetupError::ConstructionFailed`] may leave partial effects;
/// inspect them before any further attempt. Success means provisioning,
/// not native launch or delivery of a caller's receipt.
pub fn provision_opencode(
    setup: &OpenCodeSetup,
    identity: Option<&Identity>,
) -> Result<OpenCodeLaunch, OpenCodeSetupError> {
    let inputs = setup_inputs(setup).map_err(OpenCodeSetupError::InputInvalid)?;
    let launch = write_launch(setup, inputs).map_err(OpenCodeSetupError::ConstructionFailed)?;
    if let Some(identity) = identity {
        hand_over(
            Path::new(&setup.dir),
            identity,
            launch.auth.is_some(),
            setup.explore.is_some(),
        )
        .map_err(OpenCodeSetupError::ConstructionFailed)?;
    }
    Ok(launch)
}

/// Gives the fresh launch tree its `host-root` ownership (see the module
/// docs). The launch directory stays `0700` and the caller's until last,
/// so nothing here can be raced by another user.
fn hand_over(dir: &Path, identity: &Identity, secrets: bool, explore: bool) -> Result<(), String> {
    use std::os::unix::fs::{PermissionsExt, lchown};
    let set = |path: &Path, uid: Option<u32>, mode: Option<u32>| {
        lchown(path, uid, Some(identity.gid))
            .and_then(|()| match mode {
                Some(mode) => fs::set_permissions(path, fs::Permissions::from_mode(mode)),
                None => Ok(()),
            })
            .map_err(|error| format!("{}: {error}", path.display()))
    };
    let config = dir.join("xdg/config/opencode");
    for file in [
        "acp-v2-endpoint.ts",
        "tool/bash.ts",
        "agent-bash/bash.ts",
        "package.json",
        "package-lock.json",
        "opencode.json",
    ] {
        set(&config.join(file), None, Some(0o640))?;
    }
    if explore {
        for file in ["tool/explore.ts", "explore-client.mjs", "explore.json"] {
            set(&config.join(file), None, Some(0o640))?;
        }
    }
    set(&config.join("node_modules"), None, None)?;
    for sub in ["tool", "agent-bash"] {
        set(&config.join(sub), None, Some(0o750))?;
    }
    set(&config, None, Some(0o1770))?;
    if secrets {
        for (sub, mode) in [
            ("secret/server-password", 0o600),
            ("secret", 0o700),
            ("xdg/data/opencode/auth.json", 0o600),
            ("xdg/data/opencode", 0o700),
        ] {
            set(&dir.join(sub), Some(identity.uid), Some(mode))?;
        }
    }
    for sub in ["home", "xdg/data", "xdg/cache", "xdg/state"] {
        set(&dir.join(sub), Some(identity.uid), Some(0o700))?;
    }
    for sub in ["xdg/config", "xdg", ""] {
        set(&dir.join(sub), None, Some(0o750))?;
    }
    Ok(())
}

struct SetupInputs<'a> {
    dir: PathBuf,
    deps: PathBuf,
    bin: PathBuf,
    policy: Policy,
    tool_source: String,
    model_provider: Option<&'a str>,
    /// The checked `auth` file's text.
    auth: Option<String>,
}

// All setup input checks finish before entering the writing phase.
fn setup_inputs(setup: &OpenCodeSetup) -> Result<SetupInputs<'_>, String> {
    let dir = absolute("dir", &setup.dir)?;
    let deps = absolute("deps", &setup.deps)?;
    let tool = absolute("agent_bash_tool", &setup.agent_bash_tool)?;
    let bin = absolute("agent_bash_bin", &setup.agent_bash_bin)?;
    let policy = Policy::of(setup)?;
    let model_provider = match (&setup.model, &setup.provider) {
        (Some(model), Some(provider)) => {
            let (name, _) = model
                .split_once('/')
                .ok_or("model must be provider/model")?;
            if !provider.contains_key(name) {
                return Err(format!("provider has no {name}"));
            }
            Some(name)
        }
        (None, None) => None,
        _ => return Err("model and provider go together".to_owned()),
    };
    let auth = match (&setup.auth, model_provider) {
        (Some(path), Some(name)) => Some(read_auth(&absolute("auth", path)?, name)?),
        (Some(_), None) => return Err("auth needs a model and provider".to_owned()),
        (None, _) => None,
    };
    let opencode = deps.join(OPENCODE);
    if !opencode.is_file() {
        return Err(format!("no OpenCode binary at {}", opencode.display()));
    }
    match fs::read_to_string(deps.join("package-lock.json")) {
        Ok(lock) if lock == LOCK => {}
        Ok(_) => return Err("deps were installed from a different lockfile".to_owned()),
        Err(error) => return Err(format!("deps package-lock.json: {error}")),
    }
    if !deps.join("node_modules/@opencode-ai/plugin").is_dir() {
        return Err("deps lack @opencode-ai/plugin".to_owned());
    }
    let tool_source =
        fs::read_to_string(&tool).map_err(|error| format!("agent_bash_tool: {error}"))?;
    if !bin.is_file() {
        return Err(format!("no agent-bash binary at {}", bin.display()));
    }
    Ok(SetupInputs {
        dir,
        deps,
        bin,
        policy,
        tool_source,
        model_provider,
        auth,
    })
}

/// Reads and checks the caller's `auth` file. No reason carries any part
/// of its contents: only the path, an I/O error kind, a JSON position or
/// the model's provider name.
fn read_auth(path: &Path, provider: &str) -> Result<String, String> {
    use std::io::Read;
    // No final symlink, and no blocking open of a FIFO.
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| format!("auth {}: {}", path.display(), error.kind()))?;
    let meta = file
        .metadata()
        .map_err(|error| format!("auth: {}", error.kind()))?;
    if !meta.is_file() {
        return Err("auth is not a regular file".to_owned());
    }
    if meta.len() > AUTH_LIMIT {
        return Err(format!("auth is larger than {AUTH_LIMIT} bytes"));
    }
    let mut bytes = Vec::new();
    file.take(AUTH_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("auth: {}", error.kind()))?;
    if bytes.len() as u64 > AUTH_LIMIT {
        return Err(format!("auth is larger than {AUTH_LIMIT} bytes"));
    }
    let text = String::from_utf8(bytes).map_err(|_| "auth is not UTF-8".to_owned())?;
    let value: Value = serde_json::from_str(&text).map_err(|error| {
        format!(
            "auth is not JSON (line {}, column {})",
            error.line(),
            error.column()
        )
    })?;
    let entries = value
        .as_object()
        .ok_or("auth is not a JSON object of provider entries")?;
    // Check required fields; optional types and the full native schema
    // are not validated here, so native decoding may still drop an entry.
    let string = |entry: &Value, field: &str| entry.get(field).is_some_and(Value::is_string);
    for entry in entries.values() {
        let known = match entry.get("type").and_then(Value::as_str) {
            Some("oauth") => {
                string(entry, "refresh")
                    && string(entry, "access")
                    && entry.get("expires").is_some_and(Value::is_u64)
            }
            Some("api") => string(entry, "key"),
            Some("wellknown") => string(entry, "key") && string(entry, "token"),
            _ => false,
        };
        if !known {
            return Err(
                "auth has an entry that is not a complete oauth, api or wellknown entry".to_owned(),
            );
        }
    }
    if !entries.contains_key(provider) {
        return Err(format!("auth has no {provider} entry"));
    }
    Ok(text)
}

/// A fresh loopback server password: 32 random bytes, hex.
fn server_password() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    let mut filled = 0;
    while filled < bytes.len() {
        // SAFETY: the kernel writes at most the remaining length into
        // `bytes` from `filled`.
        let read = unsafe {
            libc::getrandom(bytes[filled..].as_mut_ptr().cast(), bytes.len() - filled, 0)
        };
        if read < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(format!("server password: {error}"));
        }
        filled += read as usize;
    }
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn write_launch(setup: &OpenCodeSetup, inputs: SetupInputs<'_>) -> Result<OpenCodeLaunch, String> {
    let SetupInputs {
        dir,
        deps,
        bin,
        policy,
        tool_source,
        model_provider,
        auth,
    } = inputs;
    let mkdir = |path: &Path| {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(path)
            .map_err(|error| format!("{}: {error}", path.display()))
    };
    mkdir(&dir)?;
    for sub in [
        "home",
        "xdg",
        "xdg/config",
        "xdg/data",
        "xdg/cache",
        "xdg/state",
    ] {
        mkdir(&dir.join(sub))?;
    }
    let config_dir = dir.join("xdg/config/opencode");
    mkdir(&config_dir)?;
    mkdir(&config_dir.join("tool"))?;
    mkdir(&config_dir.join("agent-bash"))?;
    let endpoint = config_dir.join("acp-v2-endpoint.ts");
    write_new(&endpoint, ENDPOINT)?;
    write_new(&config_dir.join("tool/bash.ts"), GATE)?;
    write_new(&config_dir.join("agent-bash/bash.ts"), &tool_source)?;
    write_new(&config_dir.join("package.json"), PACKAGE)?;
    if let Some(explore) = &setup.explore {
        write_new(&config_dir.join("tool/explore.ts"), EXPLORE)?;
        write_new(&config_dir.join("explore-client.mjs"), EXPLORE_CLIENT)?;
        write_new(
            &config_dir.join("explore.json"),
            &serde_json::to_string(explore).expect("explore json"),
        )?;
    }
    write_new(&config_dir.join("package-lock.json"), LOCK)?;
    std::os::unix::fs::symlink(deps.join("node_modules"), config_dir.join("node_modules"))
        .map_err(|error| format!("node_modules: {error}"))?;

    let mut config = json!({
        "$schema": "https://opencode.ai/config.json",
        "plugin": [format!("file://{}", utf8(&endpoint)?)],
        "autoupdate": false,
        "share": "disabled",
        "agent": { "title": { "disable": true } },
    });
    if let (Some(model), Some(provider), Some(name)) =
        (&setup.model, &setup.provider, model_provider)
    {
        config["model"] = json!(model);
        config["enabled_providers"] = json!([name]);
        config["provider"] = Value::Object(provider.clone());
    }
    let config = serde_json::to_string(&config).expect("config json");
    let auth = match auth {
        Some(text) => {
            let secret = dir.join("secret");
            mkdir(&secret)?;
            let password_file = secret.join("server-password");
            write_new(&password_file, &format!("{}\n", server_password()?))?;
            mkdir(&dir.join("xdg/data/opencode"))?;
            let auth_file = dir.join("xdg/data/opencode/auth.json");
            write_new(&auth_file, &text)?;
            Some(AuthPlacement {
                auth_file,
                password_file,
            })
        }
        None => None,
    };
    let config = format!(
        r#"{},"permission":{}}}"#,
        config.strip_suffix('}').expect("json object"),
        policy.native
    );
    write_new(&config_dir.join("opencode.json"), &config)?;

    let path = |sub: &str| utf8(&dir.join(sub));
    let mut env = vec![
        ("HOME".to_owned(), path("home")?),
        ("XDG_CONFIG_HOME".to_owned(), path("xdg/config")?),
        ("XDG_DATA_HOME".to_owned(), path("xdg/data")?),
        ("XDG_CACHE_HOME".to_owned(), path("xdg/cache")?),
        ("XDG_STATE_HOME".to_owned(), path("xdg/state")?),
        ("AGENT_BASH_BIN".to_owned(), utf8(&bin)?),
        ("OPENCODE_DISABLE_PROJECT_CONFIG".to_owned(), "1".to_owned()),
        (
            "OPENCODE_DISABLE_DEFAULT_PLUGINS".to_owned(),
            "1".to_owned(),
        ),
        ("OPENCODE_DISABLE_MODELS_FETCH".to_owned(), "1".to_owned()),
        ("OPENCODE_DISABLE_AUTOUPDATE".to_owned(), "1".to_owned()),
        ("OPENCODE_DISABLE_CLAUDE_CODE".to_owned(), "1".to_owned()),
        ("OPENCODE_DISABLE_LSP_DOWNLOAD".to_owned(), "1".to_owned()),
        ("OPENCODE_DISABLE_SHARE".to_owned(), "1".to_owned()),
    ];
    if auth.is_some() {
        env.retain(|(name, _)| name != "OPENCODE_DISABLE_DEFAULT_PLUGINS");
    }
    // `env` keeps what the owner and root PID 1 add (the Bash ingress and
    // the endpoint socket), removes inherited config, sets the rest and
    // execs OpenCode (through `sh`, which reads the server password into
    // its own environment and execs, for an authenticated launch): no
    // further process. `acp` loads the config directory's plugins at
    // startup; its own stdio ACP is unused.
    let mut argv = vec!["/usr/bin/env".to_owned()];
    for name in REMOVED_ENV {
        argv.push("-u".to_owned());
        argv.push(name.to_owned());
    }
    argv.extend(env.iter().map(|(key, value)| format!("{key}={value}")));
    if let Some(auth) = &auth {
        argv.extend([
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            WITH_PASSWORD.to_owned(),
            utf8(&auth.password_file)?,
        ]);
    }
    argv.extend([
        utf8(&deps.join(OPENCODE))?,
        "acp".to_owned(),
        "--hostname".to_owned(),
        "127.0.0.1".to_owned(),
        "--port".to_owned(),
        "0".to_owned(),
    ]);
    Ok(OpenCodeLaunch {
        argv,
        env,
        config_dir,
        auth,
        policy,
    })
}

#[cfg(test)]
mod tests {
    use super::{Identity, SetupInputs, hand_over, permission, read_auth, write_launch};
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;

    const SECRET: &str = "fixture-secret-marker";

    fn setup(dir: &Path, auth: Option<&str>) -> super::OpenCodeSetup {
        super::OpenCodeSetup {
            dir: dir.to_str().unwrap().to_owned(),
            deps: "/nonexistent-deps".to_owned(),
            agent_bash_tool: "/t".to_owned(),
            agent_bash_bin: "/b".to_owned(),
            bash_allow: vec!["true".to_owned()],
            bash_authority: None,
            model: auth.map(|_| "openai/m".to_owned()),
            provider: auth.map(|_| {
                serde_json::from_str(r#"{"openai":{"models":{"m":{"name":"m"}}}}"#).unwrap()
            }),
            auth: auth.map(str::to_owned),
            explore: None,
        }
    }

    fn inputs(dir: &Path, auth: Option<String>) -> SetupInputs<'static> {
        SetupInputs {
            dir: dir.to_path_buf(),
            deps: "/nonexistent-deps".into(),
            bin: "/b".into(),
            policy: super::Policy::of(&setup(dir, None)).unwrap(),
            tool_source: "// tool".to_owned(),
            model_provider: auth.as_ref().map(|_| "openai"),
            auth,
        }
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let base = std::env::temp_dir().join(format!("native-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir(&base).unwrap();
        base
    }

    fn own_identity() -> Identity {
        // SAFETY: getuid/getgid have no preconditions.
        let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
        Identity {
            user: "self".into(),
            uid,
            gid,
            groups: vec![gid],
        }
    }

    fn oauth() -> String {
        format!(
            r#"{{"openai":{{"type":"oauth","refresh":"","access":"{SECRET}","expires":1,"accountId":"a"}}}}"#
        )
    }

    /// The host-root layout, handed to this (unprivileged) process's own
    /// identity: chowning to oneself needs no privilege, so the modes and
    /// which paths are handed over are checked here; only the uid differs
    /// from a real host-root setup.
    #[test]
    fn hand_over_gives_the_identity_its_state_and_keeps_code_and_policy_read_only() {
        let base = std::env::temp_dir().join(format!("native-hand-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir(&base).unwrap();
        let dir = base.join("launch");
        write_launch(&setup(&dir, None), inputs(&dir, None)).unwrap();
        let identity = own_identity();
        let gid = identity.gid;
        hand_over(&dir, &identity, false, false).unwrap();
        let mode = |sub: &str| {
            let meta = std::fs::symlink_metadata(dir.join(sub)).unwrap();
            assert_eq!(meta.gid(), gid, "{sub}");
            meta.mode() & 0o7777
        };
        for (sub, expected) in [
            ("", 0o750),
            ("home", 0o700),
            ("xdg", 0o750),
            ("xdg/config", 0o750),
            ("xdg/data", 0o700),
            ("xdg/cache", 0o700),
            ("xdg/state", 0o700),
            ("xdg/config/opencode", 0o1770),
            ("xdg/config/opencode/tool", 0o750),
            ("xdg/config/opencode/agent-bash", 0o750),
            ("xdg/config/opencode/opencode.json", 0o640),
            ("xdg/config/opencode/tool/bash.ts", 0o640),
            ("xdg/config/opencode/agent-bash/bash.ts", 0o640),
            ("xdg/config/opencode/acp-v2-endpoint.ts", 0o640),
            ("xdg/config/opencode/package.json", 0o640),
            ("xdg/config/opencode/package-lock.json", 0o640),
        ] {
            assert_eq!(mode(sub), expected, "{sub}");
        }
        assert!(!dir.join("secret").exists() && !dir.join("xdg/data/opencode").exists());
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// Without `auth`, the launch is the offline one: built-in plugins
    /// disabled, no server password, OpenCode exec'd by `env` directly.
    #[test]
    fn launch_without_auth_keeps_plugins_disabled_and_no_password() {
        let base = std::env::temp_dir().join(format!("native-offline-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir(&base).unwrap();
        let dir = base.join("launch");
        let launch = write_launch(&setup(&dir, None), inputs(&dir, None)).unwrap();
        assert!(launch.auth.is_none());
        assert!(
            launch
                .argv
                .contains(&"OPENCODE_DISABLE_DEFAULT_PLUGINS=1".to_owned())
        );
        assert!(
            !launch.argv.contains(&"/bin/sh".to_owned()),
            "{:?}",
            launch.argv
        );
        let opencode = launch
            .argv
            .iter()
            .position(|arg| arg.ends_with("/opencode"));
        assert_eq!(
            launch.argv[opencode.unwrap() - 1],
            "OPENCODE_DISABLE_SHARE=1"
        );
        assert_eq!(launch.to_json()["auth"], serde_json::Value::Null);
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// With `auth`: the auth file and a fresh password are private files,
    /// handed to the identity; plugins are enabled; no argv, config or
    /// receipt carries either value.
    #[test]
    fn launch_with_auth_places_private_secrets_and_keeps_values_out_of_argv_and_config() {
        let base = std::env::temp_dir().join(format!("native-auth-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir(&base).unwrap();
        let dir = base.join("launch");
        let launch = write_launch(
            &setup(&dir, Some("/caller/auth.json")),
            inputs(&dir, Some(oauth())),
        )
        .unwrap();
        let identity = own_identity();
        hand_over(&dir, &identity, true, false).unwrap();
        let placed = launch.auth.as_ref().unwrap();
        assert_eq!(placed.auth_file, dir.join("xdg/data/opencode/auth.json"));
        assert_eq!(placed.password_file, dir.join("secret/server-password"));
        assert_eq!(std::fs::read_to_string(&placed.auth_file).unwrap(), oauth());
        let password = std::fs::read_to_string(&placed.password_file).unwrap();
        let password = password.strip_suffix('\n').unwrap();
        assert_eq!(password.len(), 64);
        assert!(password.bytes().all(|byte| byte.is_ascii_hexdigit()));
        for (sub, expected) in [
            ("secret", 0o700),
            ("secret/server-password", 0o600),
            ("xdg/data/opencode", 0o700),
            ("xdg/data/opencode/auth.json", 0o600),
        ] {
            let meta = std::fs::symlink_metadata(dir.join(sub)).unwrap();
            assert_eq!(
                (meta.uid(), meta.mode() & 0o7777),
                (identity.uid, expected),
                "{sub}"
            );
        }
        // A second launch gets another password.
        let other = base.join("other");
        let again = write_launch(
            &setup(&other, Some("/caller/auth.json")),
            inputs(&other, Some(oauth())),
        )
        .unwrap();
        assert_ne!(
            std::fs::read_to_string(again.auth.unwrap().password_file).unwrap(),
            format!("{password}\n")
        );

        assert!(
            !launch
                .argv
                .iter()
                .any(|arg| arg.starts_with("OPENCODE_DISABLE_DEFAULT_PLUGINS")),
            "{:?}",
            launch.argv
        );
        let sh = launch.argv.iter().position(|arg| arg == "/bin/sh").unwrap();
        assert_eq!(launch.argv[sh + 1], "-c");
        assert_eq!(launch.argv[sh + 3], placed.password_file.to_str().unwrap());
        assert!(launch.argv[sh + 4].ends_with("/opencode"));
        for name in ["OPENCODE_SERVER_PASSWORD", "OPENCODE_AUTH_CONTENT"] {
            let at = launch.argv.iter().position(|arg| arg == name).unwrap();
            assert_eq!(launch.argv[at - 1], "-u");
        }
        let receipt = launch.to_json();
        assert_eq!(receipt["auth"]["default_plugins"], "enabled");
        let config =
            std::fs::read_to_string(dir.join("xdg/config/opencode/opencode.json")).unwrap();
        for text in [format!("{:?}", launch.argv), receipt.to_string(), config] {
            assert!(!text.contains(password), "password in {text}");
            assert!(!text.contains(SECRET), "auth value in {text}");
        }
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// The launcher itself: the host gets the password in its environment
    /// from the file, and an unreadable file stops it before the host.
    #[test]
    fn password_launcher_reads_the_file_into_the_environment_only() {
        let base = scratch("launcher");
        let file = base.as_path().join("pw");
        std::fs::write(&file, "abc123\n").unwrap();
        let run = |file: &Path| {
            std::process::Command::new("/bin/sh")
                .args(["-c", super::WITH_PASSWORD])
                .arg(file)
                .args([
                    "/bin/sh",
                    "-c",
                    r#"printf %s "$OPENCODE_SERVER_PASSWORD"; printf '|%s' "$0""#,
                    "argv0",
                ])
                .env_remove("OPENCODE_SERVER_PASSWORD")
                .output()
                .unwrap()
        };
        let output = run(&file);
        assert!(output.status.success(), "{output:?}");
        assert_eq!(String::from_utf8(output.stdout).unwrap(), "abc123|argv0");
        let missing = run(&base.as_path().join("absent"));
        assert!(!missing.status.success(), "{missing:?}");
        assert!(missing.stdout.is_empty(), "{missing:?}");
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// Refusals of the caller's auth file name no part of its contents.
    #[test]
    fn auth_is_checked_before_writes_and_refusals_never_carry_its_contents() {
        let base = scratch("authfile");
        let write = |name: &str, text: &str| {
            let path = base.as_path().join(name);
            std::fs::write(&path, text).unwrap();
            path
        };
        let good = write("good", &oauth());
        assert_eq!(read_auth(&good, "openai").unwrap(), oauth());
        let link = base.as_path().join("link");
        std::os::unix::fs::symlink(&good, &link).unwrap();
        let fifo = base.as_path().join("fifo");
        let fifo_c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        // SAFETY: a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) }, 0);
        let cases = [
            (base.as_path().join("absent"), "openai", "not found"),
            (link, "openai", "loop"),
            (fifo, "openai", "not a regular file"),
            (
                write("syntax", &format!("{{\"openai\":\"{SECRET}")),
                "openai",
                "not JSON",
            ),
            (
                write("array", &format!("[\"{SECRET}\"]")),
                "openai",
                "not a JSON object",
            ),
            (
                write(
                    "partial",
                    &format!(r#"{{"openai":{{"type":"oauth","access":"{SECRET}","expires":1}}}}"#),
                ),
                "openai",
                "not a complete",
            ),
            (
                write("kind", &format!(r#"{{"openai":{{"type":"{SECRET}"}}}}"#)),
                "openai",
                "not a complete",
            ),
            (good.clone(), "anthropic", "no anthropic entry"),
            (
                write("large", &format!("{{\"x\":\"{}\"}}", "a".repeat(70_000))),
                "openai",
                "larger than",
            ),
        ];
        for (path, provider, reason) in cases {
            let refused = read_auth(&path, provider).unwrap_err();
            assert!(refused.contains(reason), "{path:?}: {refused}");
            assert!(!refused.contains(SECRET), "{path:?}: {refused}");
        }
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// `auth` without a model is refused by input checks, before writes.
    #[test]
    fn auth_needs_a_model() {
        let base = scratch("nomodel");
        let dir = base.as_path().join("launch");
        let mut setup = setup(&dir, Some("/caller/auth.json"));
        setup.model = None;
        setup.provider = None;
        let refused = super::provision_opencode(&setup, None).unwrap_err();
        assert_eq!(
            refused,
            super::OpenCodeSetupError::InputInvalid("auth needs a model and provider".to_owned())
        );
        assert!(!dir.exists());
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn permission_denies_by_default_and_allows_only_named_commands() {
        let policy = permission(&["true".to_owned(), "printf \"a\"".to_owned()]).unwrap();
        // `*` first: a named command is the last matching rule.
        assert_eq!(
            policy,
            r#"{"*":"deny","bash":{"*":"deny","true":"allow","printf \"a\"":"allow"}}"#
        );
        serde_json::from_str::<serde_json::Value>(&policy).unwrap();
    }

    /// `trusted-task` is selected only explicitly, never with a list, and
    /// a setup naming neither is refused as before.
    #[test]
    fn policy_is_the_named_list_unless_trusted_task_is_explicit() {
        use super::{BashAuthority, Policy};
        let mut setup = setup(Path::new("/launch"), None);
        let listed = Policy::of(&setup).unwrap();
        assert_eq!(listed.native, permission(&["true".to_owned()]).unwrap());
        assert_eq!(
            listed.to_json(),
            serde_json::json!({
                "bash": { "allow": ["true"] },
                "other": "deny",
                "native": r#"{"*":"deny","bash":{"*":"deny","true":"allow"}}"#,
            })
        );

        setup.bash_authority = Some(BashAuthority::TrustedTask);
        assert_eq!(
            Policy::of(&setup).unwrap_err(),
            "bash_allow and bash_authority are exclusive"
        );
        setup.bash_allow.clear();
        let trusted = Policy::of(&setup).unwrap();
        // `*` first at both levels: native evaluation takes the last
        // matching rule, so nothing but `bash` is allowed.
        assert_eq!(trusted.native, r#"{"*":"deny","bash":{"*":"allow"}}"#);
        assert_eq!(trusted.to_json()["bash"], "trusted-task");
        assert_eq!(trusted.to_json()["other"], "deny");

        setup.bash_authority = None;
        assert_eq!(
            Policy::of(&setup).unwrap_err(),
            "bash_allow names no command"
        );
        for text in [r#""trusted""#, r#""TrustedTask""#, "true"] {
            assert!(
                serde_json::from_str::<BashAuthority>(text).is_err(),
                "{text}"
            );
        }
        assert_eq!(
            serde_json::from_str::<BashAuthority>(r#""trusted-task""#).unwrap(),
            BashAuthority::TrustedTask
        );
    }

    #[test]
    fn permission_refuses_globs_empty_and_handles() {
        for bad in [
            vec![],
            vec!["".to_owned()],
            vec!["ls *".to_owned()],
            vec!["a?".to_owned()],
            vec!["a\\b".to_owned()],
            vec!["handle x".to_owned()],
            vec!["42".to_owned()],
        ] {
            assert!(permission(&bad).is_err(), "{bad:?}");
        }
    }
}
