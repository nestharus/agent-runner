use oulipoly_runtime::repl_default_provider::{RuntimeServices, run_repl_with_default_provider};

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    if argv.len() > 1 {
        eprintln!(
            "error: 'agent' takes no arguments (received: {})",
            argv[1..].join(" ")
        );
        std::process::exit(2);
    }

    let services = match RuntimeServices::production(None) {
        Ok(services) => services,
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(2);
        }
    };

    let code = match run_repl_with_default_provider(services) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error}");
            2
        }
    };

    std::process::exit(code);
}
