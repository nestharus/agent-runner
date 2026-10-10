//! Ordinary `agents` launch on the per-root ACP v2 native root (Linux).
//!
//! ## Declared roles
//!
//! `orchestration`, `parser`, `validator`, `mapper`, `formatter`
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: src-tauri/src/commands/native_entry.rs
//!     role: intrinsic-surface
//!     Domain: native_entry_cli_selection
//!     Owns:
//!       - native.toml requirement for ordinary Linux CLI launch
//!       - model-name to site-route mapping for -m and agent frontmatter selection
//!       - refusal of launch forms the native entry does not support
//!       - one installed native caller invocation and its result rendering
//!       - observational requester root discovery through the configured caller
//!       - opt-in live opening and handle-addressed live root controls
//! ```
//!
//! The ordinary Linux CLI launch forms require `<config root>/native.toml`.
//! With valid configuration, those forms
//! (`agents -m MODEL PROMPT`, `agents AGENT PROMPT`, `--agent-file`) run as
//! one installed native caller task (`oulipoly-native-call` through the
//! privileged front door) before any legacy owner bootstrap, State, runtime
//! services or provider registry. The selected model name, from `-m` or the
//! agent's frontmatter, must have an explicit `[models."NAME"]` mapping to a
//! site route. Anything else, including resume/REPL/provider pinning and
//! unmapped models, is refused: never a legacy launch, never a substitute
//! model. Missing, unreadable or invalid configuration, and an unresolved
//! config root, are refused before legacy bootstrap.
//!
//! Parent/child authentication stays with the external registered adapters;
//! this configuration and caller have no credential-source fields.
//! The caller is one attempt, with no replay. Its exit code is returned as
//! is unless answer presentation fails after caller success (entry exit 6);
//! `answered` (0) is not task correctness.
//! `agents roots` uses the same configured caller's observational discovery
//! before legacy initialization, with no model, prompt or run allocation.
//!
//! `--live-handle NEWFILE` on a launch form is the explicit opt-in to an
//! ongoing root: the caller writes the private handle (mode 0600) and the
//! root outlives this call. `agents root FILE <control>` hands that file to
//! the same caller's `--root` operations. This entry never reads the handle
//! or prints its contents; the caller and front door keep the UID, peer,
//! token and exact v3 authority checks. Without the opt-in a launch stays
//! one-shot. One caller attempt per control, no replay.

