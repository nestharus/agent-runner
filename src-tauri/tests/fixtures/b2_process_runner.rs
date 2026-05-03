#![allow(dead_code)]

use agent_runner_lib::config::{
    ModelConfig, PromptMode, ProviderConfig, ProviderEntry, ProvidersConfig, ResumeKind,
    ResumeStrategy,
};
use agent_runner_lib::process::{
    CommandSpec, InteractiveCommandSpec, OutputSpec, ProcessOutput, ProcessRunner, StdinSpec,
};
use std::collections::{HashMap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Mutex, OnceLock};

#[derive(Default)]
pub struct FakeProcessRunner {
    calls: Mutex<Vec<CommandSpec>>,
    interactive_calls: Mutex<Vec<InteractiveCommandSpec>>,
    responses: Mutex<VecDeque<Result<ProcessOutput, String>>>,
    interactive_responses: Mutex<VecDeque<Result<i32, String>>>,
}

impl FakeProcessRunner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_response(&self, response: Result<ProcessOutput, String>) {
        self.responses.lock().unwrap().push_back(response);
    }

    pub fn push_stdout(&self, stdout: impl AsRef<[u8]>) {
        self.push_response(Ok(ProcessOutput {
            stdout: stdout.as_ref().to_vec(),
            stderr: Vec::new(),
            exit_code: 0,
            timed_out: false,
        }));
    }

    pub fn push_stderr_exit(&self, stderr: &str, exit_code: i32) {
        self.push_response(Ok(ProcessOutput {
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
            exit_code,
            timed_out: false,
        }));
    }

    pub fn push_error(&self, err: &str) {
        self.push_response(Err(err.to_string()));
    }

    pub fn push_interactive_response(&self, response: Result<i32, String>) {
        self.interactive_responses
            .lock()
            .unwrap()
            .push_back(response);
    }

    pub fn calls(&self) -> Vec<CommandSpec> {
        self.calls.lock().unwrap().clone()
    }

    pub fn interactive_calls(&self) -> Vec<InteractiveCommandSpec> {
        self.interactive_calls.lock().unwrap().clone()
    }

    pub fn only_call(&self) -> CommandSpec {
        let calls = self.calls();
        assert_eq!(
            calls.len(),
            1,
            "expected exactly one process call: {calls:?}"
        );
        calls[0].clone()
    }

    pub fn only_interactive_call(&self) -> InteractiveCommandSpec {
        let calls = self.interactive_calls();
        assert_eq!(
            calls.len(),
            1,
            "expected exactly one interactive process call: {calls:?}"
        );
        calls[0].clone()
    }
}

impl ProcessRunner for FakeProcessRunner {
    fn run(&self, spec: CommandSpec) -> Result<ProcessOutput, String> {
        self.calls.lock().unwrap().push(spec.clone());
        let response = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Ok(success_output(b"")));
        response.map(|mut output| {
            if spec.stdout == OutputSpec::Null {
                output.stdout.clear();
            }
            if spec.stderr == OutputSpec::Null {
                output.stderr.clear();
            }
            output
        })
    }

    fn run_interactive(&self, spec: InteractiveCommandSpec) -> Result<i32, String> {
        self.interactive_calls.lock().unwrap().push(spec);
        self.interactive_responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Ok(0))
    }
}

pub fn command_spec(program: &str, args: &[&str]) -> CommandSpec {
    CommandSpec {
        program: program.to_string(),
        args: args.iter().map(|arg| (*arg).to_string()).collect(),
        cwd: None,
        env: HashMap::new(),
        stdin: StdinSpec::Null,
        stdout: OutputSpec::Capture,
        stderr: OutputSpec::Capture,
        timeout: None,
        description: "fixture command".to_string(),
    }
}

pub fn success_output(stdout: &[u8]) -> ProcessOutput {
    ProcessOutput {
        stdout: stdout.to_vec(),
        stderr: Vec::new(),
        exit_code: 0,
        timed_out: false,
    }
}

pub fn output(stdout: &[u8], stderr: &[u8], exit_code: i32) -> ProcessOutput {
    ProcessOutput {
        stdout: stdout.to_vec(),
        stderr: stderr.to_vec(),
        exit_code,
        timed_out: false,
    }
}

pub fn model_with_provider(
    name: &str,
    prompt_mode: PromptMode,
    provider: ProviderConfig,
) -> ModelConfig {
    ModelConfig {
        name: name.to_string(),
        prompt_mode,
        providers: vec![provider],
        inputs: Vec::new(),
    }
}

pub fn provider(command: &str, args: &[&str]) -> ProviderConfig {
    ProviderConfig::new(
        command.to_string(),
        args.iter().map(|arg| (*arg).to_string()).collect(),
    )
}

pub fn interactive_provider(command: &str, interactive_args: &[&str]) -> ProviderConfig {
    ProviderConfig {
        name: command.to_string(),
        command: command.to_string(),
        args: Vec::new(),
        interactive_args: Some(
            interactive_args
                .iter()
                .map(|arg| (*arg).to_string())
                .collect(),
        ),
        resume: None,
        session_capture: None,
        resume_acceptance: None,
        session_storage: None,
    }
}

pub fn resumable_provider(command: &str) -> ProviderConfig {
    ProviderConfig {
        name: command.to_string(),
        command: command.to_string(),
        args: vec!["run".to_string()],
        interactive_args: Some(vec!["repl".to_string()]),
        resume: Some(ResumeStrategy {
            kind: ResumeKind::Flag,
            flag: Some("--resume".to_string()),
            subcommand: None,
        }),
        session_capture: None,
        resume_acceptance: None,
        session_storage: None,
    }
}

pub fn quota_providers_config(
    provider_name: &str,
    quota_script: Option<&str>,
    auth_refresh_command: Option<&str>,
) -> ProvidersConfig {
    let mut config = ProvidersConfig::default();
    config.entries.insert(
        provider_name.to_string(),
        ProviderEntry {
            quota_script: quota_script.map(ToOwned::to_owned),
            auth_refresh_command: auth_refresh_command.map(ToOwned::to_owned),
            ..ProviderEntry::default()
        },
    );
    config
}

pub fn isolated_process_env<F: FnOnce()>(body: F) {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let _guard = LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let previous_home = std::env::var_os("HOME");
    let previous_openai_key = std::env::var_os("OPENAI_API_KEY");
    unsafe {
        std::env::set_var("HOME", home.path());
        std::env::remove_var("OPENAI_API_KEY");
    }
    let result = catch_unwind(AssertUnwindSafe(body));
    unsafe {
        match previous_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
        match previous_openai_key {
            Some(value) => std::env::set_var("OPENAI_API_KEY", value),
            None => std::env::remove_var("OPENAI_API_KEY"),
        }
    }
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}
