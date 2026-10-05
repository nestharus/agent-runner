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
//!       - native.toml presence as the ordinary-launch entry selection
//!       - model-name to site-route mapping for -m and agent frontmatter selection
//!       - refusal of launch forms the native entry does not support
//!       - one installed native caller invocation and its result rendering
//! ```
//!
//! When `<config root>/native.toml` exists, the ordinary launch forms
//! (`agents -m MODEL PROMPT`, `agents AGENT PROMPT`, `--agent-file`) run as
//! one installed native caller task (`oulipoly-native-call` through the
//! privileged front door) before any legacy owner bootstrap, State, runtime
//! services or provider registry. The selected model name, from `-m` or the
//! agent's frontmatter, must have an explicit `[models."NAME"]` mapping to a
//! site route. Anything else, including resume/REPL/provider pinning and
//! unmapped models, is refused: never a legacy launch, never a substitute
//! model. A resolved root without `native.toml` leaves the entry unselected;
//! an unresolved config root is refused, never treated as an absent file.
//!
//! The caller is one attempt, with no replay. Its exit code is returned as
//! is unless answer presentation fails after caller success (entry exit 6);
//! `answered` (0) is not task correctness.

use crate::usage::cli::{Cli, Subcommands};
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
    /// Site route name; the site, not this file, fixes provider/model/effort.
    route: String,
    /// `"trusted-task"`; or omit and give `bash_allow`.
    bash: Option<String>,
    bash_allow: Option<Vec<String>>,
    deadline_s: Option<u32>,
    credential_codex_profile: Option<PathBuf>,
    credential_opencode_auth: Option<PathBuf>,
    credential_provider: Option<String>,
    #[serde(default)]
    children: Vec<String>,
    child_max_starts: Option<u8>,
    child_max_concurrent: Option<u8>,
    child_credential_codex_profile: Option<PathBuf>,
}

/// What the ordinary command line selected, before mapping.
#[derive(Debug, PartialEq, Eq)]
enum Selection {
    Model(String),
    Agent,
}

/// Runs the ordinary launch on the native root when `native.toml` selects it.
/// `Ok(None)`: not selected (no file, or not a launch form); legacy dispatch
/// continues unchanged.
pub(crate) fn run_if_selected(cli: &Cli) -> Result<Option<i32>, String> {
    if !is_launch_form(cli) {
        return Ok(None);
    }
    let root = match crate::cli::paths::default_config_root() {
        Ok(root) => root,
        Err(reason) => {
            return refuse(&format!(
                "cannot determine native configuration selection: {reason}"
            ));
        }
    };
    let config = match selected_config(&root) {
        Ok(None) => return Ok(None),
        Ok(Some(config)) => config,
        Err(reason) => return refuse(&reason),
    };
    match run_selected(cli, &config) {
        Ok(code) => Ok(Some(code)),
        Err(reason) => refuse(&reason),
    }
}

/// The entry selection: `None` only when `native.toml` does not exist. An
/// unreadable or invalid file selects the native entry and refuses there.
fn selected_config(root: &Path) -> Result<Option<NativeEntryConfig>, String> {
    let path = root.join(CONFIG_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    parse_config(&text)
        .map(Some)
        .map_err(|error| format!("{}: {error}", path.display()))
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
    if model.credential_codex_profile.is_some() && model.credential_opencode_auth.is_some() {
        return Err("credential_codex_profile and credential_opencode_auth are exclusive".into());
    }
    if model.credential_opencode_auth.is_some() != model.credential_provider.is_some() {
        return Err("credential_opencode_auth and credential_provider go together".into());
    }
    if model.children.is_empty()
        && (model.child_max_starts.is_some()
            || model.child_max_concurrent.is_some()
            || model.child_credential_codex_profile.is_some())
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
            let raw = crate::cli::inputs::resolve_prompt(cli, false)?;
            let prompt = crate::cli::inputs::format_agent_prompt_with_inputs(&agent, raw, &inputs)?;
            (agent.model, prompt)
        }
    };
    let model = lookup(config, &model_name)?;
    let cwd = match &cli.project {
        Some(project) => std::path::absolute(project).map_err(|error| error.to_string())?,
        None => std::env::current_dir().map_err(|error| format!("cwd: {error}"))?,
    };
    launch(config, &model_name, model, &prompt, &cwd)
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
    if let Some(profile) = &model.credential_codex_profile {
        push("--credential-codex-profile", profile);
    }
    if let (Some(auth), Some(provider)) =
        (&model.credential_opencode_auth, &model.credential_provider)
    {
        push("--credential-opencode-auth", auth);
        push("--credential-provider", provider);
    }
    for route in &model.children {
        push("--child-route", route);
    }
    if let Some(limit) = model.child_max_starts {
        push("--child-max-starts", &limit.to_string());
    }
    if let Some(limit) = model.child_max_concurrent {
        push("--child-max-concurrent", &limit.to_string());
    }
    if let Some(profile) = &model.child_credential_codex_profile {
        push("--child-credential-codex-profile", profile);
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
    );
    eprintln!(
        "native entry: model '{model_name}' -> site route '{}' via {} (one attempt, no replay); out {}",
        model.route,
        config.caller.display(),
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
    report_to(out, status, &mut std::io::stdout().lock())
}

fn report_to(
    out: &Path,
    status: std::process::ExitStatus,
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
    eprintln!(
        "native entry: class {class}; caller exit {code}; front door exit {front_door}; records {} (answered is not correctness)",
        out.display()
    );
    if let Err(reason) = present_answer(out, stdout, code == 0 || class == "answered") {
        // Presentation is this entry's boundary, separate from caller custody.
        // Keep every non-successful caller code; never upgrade its outcome.
        let entry_code = if code == 0 { 6 } else { code };
        eprintln!("native entry: answer presentation failed: {reason}; entry exit {entry_code}");
        return Ok(entry_code);
    }
    Ok(code)
}

fn present_answer(out: &Path, stdout: &mut impl Write, required: bool) -> Result<(), String> {
    let path = out.join("final.md");
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
        assert!(!is_launch_form(&cli(&["--usage"])));
        assert!(!is_launch_form(&cli(&["migrate-db"])));
        assert!(is_launch_form(&cli(&["-m", "m", "hi"])));
    }

    #[test]
    fn only_an_absent_file_leaves_the_entry_unselected() {
        let dir = tempfile::tempdir().unwrap();
        assert!(selected_config(dir.path()).unwrap().is_none());
        std::fs::write(dir.path().join(CONFIG_FILE), "caller = 1\n").unwrap();
        assert!(selected_config(dir.path()).is_err());
        std::fs::write(
            dir.path().join(CONFIG_FILE),
            "caller = \"/c\"\nruns_dir = \"/r\"\n",
        )
        .unwrap();
        assert!(selected_config(dir.path()).unwrap().is_some());
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
        assert_eq!(report_to(dir.path(), status, &mut stdout).unwrap(), 6);
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
}