use crate::usage::cli::{Cli, RootControl, Subcommands};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) const CONFIG_FILE: &str = "native.toml";
const DEFAULT_DEADLINE_S: u32 = 1800;
const MAX_DEADLINE_S: u32 = 7200;
/// Refusal before anything is started (mirrors the caller's own local refusal).
const REFUSED: i32 = 3;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeEntryConfig {
    /// Absolute path of the installed, versioned `oulipoly-native-call`.
    caller: PathBuf,
    /// Absolute directory for prompt files and per-run caller output.
    runs_dir: PathBuf,
    deadline_s: Option<u32>,
    #[serde(default)]
    models: BTreeMap<String, NativeModel>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeModel {
    /// Registered site route name; adapter settings are selected there.
    route: String,
    /// `"trusted-task"`; or omit and give `bash_allow`.
    bash: Option<String>,
    bash_allow: Option<Vec<String>>,
    deadline_s: Option<u32>,
    #[serde(default)]
    children: Vec<String>,
    child_max_starts: Option<u8>,
    child_max_concurrent: Option<u8>,
}

/// What the ordinary command line selected, before mapping.
#[derive(Debug, PartialEq, Eq)]
enum Selection {
    Model(String),
    Agent,
}

/// Runs ordinary Linux CLI launches on the configured native root or refuses.
/// `Ok(None)` is reserved for help/usage and non-launch forms.
pub(crate) fn run_if_selected(cli: &Cli) -> Result<Option<i32>, String> {
    run_if_selected_with_root(cli, crate::cli::paths::default_config_root)
}

fn run_if_selected_with_root(
    cli: &Cli,
    config_root: impl FnOnce() -> Result<PathBuf, String>,
) -> Result<Option<i32>, String> {
    let discovery = matches!(cli.command, Some(Subcommands::Roots));
    let control = matches!(cli.command, Some(Subcommands::Root { .. }));
    if let Some(Subcommands::Root {
        control,
        control_prior: Some(_),
        ..
    }) = &cli.command
        && control.control_request.is_none()
    {
        return refuse("--control-prior needs --control-request");
    }
    if !discovery && !control && !is_launch_form(cli) {
        if cli.live_handle.is_some() {
            return refuse("--live-handle needs a native launch form");
        }
        return Ok(None);
    }
    // Private live opening, discovery and controls never quote settings.
    let private = discovery || control || cli.live_handle.is_some();
    let root = match config_root() {
        Ok(root) => root,
        Err(_) if private => {
            return refuse("cannot determine native discovery/control configuration");
        }
        Err(reason) => {
            return refuse(&format!(
                "cannot determine native configuration selection: {reason}"
            ));
        }
    };
    let config = match selected_config(&root) {
        Ok(config) => config,
        // Resolver and TOML diagnostics may quote private configuration.
        Err(_) if private => {
            return refuse("native discovery/control configuration unavailable or invalid");
        }
        Err(reason) => return refuse(&reason),
    };
    let result = if discovery {
        discover(&config)
    } else if let Some(Subcommands::Root {
        handle,
        control,
        control_prior,
        wait,
    }) = &cli.command
    {
        root_control(&config, handle, control, control_prior.as_deref(), *wait)
    } else {
        run_selected(cli, &config)
    };
    match result {
        Ok(code) => Ok(Some(code)),
        Err(reason) => refuse(&reason),
    }
}

/// Launch requires valid native configuration; every read/parse error refuses.
fn selected_config(root: &Path) -> Result<NativeEntryConfig, String> {
    let path = root.join(CONFIG_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!(
                "native launch is not configured: {} is missing; configure the installed native caller and model routes",
                path.display()
            ));
        }
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    parse_config(&text).map_err(|error| format!("{}: {error}", path.display()))
}

fn discover(config: &NativeEntryConfig) -> Result<i32, String> {
    let status = Command::new(&config.caller)
        .arg("--discover")
        .stdin(std::process::Stdio::null())
        .status()
        .map_err(|error| format!("native discovery caller unavailable: {}", error.kind()))?;
    Ok(status.code().unwrap_or(6))
}

/// Caller `--root` argv for one control, and whether its result is printed.
fn root_argv(
    handle: &Path,
    control: &RootControl,
    prior: Option<&Path>,
    wait: Option<u32>,
    out: &Path,
) -> (Vec<std::ffi::OsString>, bool) {
    let mut argv: Vec<std::ffi::OsString> =
        vec!["--root".into(), handle.into(), "--out".into(), out.into()];
    let flag = [
        (control.inspect, "--inspect"),
        (control.hold, "--hold"),
        (control.release, "--release"),
        (control.cancel, "--cancel"),
        (control.close, "--close"),
        (control.stop, "--stop"),
    ]
    .into_iter()
    .find_map(|(set, flag)| set.then_some(flag));
    let mut face = flag.is_some_and(|flag| flag != "--close" && flag != "--stop");
    if let Some(flag) = flag {
        argv.push(flag.into());
    } else if let Some(file) = &control.prompt_file {
        argv.extend(["--prompt-file".into(), file.into()]);
    } else if let Some(file) = &control.control_request {
        argv.extend(["--control-request".into(), file.into()]);
        face = true;
    }
    if let Some(prior) = prior {
        argv.extend(["--control-prior".into(), prior.into()]);
    }
    if let Some(wait) = wait {
        argv.extend(["--wait".into(), wait.to_string().into()]);
    }
    (argv, face)
}

