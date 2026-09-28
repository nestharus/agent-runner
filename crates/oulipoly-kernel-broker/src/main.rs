#[cfg(target_os = "linux")]
mod linux_main;

#[cfg(target_os = "linux")]
fn main() {
    let mut arguments = std::env::args_os().skip(1);
    if arguments.next().as_deref() == Some(std::ffi::OsStr::new("--bootstrap-empty-v30-state")) {
        if arguments.next().is_some() {
            eprintln!("empty v30 Broker State bootstrap takes no arguments");
            std::process::exit(2);
        }
        match oulipoly_state::mailbox::EmptyV30BootstrapIdentity::bootstrap_at(
            std::path::Path::new("/var/lib/oulipoly-kernel-broker"),
        ) {
            Ok(identity) => {
                println!(
                    "{}",
                    serde_json::to_string(&identity).expect("bootstrap identity JSON")
                );
                return;
            }
            Err(error) => {
                eprintln!("empty v30 Broker State bootstrap refused: {error}");
                std::process::exit(1);
            }
        }
    }
    linux_main::run();
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("oulipoly-kernel-broker requires Linux");
    std::process::exit(1);
}
