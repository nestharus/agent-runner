#![allow(dead_code)]

use agent_runner_lib::process::{
    CommandSpec, InteractiveCommandSpec, OutputSpec, ProcessOutput, ProcessRunner,
};
use std::collections::VecDeque;
use std::sync::Mutex;

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

    pub fn with_responses(responses: Vec<Result<ProcessOutput, String>>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
            ..Self::default()
        }
    }

    pub fn push_response(&self, response: Result<ProcessOutput, String>) {
        self.responses.lock().unwrap().push_back(response);
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

    pub fn single_call(&self) -> CommandSpec {
        let calls = self.calls();
        assert_eq!(calls.len(), 1, "expected exactly one process call");
        calls.into_iter().next().unwrap()
    }

    pub fn single_interactive_call(&self) -> InteractiveCommandSpec {
        let calls = self.interactive_calls();
        assert_eq!(calls.len(), 1, "expected exactly one interactive call");
        calls.into_iter().next().unwrap()
    }
}

impl ProcessRunner for FakeProcessRunner {
    fn run(&self, spec: CommandSpec) -> Result<ProcessOutput, String> {
        let stdout_mode = spec.stdout;
        let stderr_mode = spec.stderr;
        self.calls.lock().unwrap().push(spec);

        let mut output = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| ok_output("", "", 0))?;

        if stdout_mode == OutputSpec::Null {
            output.stdout.clear();
        }
        if stderr_mode == OutputSpec::Null {
            output.stderr.clear();
        }

        Ok(output)
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

pub fn ok_output(
    stdout: impl AsRef<[u8]>,
    stderr: impl AsRef<[u8]>,
    exit_code: i32,
) -> Result<ProcessOutput, String> {
    Ok(ProcessOutput {
        stdout: stdout.as_ref().to_vec(),
        stderr: stderr.as_ref().to_vec(),
        exit_code,
        timed_out: false,
    })
}

pub fn timeout_error(seconds: u64) -> Result<ProcessOutput, String> {
    Err(format!("process timed out after {seconds}s"))
}
