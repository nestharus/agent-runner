//! Owner-side setup of one native Claude Code harness for a root (Linux).
//!
//! [`provision_claude`] writes a fresh private launch directory and
//! returns the harness `argv` that starts the ACP v2 receiver
//! (`native/claude/acp-v2-receiver.mjs`) with the caller's Node runtime.
//! The receiver speaks ACP v2 on its stdin/stdout (a `stdio` harness) and
//! drives the unmodified Claude Code executable through the published
//! Claude Agent SDK, both from the caller's installed dependencies of this
//! crate's `native/claude/package.json` (`npm ci --ignore-scripts`) beside
//! exactly this crate's lockfile. The `argv` is kept in the root's durable
//! intent, so every launch and relaunch reads the same directory.
//!
//! * **Credential.** None passes through here. Claude Code runs as the
//!   work identity against `config_dir`, that user's own Claude
//!   configuration directory, and its own login there pays and refreshes.
//!   Setup never opens, lists or checks `config_dir`; it only names it to
//!   the receiver. Every inherited `ANTHROPIC_*` and `CLAUDE*` variable
//!   (API keys, OAuth tokens, provider and endpoint redirects, another
//!   config directory) is withheld from Claude Code by the receiver; the
//!   named credential and loader variables in [`REMOVED_ENV`] are also
//!   removed from the receiver itself. Claude Code's own transcripts and
//!   state are written in that store, outside the root's run directory.
//! * **Policy.** No user, project or local settings, hooks, plugins,
//!   CLAUDE.md or auto memory, and only the receiver's own in-process MCP
//!   server. Permission mode `dontAsk`: anything not pre-approved is
//!   denied, never asked. Built-in execution and delegation tools (Bash,
//!   Agent/Task, Monitor, background tasks, web, skills) are not offered.
//!   `bash` is the receiver's attributed tool: every command goes through
//!   agent-bash `run --delivery sync` into the root's own Bash ingress.
//!   - `bash_allow` (the default form): only the named whole commands;
//!     anything else is refused by the tool and nothing runs. No other tool
//!     is offered.
//!   - `bash_authority: "trusted-task"`: any command, as once-per-task
//!     authority, and the built-in `Read`, `Write` and `Edit` file tools,
//!     whose edits are made by the harness itself (not Bash work records).
//!
//!   This is constructed configuration, not a certificate of absence: a
//!   trusted shell keeps the work user's normal host rights, and Claude
//!   Code's managed settings (`/etc/claude-code`) still apply.
//! * **Model.** `model` and `effort` are passed to Claude Code as given; the
//!   receiver reports a different model in Claude Code's init as a warning
//!   notice. No version gate: unsupported behavior is a visible outcome.
//! * **Work identity.** Without one, everything is the caller's (`0700` /
//!   `0600`). With a `host-root` identity the tree stays the caller's with
//!   the identity's primary group (`0750` / `0640`), so the harness reads
//!   its code and launch file but cannot change them; the receiver writes
//!   nothing there. The caller's `deps`, Node runtime and agent-bash binary
//!   are only read and must be readable and executable by the identity.

use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::native::BashAuthority;
use crate::workload::Identity;

const RECEIVER: &str = include_str!("../native/claude/acp-v2-receiver.mjs");
const PACKAGE: &str = include_str!("../native/claude/package.json");
const LOCK: &str = include_str!("../native/claude/package-lock.json");

/// The published Claude Code executable inside the installed dependencies
/// (the Agent SDK's Linux x64 glibc platform package), used unmodified.
pub const CLAUDE_EXECUTABLE: &str = "node_modules/@anthropic-ai/claude-agent-sdk-linux-x64/claude";

/// Removed from the receiver's own environment by its `argv`: Node loader
/// and debug hooks, and named credential, provider and config-directory
/// variables. (The receiver withholds every `ANTHROPIC_*` and `CLAUDE*`
/// name from Claude Code itself.)
pub const REMOVED_ENV: [&str; 16] = [
    "NODE_OPTIONS",
    "NODE_PATH",
    "DEBUG_CLAUDE_AGENT_SDK",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_REFRESH_TOKEN",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
    "CLAUDE_CODE_USE_MANTLE",
    "CLAUDE_CODE_USE_ANTHROPIC_AWS",
    "CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST",
    "AWS_BEARER_TOKEN_BEDROCK",
    "CLAUDE_CONFIG_DIR",
];