fn root_control(
    config: &NativeEntryConfig,
    handle: &Path,
    control: &RootControl,
    prior: Option<&Path>,
    wait: Option<u32>,
) -> Result<i32, String> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&config.runs_dir)
        .map_err(|error| format!("runs_dir: {}", error.kind()))?;
    let out = config.runs_dir.join(run_id());
    let (argv, face) = root_argv(handle, control, prior, wait, &out);
    let previous = unsafe { libc::signal(libc::SIGINT, libc::SIG_IGN) };
    let status = Command::new(&config.caller)
        .args(&argv)
        .stdin(std::process::Stdio::null())
        .status();
    unsafe { libc::signal(libc::SIGINT, previous) };
    let status =
        status.map_err(|error| format!("native root caller unavailable: {}", error.kind()))?;
    let mut stdout = std::io::stdout().lock();
    if face {
        // The control-face record is private requester metadata.
        return report_to(&out, status, Answer::Record, &mut stdout);
    }
    let answer = if control.prompt_file.is_some() {
        Answer::Final
    } else {
        Answer::None
    };
    report_to(&out, status, answer, &mut stdout)
}

fn refuse(reason: &str) -> Result<Option<i32>, String> {
    eprintln!("native entry refused: {reason} (nothing launched; no legacy launch)");
    Ok(Some(REFUSED))
}

/// Ordinary provider-launch forms. Non-launch subcommands keep their own paths.
fn is_launch_form(cli: &Cli) -> bool {
    !cli.usage
        && matches!(
            cli.command,
            None | Some(Subcommands::Repl { .. }) | Some(Subcommands::Resume { .. })
        )
}

fn parse_config(text: &str) -> Result<NativeEntryConfig, String> {
    let config: NativeEntryConfig = toml::from_str(text).map_err(|error| error.to_string())?;
    if !config.caller.is_absolute() {
        return Err("caller must be an absolute path".into());
    }
    if !config.runs_dir.is_absolute() {
        return Err("runs_dir must be an absolute path".into());
    }
    check_deadline(config.deadline_s)?;
    for (name, model) in &config.models {
        check_model(model).map_err(|error| format!("models.\"{name}\": {error}"))?;
    }
    Ok(config)
}

fn check_deadline(deadline: Option<u32>) -> Result<(), String> {
    match deadline {
        Some(value) if !(1..=MAX_DEADLINE_S).contains(&value) => {
            Err(format!("deadline_s must be 1..{MAX_DEADLINE_S}"))
        }
        _ => Ok(()),
    }
}

fn check_model(model: &NativeModel) -> Result<(), String> {
    if model.route.trim().is_empty() {
        return Err("route is empty".into());
    }
    match (&model.bash, &model.bash_allow) {
        (Some(bash), None) if bash == "trusted-task" => {}
        (Some(_), None) => return Err("bash must be \"trusted-task\" (or use bash_allow)".into()),
        (None, Some(_)) => {}
        _ => return Err("exactly one of bash = \"trusted-task\" or bash_allow is required".into()),
    }
    check_deadline(model.deadline_s)?;
    if model.children.is_empty()
        && (model.child_max_starts.is_some() || model.child_max_concurrent.is_some())
    {
        return Err("child_* options need children".into());
    }
    for limit in [model.child_max_starts, model.child_max_concurrent]
        .into_iter()
        .flatten()
    {
        if !(1..=4).contains(&limit) {
            return Err("child limits must be 1..4".into());
        }
    }
    Ok(())
}

