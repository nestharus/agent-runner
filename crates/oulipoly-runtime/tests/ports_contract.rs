use oulipoly_runtime::ports::{
    Clock, DefaultProcessRunner, DefaultUuidGenerator, ProcessRunner, ProcessSpec, StderrSink,
    StdoutSink, SystemClock, UuidGenerator,
};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

#[test]
fn system_clock_now_tracks_std_system_time_with_small_tolerance() {
    let clock = SystemClock;
    let before = SystemTime::now();
    let observed = <SystemClock as Clock>::now(&clock);
    let after = SystemTime::now();

    assert!(
        observed.duration_since(before).unwrap_or(Duration::ZERO) <= Duration::from_secs(5),
        "clock was more than 5s after the pre-call timestamp"
    );
    assert!(
        after.duration_since(observed).unwrap_or(Duration::ZERO) <= Duration::from_secs(5),
        "clock was more than 5s before the post-call timestamp"
    );
}

#[test]
fn default_uuid_generator_produces_unique_v4_values() {
    let generator = DefaultUuidGenerator;
    let mut values = std::collections::HashSet::new();

    for _ in 0..128 {
        let value = <DefaultUuidGenerator as UuidGenerator>::new_v4(&generator);
        assert_eq!(value.get_version_num(), 4);
        assert!(values.insert(value), "duplicate UUID generated: {value}");
    }
}

#[test]
fn default_process_runner_runs_echo_and_captures_stdout() {
    let runner = DefaultProcessRunner;
    let outcome = <DefaultProcessRunner as ProcessRunner>::run(
        &runner,
        ProcessSpec {
            program: "echo".to_string(),
            args: vec!["hello".to_string()],
            current_dir: None,
            env: Vec::new(),
            stdin: None,
        },
    )
    .unwrap();

    assert_eq!(outcome.exit_code, 0);
    assert_eq!(outcome.stdout, b"hello\n");
    assert!(outcome.stderr.is_empty());
}

#[derive(Clone, Default)]
struct BufferSink {
    bytes: Arc<Mutex<Vec<u8>>>,
}

impl StdoutSink for BufferSink {
    fn write_line(&self, line: &str) -> Result<(), String> {
        let mut bytes = self.bytes.lock().unwrap();
        bytes.extend_from_slice(line.as_bytes());
        bytes.push(b'\n');
        Ok(())
    }
}

impl StderrSink for BufferSink {
    fn write_bytes(&self, bytes: &[u8]) -> Result<(), String> {
        self.bytes.lock().unwrap().extend_from_slice(bytes);
        Ok(())
    }
}

#[test]
fn output_sink_traits_are_buffer_implementable_without_polluting_stdio() {
    let sink = BufferSink::default();
    let stdout: &dyn StdoutSink = &sink;
    let stderr: &dyn StderrSink = &sink;

    stdout.write_line("hello").unwrap();
    stderr.write_bytes(b"error bytes").unwrap();

    assert_eq!(&*sink.bytes.lock().unwrap(), b"hello\nerror bytes");
}

#[ignore = "production stdout/stderr smoke writes to real process output; buffer-capable production sinks can unignore this"]
#[test]
fn production_stdout_and_stderr_sinks_are_intentionally_not_exercised_here() {}