/// Bounds the receiver applies when the setup names none: Claude Code's
/// control handshake at session/new, and its consumption echo of a prompt.
pub const START_TIMEOUT_S: u32 = 120;
pub const ACK_TIMEOUT_S: u32 = 120;
const TIMEOUT_MAX_S: u32 = 3600;

/// Built-in tools a `trusted-task` launch offers besides `bash`.
pub const TRUSTED_TASK_TOOLS: [&str; 3] = ["Read", "Write", "Edit"];

/// What the caller chooses for one native Claude Code harness.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaudeSetup {
    /// Absolute path of the launch directory to create; must not exist.
    pub dir: String,
    /// Absolute path of the installed dependencies (see module docs).
    pub deps: String,
    /// Absolute path of the Node runtime that runs the receiver.
    pub node: String,
    /// The agent-bash binary the `bash` tool runs.
    pub agent_bash_bin: String,
    /// The only commands `bash` may run, each the whole command string.
    /// Required unless `bash_authority` is given.
    #[serde(default)]
    pub bash_allow: Vec<String>,
    /// An explicit wider `bash` authority instead of `bash_allow`.
    #[serde(default)]
    pub bash_authority: Option<BashAuthority>,
    /// Claude model id, e.g. `claude-opus-5-5`.
    pub model: String,
    pub effort: Effort,
    /// Absolute path of the work user's own Claude configuration
    /// directory. Never read by setup.
    pub config_dir: String,
    #[serde(default)]
    pub start_timeout_s: Option<u32>,
    #[serde(default)]
    pub ack_timeout_s: Option<u32>,
}

/// Claude Code's effort levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl Effort {
    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}

/// The effective policy of a launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudePolicy {
    /// `None`: `trusted-task`; else the named whole commands.
    pub bash_allow: Option<Vec<String>>,
    /// Built-in tools offered besides `bash`.
    pub tools: Vec<String>,
}

impl ClaudePolicy {
    /// The policy `setup` selects, refused unless exactly one form is given.
    pub fn of(setup: &ClaudeSetup) -> Result<Self, String> {
        match setup.bash_authority {
            Some(BashAuthority::TrustedTask) if !setup.bash_allow.is_empty() => {
                Err("bash_allow and bash_authority are exclusive".to_owned())
            }
            Some(BashAuthority::TrustedTask) => Ok(Self {
                bash_allow: None,
                tools: TRUSTED_TASK_TOOLS.map(str::to_owned).to_vec(),
            }),
            None => {
                if setup.bash_allow.is_empty() {
                    return Err("bash_allow names no command".to_owned());
                }
                if setup
                    .bash_allow
                    .iter()
                    .any(|command| command.trim().is_empty())
                {
                    return Err("bash_allow has an empty command".to_owned());
                }
                Ok(Self {
                    bash_allow: Some(setup.bash_allow.clone()),
                    tools: Vec::new(),
                })
            }
        }
    }

    fn bash(&self) -> Value {
        match &self.bash_allow {
            None => json!({ "authority": "trusted-task" }),
            Some(commands) => json!({ "allow": commands }),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "bash": match &self.bash_allow {
                None => json!("trusted-task"),
                Some(commands) => json!({ "allow": commands }),
            },
            "bash_route": "agent-bash run --delivery sync -> root Bash ingress",
            "builtin_tools": self.tools,
            "other": "deny",
            "permission_mode": "dontAsk",
            "setting_sources": [],
            "mcp": "strict: the receiver's own server only",
        })
    }
}

/// A provisioned launch: the harness `argv` and what it names.
#[derive(Debug, Clone)]
pub struct ClaudeLaunch {
    pub argv: Vec<String>,
    /// The receiver's launch file.
    pub launch_file: PathBuf,
    /// The launch file's contents (no secret: paths, model and policy).
    pub launch: Value,
    pub policy: ClaudePolicy,
}

