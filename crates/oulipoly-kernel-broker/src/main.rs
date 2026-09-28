#[cfg(target_os = "linux")]
mod linux_main;

#[cfg(target_os = "linux")]
fn main() {
    let mut arguments = std::env::args_os().skip(1);
    if arguments.next().as_deref() == Some(std::ffi::OsStr::new("--activate-first-install-v30")) {
        if arguments.next().is_some() {
            eprintln!("first-install activation takes no arguments");
            std::process::exit(2);
        }
        use oulipoly_kernel_broker::first_install_activation::{FirstInstallActivation, PairPaths};
        use oulipoly_kernel_broker::installed_pair;
        let paths = PairPaths {
            manifest: std::path::Path::new(installed_pair::MANIFEST),
            runner: std::path::Path::new(installed_pair::RUNNER),
            broker: std::path::Path::new(installed_pair::BROKER),
            launcher: std::path::Path::new(installed_pair::LAUNCHER),
            bash: std::path::Path::new(installed_pair::BASH),
        };
        let pair =
            installed_pair::InstalledPair::load(paths.manifest, true).unwrap_or_else(|error| {
                eprintln!("first-install activation refused: {error}");
                std::process::exit(1);
            });
        let running_image = std::fs::File::open("/proc/self/exe").and_then(|current| {
            pair.verify_image_against(paths.broker, &pair.broker_sha256, true, &current)
        });
        if let Err(error) = running_image {
            eprintln!("first-install activation refused: {error}");
            std::process::exit(1);
        }
        match FirstInstallActivation::activate_at(
            std::path::Path::new("/var/lib/oulipoly-kernel-broker"),
            paths,
            false,
        ) {
            Ok(identity) => {
                println!(
                    "{}",
                    serde_json::to_string(&identity).expect("activation JSON")
                );
                return;
            }
            Err(error) => {
                eprintln!("first-install activation refused: {error}");
                std::process::exit(1);
            }
        }
    }
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