/// Refuses launch forms that have no native realization yet.
fn unsupported_form(cli: &Cli) -> Option<&'static str> {
    match &cli.command {
        Some(Subcommands::Repl { .. }) => return Some("repl"),
        Some(Subcommands::Resume { .. }) => return Some("resume"),
        _ => {}
    }
    if cli.resume.is_some() {
        Some("--resume")
    } else if cli.new {
        Some("--new")
    } else if cli.rotate_provider.is_some() {
        Some("--rotate-provider")
    } else if cli.pin_provider.is_some() {
        Some("--pin-provider")
    } else if cli.fresh_continuation_request.is_some() {
        Some("--fresh-continuation-request")
    } else if cli.submission_token.is_some() {
        Some("--submission-token")
    } else if cli.model.is_some() && !cli.inputs.is_empty() {
        // Model inputs map to flags of a wrapped CLI; no native meaning.
        Some("-i/--input with -m")
    } else {
        None
    }
}

fn selection(cli: &Cli) -> Selection {
    match &cli.model {
        Some(model) => Selection::Model(model.clone()),
        None => Selection::Agent,
    }
}

fn run_selected(cli: &Cli, config: &NativeEntryConfig) -> Result<i32, String> {
    if let Some(form) = unsupported_form(cli) {
        return Err(format!("{form} is not supported on the native entry"));
    }
    // Map before reading any prompt: an unmapped selection is refused first.
    let (model_name, prompt) = match selection(cli) {
        Selection::Model(name) => {
            lookup(config, &name)?;
            (name, direct_model_prompt(cli)?)
        }
        Selection::Agent => {
            let agent = crate::agent_resolution::resolve_agent(
                cli,
                &oulipoly_config::repositories::FilesystemAgentConfigRepository,
            )?;
            lookup(config, &agent.model)
                .map_err(|error| format!("{error} (selected by agent '{}')", agent.name))?;
            let inputs = crate::cli::inputs::parse_inputs(&cli.inputs)?;
            // With -a the file already selects the agent, so Clap's first
            // positional `agent` slot is part of the prompt. Named-agent
            // invocation consumes that slot as its selector instead.
            let raw = crate::cli::inputs::resolve_prompt(cli, cli.agent_file.is_some())?;
            let prompt = crate::cli::inputs::format_agent_prompt_with_inputs(&agent, raw, &inputs)?;
            (agent.model, prompt)
        }
    };
    let model = lookup(config, &model_name)?;
    let cwd = match &cli.project {
        Some(project) => std::path::absolute(project).map_err(|error| error.to_string())?,
        None => std::env::current_dir().map_err(|error| format!("cwd: {error}"))?,
    };
    let live = match &cli.live_handle {
        Some(path) => Some(std::path::absolute(path).map_err(|error| error.to_string())?),
        None => None,
    };
    launch(config, &model_name, model, &prompt, &cwd, live.as_deref())
}

fn lookup<'a>(config: &'a NativeEntryConfig, name: &str) -> Result<&'a NativeModel, String> {
    config.models.get(name).ok_or_else(|| {
        format!("model '{name}' has no native route mapping in {CONFIG_FILE}; no substitute model")
    })
}

fn direct_model_prompt(cli: &Cli) -> Result<String, String> {
    let raw = crate::cli::inputs::resolve_prompt(cli, true)?;
    match &cli.agent_file {
        Some(path) => {
            let agent = oulipoly_config::load_agent_file(path)?;
            Ok(crate::cli::inputs::format_agent_prompt(&agent, raw))
        }
        None => Ok(raw),
    }
}

fn run_id() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!(
        "run-{}-{:09}-{}",
        now.as_secs(),
        now.subsec_nanos(),
        std::process::id()
    )
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    file.write_all(bytes)
        .map_err(|error| format!("{}: {error}", path.display()))
}