impl ClaudeLaunch {
    pub fn to_json(&self) -> Value {
        json!({
            "argv": self.argv,
            "endpoint": "stdio",
            "removed_env": REMOVED_ENV,
            "launch_file": self.launch_file,
            "model": self.launch["model"],
            "effort": self.launch["effort"],
            "claude_executable": self.launch["claude_executable"],
            "config_dir": self.launch["claude_env"]["CLAUDE_CONFIG_DIR"],
            "claude_env_set": self.launch["claude_env"],
            "claude_env_withheld": "every inherited ANTHROPIC_* and CLAUDE* name, OULIPOLY_*, AGENT_BASH_*, NODE_*, OTEL_*",
            "credential": "none: Claude Code's own login in config_dir, never read by setup",
            "policy": self.policy.to_json(),
        })
    }
}

/// Why setup did not return a provisioned launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaudeSetupError {
    /// Input or prerequisites were refused before any setup writes.
    InputInvalid(String),
    /// Construction was attempted; a partly written directory may remain.
    /// The caller must inspect effects rather than automatically replay.
    ConstructionFailed(String),
}

impl std::fmt::Display for ClaudeSetupError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InputInvalid(reason) | Self::ConstructionFailed(reason) => {
                formatter.write_str(reason)
            }
        }
    }
}

impl std::error::Error for ClaudeSetupError {}

fn absolute(name: &str, path: &str) -> Result<PathBuf, String> {
    if !path.starts_with('/') || path.contains('\0') {
        return Err(format!("{name} must be absolute"));
    }
    Ok(PathBuf::from(path))
}

fn utf8(path: &Path) -> Result<String, String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("{path:?} is not UTF-8"))
}

fn timeout(name: &str, value: Option<u32>, default: u32) -> Result<u32, String> {
    match value {
        None => Ok(default),
        Some(value) if (1..=TIMEOUT_MAX_S).contains(&value) => Ok(value),
        Some(_) => Err(format!("{name} must be 1..{TIMEOUT_MAX_S}")),
    }
}

struct Inputs {
    dir: PathBuf,
    deps: PathBuf,
    launch: Value,
    node: String,
    policy: ClaudePolicy,
}

