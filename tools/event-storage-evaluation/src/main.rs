use age375_event_storage_evaluation::{EvalResult, EvaluationConfig, run_evaluation};
use std::path::PathBuf;

fn main() -> EvalResult<()> {
    let mut arguments = std::env::args().skip(1);
    let mut root = None;
    let mut output = None;
    let mut config = EvaluationConfig::default();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--root" => root = Some(PathBuf::from(required_value(&mut arguments, "--root")?)),
            "--output" => output = Some(PathBuf::from(required_value(&mut arguments, "--output")?)),
            "--records" => config.records = parse_value(&mut arguments, "--records")?,
            "--producers" => config.producers = parse_value(&mut arguments, "--producers")?,
            "--batch" => config.batch_size = parse_value(&mut arguments, "--batch")?,
            "--repetitions" => config.repetitions = parse_value(&mut arguments, "--repetitions")?,
            "--payload-bytes" => {
                config.payload_bytes = parse_value(&mut arguments, "--payload-bytes")?
            }
            "--help" | "-h" => {
                print_help();
                return Ok(());
            }
            unknown => return Err(format!("unknown argument: {unknown}").into()),
        }
    }
    let root = root.ok_or("--root is required; use a private disposable directory")?;
    let report = run_evaluation(&root, config)?;
    let rendered = serde_json::to_string_pretty(&report)?;
    if let Some(output) = output {
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(output, format!("{rendered}\n"))?;
    }
    println!("{rendered}");
    Ok(())
}

fn required_value(arguments: &mut impl Iterator<Item = String>, flag: &str) -> EvalResult<String> {
    arguments
        .next()
        .ok_or_else(|| format!("{flag} requires a value").into())
}

fn parse_value<T: std::str::FromStr>(
    arguments: &mut impl Iterator<Item = String>,
    flag: &str,
) -> EvalResult<T>
where
    T::Err: std::fmt::Display,
{
    let raw = required_value(arguments, flag)?;
    raw.parse()
        .map_err(|error| format!("invalid value for {flag}: {error}").into())
}

fn print_help() {
    println!(
        r#"AGE-375 bounded event-storage evaluator

Usage:
  cargo run --release --manifest-path tools/event-storage-evaluation/Cargo.toml -- \
    --root /private/disposable/root [--output results.json] \
    [--records 4096] [--producers 4] [--batch 32] \
    [--repetitions 5] [--payload-bytes 256]

The root must not already exist. It is retained for inspection."#
    );
}