fn caller_argv(
    config: &NativeEntryConfig,
    model: &NativeModel,
    prompt_file: &Path,
    allow_file: Option<&Path>,
    cwd: &Path,
    out: &Path,
    live: Option<&Path>,
) -> Vec<std::ffi::OsString> {
    let mut argv: Vec<std::ffi::OsString> = Vec::new();
    let mut push = |flag: &str, value: &dyn AsRef<std::ffi::OsStr>| {
        argv.push(flag.into());
        argv.push(value.as_ref().to_owned());
    };
    push("--route", &model.route);
    push("--prompt-file", &prompt_file);
    push("--cwd", &cwd);
    push("--out", &out);
    let deadline = model
        .deadline_s
        .or(config.deadline_s)
        .unwrap_or(DEFAULT_DEADLINE_S)
        .to_string();
    push("--deadline", &deadline);
    for route in &model.children {
        push("--child-route", route);
    }
    if let Some(limit) = model.child_max_starts {
        push("--child-max-starts", &limit.to_string());
    }
    if let Some(limit) = model.child_max_concurrent {
        push("--child-max-concurrent", &limit.to_string());
    }
    if let Some(handle) = live {
        push("--live-handle", &handle);
    }
    match allow_file {
        Some(file) => push("--allow-file", &file),
        None => argv.push("--trusted-task".into()),
    }
    argv
}

fn launch(
    config: &NativeEntryConfig,
    model_name: &str,
    model: &NativeModel,
    prompt: &str,
    cwd: &Path,
    live: Option<&Path>,
) -> Result<i32, String> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&config.runs_dir)
        .map_err(|error| format!("runs_dir {}: {error}", config.runs_dir.display()))?;
    let id = run_id();
    let out = config.runs_dir.join(&id);
    let prompt_file = config.runs_dir.join(format!("{id}.prompt.md"));
    write_private(&prompt_file, prompt.as_bytes())?;
    let allow_file = match &model.bash_allow {
        Some(allow) => {
            let path = config.runs_dir.join(format!("{id}.allow.json"));
            let body = serde_json::to_vec(allow).map_err(|error| error.to_string())?;
            write_private(&path, &body)?;
            Some(path)
        }
        None => None,
    };
    let argv = caller_argv(
        config,
        model,
        &prompt_file,
        allow_file.as_deref(),
        cwd,
        &out,
        live,
    );
    eprintln!(
        "native entry: model '{model_name}' -> site route '{}' via {} (one attempt, no replay{}); out {}",
        model.route,
        config.caller.display(),
        if live.is_some() {
            "; live root requested"
        } else {
            ""
        },
        out.display()
    );
    // A terminal Ctrl-C reaches the caller in the same foreground group; it
    // sends cancel and collects. This process waits for it rather than
    // exiting first and hiding the caller's outcome.
    let previous = unsafe { libc::signal(libc::SIGINT, libc::SIG_IGN) };
    let status = Command::new(&config.caller).args(&argv).status();
    unsafe { libc::signal(libc::SIGINT, previous) };
    let status = status
        .map_err(|error| format!("caller {} did not start: {error}", config.caller.display()))?;
    report(&out, status)
}

fn report(out: &Path, status: std::process::ExitStatus) -> Result<i32, String> {
    report_to(out, status, Answer::Final, &mut std::io::stdout().lock())
}

/// What a caller call presents on stdout.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Answer {
    /// `final.md`, required after success.
    Final,
    /// The caller's `result.json` control record, required.
    Record,
    /// Nothing; the status line and record directory only.
    None,
}

