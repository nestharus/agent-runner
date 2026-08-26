//! Internal child-process entrypoint for physical read-only snapshots.
//!
//! The startup hook lets every executable linked with `oulipoly-state` serve
//! the protocol before its application or test-harness main function runs.

use std::path::Path;

pub(crate) const MODE_ARG: &str = "__oulipoly-snapshot-helper";

#[ctor::ctor]
fn dispatch_process_mode() {
    if let Some(code) = run_from_env() {
        std::process::exit(code);
    }
}

fn run_from_env() -> Option<i32> {
    let mut args = std::env::args_os();
    let _program = args.next()?;
    if args.next()?.as_os_str() != MODE_ARG {
        return None;
    }
    let Some(source) = args.next() else {
        return Some(2);
    };
    let Some(destination) = args.next() else {
        return Some(2);
    };
    let Some(control) = args.next() else {
        return Some(2);
    };
    if args.next().is_some() {
        return Some(2);
    }
    Some(crate::read_only_snapshot::run_helper(
        Path::new(&source),
        Path::new(&destination),
        Path::new(&control),
    ))
}
