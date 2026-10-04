//! Launch entry for one per-root supervisor process. See the library docs.

use std::io::{self, BufReader};
use std::process::ExitCode;

fn main() -> ExitCode {
    let input = BufReader::new(io::stdin());
    let code = oulipoly_root_supervisor::run(input, io::stdout().lock());
    ExitCode::from(code)
}