fn report_to(
    out: &Path,
    status: std::process::ExitStatus,
    answer: Answer,
    stdout: &mut impl Write,
) -> Result<i32, String> {
    use std::os::unix::process::ExitStatusExt;
    let result: Option<serde_json::Value> = std::fs::read(out.join("result.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    let class = result
        .as_ref()
        .and_then(|value| value.get("class"))
        .and_then(|value| value.as_str())
        .unwrap_or("unknown (no result.json)");
    let front_door = result
        .as_ref()
        .and_then(|value| value.get("front_door_exit"))
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unknown".into());
    let code = match (status.code(), status.signal()) {
        (Some(code), _) => code,
        (None, Some(signal)) => 128 + signal,
        (None, None) => 1,
    };
    // The caller's own reading of whether the root lives on after this call.
    let root = result
        .as_ref()
        .and_then(|value| value.get("root"))
        .and_then(|value| value.as_str())
        .map(|root| format!("; root {root}"))
        .unwrap_or_default();
    eprintln!(
        "native entry: class {class}; caller exit {code}; front door exit {front_door}{root}; records {} (answered is not correctness)",
        out.display()
    );
    let presented = match answer {
        Answer::Final => present_answer(out, "final.md", stdout, code == 0 || class == "answered"),
        Answer::Record => present_answer(out, "result.json", stdout, true),
        Answer::None => Ok(()),
    };
    if let Err(reason) = presented {
        // Presentation is this entry's boundary, separate from caller custody.
        // Keep every non-successful caller code; never upgrade its outcome.
        let entry_code = if code == 0 { 6 } else { code };
        eprintln!("native entry: answer presentation failed: {reason}; entry exit {entry_code}");
        return Ok(entry_code);
    }
    Ok(code)
}

fn present_answer(
    out: &Path,
    name: &str,
    stdout: &mut impl Write,
    required: bool,
) -> Result<(), String> {
    let path = out.join(name);
    let answer = match std::fs::read_to_string(&path) {
        Ok(answer) => answer,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !required => return Ok(()),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    stdout
        .write_all(answer.as_bytes())
        .map_err(|error| format!("cannot write answer to stdout: {error}"))?;
    if !answer.ends_with('\n') {
        stdout
            .write_all(b"\n")
            .map_err(|error| format!("cannot write answer newline to stdout: {error}"))?;
    }
    stdout
        .flush()
        .map_err(|error| format!("cannot flush answer to stdout: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn cli(args: &[&str]) -> Cli {
        let mut argv = vec!["agents"];
        argv.extend_from_slice(args);
        Cli::try_parse_from(argv).unwrap()
    }

    #[test]
    fn unsupported_launch_forms_are_named() {
        assert_eq!(
            unsupported_form(&cli(&["--resume", "x", "hi"])),
            Some("--resume")
        );
        assert_eq!(
            unsupported_form(&cli(&["-m", "m", "--pin-provider", "p", "hi"])),
            Some("--pin-provider")
        );
        assert_eq!(
            unsupported_form(&cli(&["-m", "m", "-i", "k=v", "hi"])),
            Some("-i/--input with -m")
        );
        assert_eq!(unsupported_form(&cli(&["repl", "m"])), Some("repl"));
        assert_eq!(unsupported_form(&cli(&["-m", "m", "hi"])), None);
        assert_eq!(unsupported_form(&cli(&["agent", "-i", "k=v", "hi"])), None);
    }

    #[test]
    fn non_launch_subcommands_are_not_selected() {
        for args in [&["--usage"][..], &["migrate-db"][..]] {
            let cli = cli(args);
            assert!(!is_launch_form(&cli));
            assert_eq!(
                run_if_selected_with_root(&cli, || panic!("non-launch must not read config"))
                    .unwrap(),
                None
            );
        }
        assert!(is_launch_form(&cli(&["-m", "m", "hi"])));
    }

    #[test]
    fn missing_unreadable_and_invalid_config_refuse_launch_without_fallthrough() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CONFIG_FILE);
        let launch_forms = [
            cli(&["-m", "m", "hi"]),
            cli(&["agent", "hi"]),
            cli(&["--agent-file", "absent-agent.md", "hi"]),
            cli(&["--resume", "x", "hi"]),
            cli(&["repl", "m"]),
            cli(&["resume", "x"]),
        ];
        let assert_refused = || {
            for cli in &launch_forms {
                assert_eq!(
                    run_if_selected_with_root(cli, || Ok(dir.path().to_owned())).unwrap(),
                    Some(REFUSED)
                );
            }
        };
        let reason = selected_config(dir.path()).unwrap_err();
        assert!(reason.contains("native launch is not configured"));
        assert!(reason.contains(&path.display().to_string()));
        assert_refused();
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);

        // A directory at the config path is unreadable as a TOML file even
        // under a privileged test runner (unlike a mode-000 fixture).
        std::fs::create_dir(&path).unwrap();
        assert!(
            selected_config(dir.path())
                .unwrap_err()
                .contains("cannot read")
        );
        assert_refused();
        std::fs::remove_dir(&path).unwrap();
        std::fs::write(&path, "caller = 1\n").unwrap();
        assert!(selected_config(dir.path()).is_err());
        assert_refused();
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);

        std::fs::write(&path, "caller = \"/c\"\nruns_dir = \"/r\"\n").unwrap();
        let configured = selected_config(dir.path()).unwrap();
        assert_eq!(configured.caller, Path::new("/c"));
        assert_eq!(configured.runs_dir, Path::new("/r"));
        // Valid configuration still refuses unsupported and unmapped launches
        // before prompt/agent reads or any caller invocation.
        for cli in [
            cli(&["-m", "m", "hi"]),
            cli(&["--resume", "x", "hi"]),
            cli(&["repl", "m"]),
            cli(&["resume", "x"]),
        ] {
            assert_eq!(
                run_if_selected_with_root(&cli, || Ok(dir.path().to_owned())).unwrap(),
                Some(REFUSED)
            );
        }
        for cli in &launch_forms {
            assert_eq!(
                run_if_selected_with_root(cli, || Err("fixture root unavailable".into())).unwrap(),
                Some(REFUSED)
            );
        }
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn config_requires_explicit_bash_policy_and_absolute_paths() {
        let base = "caller = \"/c\"\nruns_dir = \"/r\"\n";
        assert!(parse_config(&format!("{base}[models.m]\nroute = \"sol-high\"\n")).is_err());
        assert!(
            parse_config(&format!(
                "{base}[models.m]\nroute = \"sol-high\"\nbash = \"all\"\n"
            ))
            .is_err()
        );
        assert!(parse_config("caller = \"c\"\nruns_dir = \"/r\"\n").is_err());
        assert!(parse_config(&format!("{base}surprise = 1\n")).is_err());
        assert!(
            parse_config(&format!(
                "{base}[models.m]\nroute = \"r\"\nbash = \"trusted-task\"\nchild_max_starts = 2\n"
            ))
            .is_err()
        );
        assert!(
            parse_config(&format!(
                "{base}[models.m]\nroute = \"r\"\nbash_allow = [\"ls\"]\n"
            ))
            .is_ok()
        );
    }

    #[test]
    fn buffered_stdout_flush_loss_returns_failure_without_changing_caller_records() {
        // Small writes fit in the real buffer; /dev/full fails only when flush
        // sends the bytes to the OS. No synthetic always-error writer.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("final.md"), "retained answer").unwrap();
        let result = b"{\"class\":\"answered\",\"front_door_exit\":87}";
        std::fs::write(dir.path().join("result.json"), result).unwrap();
        let status = Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .status()
            .unwrap();
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/full")
            .unwrap();
        let mut stdout = std::io::BufWriter::with_capacity(4096, file);
        assert_eq!(
            report_to(dir.path(), status, Answer::Final, &mut stdout).unwrap(),
            6
        );
        assert_eq!(stdout.buffer(), b"retained answer\n");
        assert_eq!(
            std::fs::read(dir.path().join("result.json")).unwrap(),
            result
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("final.md")).unwrap(),
            "retained answer"
        );
    }
    #[test]
    fn embedded_credential_configuration_is_an_unknown_shape() {
        for key in [
            "credential_codex_profile",
            "credential_opencode_auth",
            "credential_provider",
            "child_credential_codex_profile",
        ] {
            let text = format!(
                r#"caller = "/opt/package/bin/oulipoly-native-call"
runs_dir = "/tmp/caller-records"
[models.example]
route = "parent"
bash = "trusted-task"
{key} = "/unread-credential"
"#
            );
            assert!(
                parse_config(&text).unwrap_err().contains("unknown field"),
                "{key}"
            );
        }
    }
}