// All input checks finish before entering the writing phase.
fn inputs(setup: &ClaudeSetup) -> Result<Inputs, String> {
    let dir = absolute("dir", &setup.dir)?;
    let deps = absolute("deps", &setup.deps)?;
    let node = absolute("node", &setup.node)?;
    let bin = absolute("agent_bash_bin", &setup.agent_bash_bin)?;
    // Named only; never opened or checked here.
    let config_dir = absolute("config_dir", &setup.config_dir)?;
    let policy = ClaudePolicy::of(setup)?;
    if setup.model.is_empty()
        || setup.model.len() > 128
        || !setup
            .model
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-._[]".contains(&byte))
    {
        return Err("model must be a Claude model id".to_owned());
    }
    let start = timeout("start_timeout_s", setup.start_timeout_s, START_TIMEOUT_S)?;
    let ack = timeout("ack_timeout_s", setup.ack_timeout_s, ACK_TIMEOUT_S)?;
    match fs::read_to_string(deps.join("package-lock.json")) {
        Ok(lock) if lock == LOCK => {}
        Ok(_) => return Err("deps were installed from a different lockfile".to_owned()),
        Err(error) => return Err(format!("deps package-lock.json: {error}")),
    }
    for package in [
        "@anthropic-ai/claude-agent-sdk",
        "@agentclientprotocol/sdk",
        "zod",
    ] {
        if !deps.join("node_modules").join(package).is_dir() {
            return Err(format!("deps lack {package}"));
        }
    }
    let claude = deps.join(CLAUDE_EXECUTABLE);
    if !claude.is_file() {
        return Err(format!("no Claude Code executable at {}", claude.display()));
    }
    if !node.is_file() {
        return Err(format!("no Node runtime at {}", node.display()));
    }
    if !bin.is_file() {
        return Err(format!("no agent-bash binary at {}", bin.display()));
    }
    let launch = json!({
        "model": setup.model,
        "effort": setup.effort.label(),
        "claude_executable": utf8(&claude)?,
        "agent_bash_bin": utf8(&bin)?,
        "bash": policy.bash(),
        "tools": policy.tools,
        "start_timeout_s": start,
        "ack_timeout_s": ack,
        "claude_env": {
            "CLAUDE_CONFIG_DIR": utf8(&config_dir)?,
            // The receiver's one MCP tool is loaded upfront, never deferred.
            "ENABLE_TOOL_SEARCH": "false",
            // The packaged executable is used as published.
            "DISABLE_AUTOUPDATER": "1",
            "DISABLE_UPDATES": "1",
            "CLAUDE_CODE_DISABLE_CLAUDE_MDS": "1",
            "CLAUDE_CODE_DISABLE_AUTO_MEMORY": "1",
            "CLAUDE_CODE_DISABLE_BACKGROUND_TASKS": "1",
            "CLAUDE_AGENT_SDK_CLIENT_APP": "oulipoly-native-root",
        },
    });
    Ok(Inputs {
        dir,
        deps,
        launch,
        node: utf8(&node)?,
        policy,
    })
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

/// Creates the launch directory for `setup` and returns its launch.
/// [`ClaudeSetupError::InputInvalid`] means no setup writes occurred.
/// [`ClaudeSetupError::ConstructionFailed`] may leave partial effects;
/// inspect them before any further attempt. Success means provisioning,
/// not a harness launch, a login or a model's availability.
pub fn provision_claude(
    setup: &ClaudeSetup,
    identity: Option<&Identity>,
) -> Result<ClaudeLaunch, ClaudeSetupError> {
    let inputs = inputs(setup).map_err(ClaudeSetupError::InputInvalid)?;
    let launch = write_launch(inputs).map_err(ClaudeSetupError::ConstructionFailed)?;
    if let Some(identity) = identity {
        hand_over(Path::new(&setup.dir), identity).map_err(ClaudeSetupError::ConstructionFailed)?;
    }
    Ok(launch)
}

fn write_launch(inputs: Inputs) -> Result<ClaudeLaunch, String> {
    let Inputs {
        dir,
        deps,
        launch,
        node,
        policy,
    } = inputs;
    let mkdir = |path: &Path| {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(path)
            .map_err(|error| format!("{}: {error}", path.display()))
    };
    mkdir(&dir)?;
    let claude = dir.join("claude");
    mkdir(&claude)?;
    let receiver = claude.join("acp-v2-receiver.mjs");
    write_new(&receiver, RECEIVER)?;
    write_new(&claude.join("package.json"), PACKAGE)?;
    write_new(&claude.join("package-lock.json"), LOCK)?;
    std::os::unix::fs::symlink(deps.join("node_modules"), claude.join("node_modules"))
        .map_err(|error| format!("node_modules: {error}"))?;
    let launch_file = claude.join("launch.json");
    write_new(
        &launch_file,
        &serde_json::to_string_pretty(&launch).expect("launch json"),
    )?;
    // `env` removes the named variables and execs Node directly: no shell,
    // no further process. The owner and root PID 1 add the Bash ingress.
    let mut argv = vec!["/usr/bin/env".to_owned()];
    for name in REMOVED_ENV {
        argv.push("-u".to_owned());
        argv.push(name.to_owned());
    }
    argv.extend([node, utf8(&receiver)?, utf8(&launch_file)?]);
    Ok(ClaudeLaunch {
        argv,
        launch_file,
        launch,
        policy,
    })
}

/// Gives the fresh launch tree its `host-root` ownership: the caller's,
/// with the identity's primary group able to read and traverse it. The
/// launch directory stays `0700` until last.
fn hand_over(dir: &Path, identity: &Identity) -> Result<(), String> {
    use std::os::unix::fs::{PermissionsExt, lchown};
    let set = |path: &Path, mode: Option<u32>| {
        lchown(path, None, Some(identity.gid))
            .and_then(|()| match mode {
                Some(mode) => fs::set_permissions(path, fs::Permissions::from_mode(mode)),
                None => Ok(()),
            })
            .map_err(|error| format!("{}: {error}", path.display()))
    };
    let claude = dir.join("claude");
    for file in [
        "acp-v2-receiver.mjs",
        "package.json",
        "package-lock.json",
        "launch.json",
    ] {
        set(&claude.join(file), Some(0o640))?;
    }
    set(&claude.join("node_modules"), None)?;
    set(&claude, Some(0o750))?;
    set(dir, Some(0o750))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    fn scratch(name: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!("claude-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir(&base).unwrap();
        base
    }

    /// A deps directory with this crate's lockfile and stand-in packages
    /// and files: enough for setup's checks, nothing that runs.
    fn deps(base: &Path) -> PathBuf {
        let deps = base.join("deps");
        for package in [
            "@anthropic-ai/claude-agent-sdk",
            "@anthropic-ai/claude-agent-sdk-linux-x64",
            "@agentclientprotocol/sdk",
            "zod",
        ] {
            fs::create_dir_all(deps.join("node_modules").join(package)).unwrap();
        }
        fs::write(deps.join("package-lock.json"), LOCK).unwrap();
        fs::write(deps.join(CLAUDE_EXECUTABLE), "stand-in").unwrap();
        fs::write(base.join("node"), "stand-in").unwrap();
        fs::write(base.join("agent-bash"), "stand-in").unwrap();
        deps
    }

    fn setup(base: &Path) -> ClaudeSetup {
        ClaudeSetup {
            dir: base.join("launch").to_str().unwrap().to_owned(),
            deps: base.join("deps").to_str().unwrap().to_owned(),
            node: base.join("node").to_str().unwrap().to_owned(),
            agent_bash_bin: base.join("agent-bash").to_str().unwrap().to_owned(),
            bash_allow: vec!["true".to_owned()],
            bash_authority: None,
            model: "claude-opus-5-5".to_owned(),
            effort: Effort::Medium,
            config_dir: "/nonexistent-claude-store".to_owned(),
            start_timeout_s: None,
            ack_timeout_s: None,
        }
    }

    #[test]
    fn launch_names_the_store_without_reading_it_and_removes_credential_names() {
        let base = scratch("launch");
        deps(&base);
        let mut setup = setup(&base);
        setup.bash_allow.clear();
        setup.bash_authority = Some(BashAuthority::TrustedTask);
        // The store does not exist: setup never looks at it.
        let launch = provision_claude(&setup, None).unwrap();
        let dir = base.join("launch");
        assert_eq!(launch.argv[0], "/usr/bin/env");
        for name in [
            "ANTHROPIC_API_KEY",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "ANTHROPIC_BASE_URL",
            "NODE_OPTIONS",
            "CLAUDE_CONFIG_DIR",
        ] {
            assert!(
                launch.argv.windows(2).any(|w| w[0] == "-u" && w[1] == name),
                "{name}: {:?}",
                launch.argv
            );
        }
        let tail = &launch.argv[launch.argv.len() - 3..];
        assert_eq!(tail[0], setup.node);
        assert_eq!(
            tail[1],
            dir.join("claude/acp-v2-receiver.mjs").to_str().unwrap()
        );
        assert_eq!(tail[2], dir.join("claude/launch.json").to_str().unwrap());
        let written: Value =
            serde_json::from_str(&fs::read_to_string(dir.join("claude/launch.json")).unwrap())
                .unwrap();
        assert_eq!(written, launch.launch);
        assert_eq!(
            written["claude_env"]["CLAUDE_CONFIG_DIR"],
            "/nonexistent-claude-store"
        );
        assert_eq!(written["claude_env"]["ENABLE_TOOL_SEARCH"], "false");
        assert_eq!(written["bash"], json!({ "authority": "trusted-task" }));
        assert_eq!(written["tools"], json!(["Read", "Write", "Edit"]));
        assert_eq!(written["model"], "claude-opus-5-5");
        assert_eq!(written["effort"], "medium");
        assert_eq!(written["start_timeout_s"], START_TIMEOUT_S);
        assert!(
            written["claude_executable"]
                .as_str()
                .unwrap()
                .ends_with(CLAUDE_EXECUTABLE)
        );
        assert_eq!(
            fs::read_to_string(dir.join("claude/acp-v2-receiver.mjs")).unwrap(),
            RECEIVER
        );
        assert_eq!(
            fs::read_link(dir.join("claude/node_modules")).unwrap(),
            base.join("deps/node_modules")
        );
        let receipt = launch.to_json();
        assert_eq!(receipt["endpoint"], "stdio");
        assert_eq!(receipt["policy"]["bash"], "trusted-task");
        assert_eq!(receipt["policy"]["permission_mode"], "dontAsk");
        assert!(!base.join("nonexistent-claude-store").exists());
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn allow_list_offers_no_builtin_tool_and_hand_over_keeps_code_read_only() {
        let base = scratch("allow");
        deps(&base);
        let launch = provision_claude(&setup(&base), None).unwrap();
        assert_eq!(launch.launch["bash"], json!({ "allow": ["true"] }));
        assert_eq!(launch.launch["tools"], json!([]));
        // SAFETY: getgid has no preconditions.
        let gid = unsafe { libc::getgid() };
        let identity = Identity {
            user: "self".into(),
            uid: unsafe { libc::getuid() },
            gid,
            groups: vec![gid],
        };
        let dir = base.join("launch");
        hand_over(&dir, &identity).unwrap();
        for (sub, expected) in [
            ("", 0o750),
            ("claude", 0o750),
            ("claude/acp-v2-receiver.mjs", 0o640),
            ("claude/launch.json", 0o640),
            ("claude/package.json", 0o640),
            ("claude/package-lock.json", 0o640),
        ] {
            let meta = fs::symlink_metadata(dir.join(sub)).unwrap();
            assert_eq!((meta.gid(), meta.mode() & 0o7777), (gid, expected), "{sub}");
        }
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn inputs_are_refused_before_any_write() {
        let base = scratch("refuse");
        deps(&base);
        let refused = |change: &dyn Fn(&mut ClaudeSetup), reason: &str| {
            let mut setup = setup(&base);
            change(&mut setup);
            match provision_claude(&setup, None) {
                Err(ClaudeSetupError::InputInvalid(why)) => {
                    assert!(why.contains(reason), "{why} / {reason}")
                }
                other => panic!("{reason}: {other:?}"),
            }
            assert!(!base.join("launch").exists(), "{reason}");
        };
        refused(&|s| s.bash_allow.clear(), "names no command");
        refused(&|s| s.bash_allow = vec![" ".to_owned()], "empty command");
        refused(
            &|s| s.bash_authority = Some(BashAuthority::TrustedTask),
            "exclusive",
        );
        refused(&|s| s.config_dir = "relative".to_owned(), "config_dir");
        refused(&|s| s.model = "opus 5".to_owned(), "model");
        refused(&|s| s.ack_timeout_s = Some(0), "ack_timeout_s");
        refused(&|s| s.node = "/nonexistent-node".to_owned(), "Node");
        fs::write(base.join("deps/package-lock.json"), "{}").unwrap();
        refused(&|_| {}, "different lockfile");
        fs::write(base.join("deps/package-lock.json"), LOCK).unwrap();
        fs::remove_file(base.join("deps").join(CLAUDE_EXECUTABLE)).unwrap();
        refused(&|_| {}, "no Claude Code executable");
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn effort_names_are_claude_codes() {
        for (text, effort) in [
            ("\"medium\"", Effort::Medium),
            ("\"high\"", Effort::High),
            ("\"xhigh\"", Effort::Xhigh),
        ] {
            assert_eq!(serde_json::from_str::<Effort>(text).unwrap(), effort);
            assert_eq!(format!("\"{}\"", effort.label()), text);
        }
        assert!(serde_json::from_str::<Effort>("\"Medium\"").is_err());
    }
}
