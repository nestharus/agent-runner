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
//! * **Policy.** Deny by default: every native tool other than `bash` is
//!   denied (and so hidden), and `bash` runs only the commands the caller
//!   named, each matched as the whole command string; anything else is a
//!   native denial, with no permission request to the owner. Native
//!   OpenCode enforces it; the owner grants nothing. Entries with `*`, `?`
//!   or `\` are refused, since native matching would treat them as globs.
//!   The workdir and delivery are not part of a decision.
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
//! * Nothing here launches, models or authenticates: a model and provider
//!   are the caller's (`model`, `provider`), and the native data directory
//!   starts empty.
//! * **Work identity.** Without one, everything is the caller's (`0700` /
//!   `0600`), as for an unprivileged root. With a `host-root` work identity
//!   (the caller is host root), the fresh tree is handed over before it is
//!   opened up: HOME and the XDG data, cache and state directories become
//!   the identity's (`0700`); the launch, XDG and XDG config directories,
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

use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::workload::Identity;

const ENDPOINT: &str = include_str!("../native/opencode/acp-v2-endpoint.ts");
const GATE: &str = include_str!("../native/opencode/bash-policy-tool.ts");
const PACKAGE: &str = include_str!("../native/opencode/package.json");
const LOCK: &str = include_str!("../native/opencode/package-lock.json");

/// The OpenCode binary inside the installed dependencies.
const OPENCODE: &str = "node_modules/opencode-linux-x64/bin/opencode";

/// Inherited variables that would add config beside the launch's own.
pub const REMOVED_ENV: [&str; 4] = [
    "OPENCODE_CONFIG",
    "OPENCODE_CONFIG_DIR",
    "OPENCODE_CONFIG_CONTENT",
    "OPENCODE_PERMISSION",
];

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
    pub bash_allow: Vec<String>,
    /// Native model, `provider/model`.
    #[serde(default)]
    pub model: Option<String>,
    /// Native provider config (`provider` in OpenCode's config).
    #[serde(default)]
    pub provider: Option<Map<String, Value>>,
}

/// A provisioned launch: the harness `argv` and what it sets.
#[derive(Debug, Clone)]
pub struct OpenCodeLaunch {
    pub argv: Vec<String>,
    /// Variables the `argv` sets for the host, in order.
    pub env: Vec<(String, String)>,
    /// The native config directory (`$XDG_CONFIG_HOME/opencode`).
    pub config_dir: PathBuf,
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
        hand_over(Path::new(&setup.dir), identity)
            .map_err(OpenCodeSetupError::ConstructionFailed)?;
    }
    Ok(launch)
}

/// Gives the fresh launch tree its `host-root` ownership (see the module
/// docs). The launch directory stays `0700` and the caller's until last,
/// so nothing here can be raced by another user.
fn hand_over(dir: &Path, identity: &Identity) -> Result<(), String> {
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
    set(&config.join("node_modules"), None, None)?;
    for sub in ["tool", "agent-bash"] {
        set(&config.join(sub), None, Some(0o750))?;
    }
    set(&config, None, Some(0o1770))?;
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
    permission: String,
    tool_source: String,
    model_provider: Option<&'a str>,
}

// All setup input checks finish before entering the writing phase.
fn setup_inputs(setup: &OpenCodeSetup) -> Result<SetupInputs<'_>, String> {
    let dir = absolute("dir", &setup.dir)?;
    let deps = absolute("deps", &setup.deps)?;
    let tool = absolute("agent_bash_tool", &setup.agent_bash_tool)?;
    let bin = absolute("agent_bash_bin", &setup.agent_bash_bin)?;
    let permission = permission(&setup.bash_allow)?;
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
        permission,
        tool_source,
        model_provider,
    })
}

fn write_launch(setup: &OpenCodeSetup, inputs: SetupInputs<'_>) -> Result<OpenCodeLaunch, String> {
    let SetupInputs {
        dir,
        deps,
        bin,
        permission,
        tool_source,
        model_provider,
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
    let config = format!(
        r#"{},"permission":{permission}}}"#,
        config.strip_suffix('}').expect("json object")
    );
    write_new(&config_dir.join("opencode.json"), &config)?;

    let path = |sub: &str| utf8(&dir.join(sub));
    let env = vec![
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
    // `env` keeps what the owner and root PID 1 add (the Bash ingress and
    // the endpoint socket), removes inherited config, sets the rest and
    // execs OpenCode: no further process. `acp` loads the config
    // directory's plugins at startup; its own stdio ACP is unused.
    let mut argv = vec!["/usr/bin/env".to_owned()];
    for name in REMOVED_ENV {
        argv.push("-u".to_owned());
        argv.push(name.to_owned());
    }
    argv.extend(env.iter().map(|(key, value)| format!("{key}={value}")));
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
    })
}

#[cfg(test)]
mod tests {
    use super::{Identity, SetupInputs, hand_over, permission, write_launch};
    use std::os::unix::fs::MetadataExt;

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
        let setup = super::OpenCodeSetup {
            dir: dir.to_str().unwrap().to_owned(),
            deps: "/nonexistent-deps".to_owned(),
            agent_bash_tool: "/t".to_owned(),
            agent_bash_bin: "/b".to_owned(),
            bash_allow: vec!["true".to_owned()],
            model: None,
            provider: None,
        };
        let inputs = SetupInputs {
            dir: dir.clone(),
            deps: "/nonexistent-deps".into(),
            bin: "/b".into(),
            permission: permission(&setup.bash_allow).unwrap(),
            tool_source: "// tool".to_owned(),
            model_provider: None,
        };
        write_launch(&setup, inputs).unwrap();
        // SAFETY: getuid/getgid have no preconditions.
        let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
        let identity = Identity {
            user: "self".into(),
            uid,
            gid,
            groups: vec![gid],
        };
        hand_over(&dir, &identity).unwrap();
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
