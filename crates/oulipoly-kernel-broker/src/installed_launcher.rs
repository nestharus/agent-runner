//! Thin installed CLI/GUI entry. No Runner or State work occurs in this process.
#[cfg(target_os = "linux")]
fn main() {
    use oulipoly_kernel_broker::{
        installed_launch,
        installed_pair::{self, InstalledPair},
        protocol,
    };
    use std::os::fd::AsRawFd;
    use std::path::Path;

    let result = (|| -> std::io::Result<()> {
        let pair = InstalledPair::load(Path::new(installed_pair::MANIFEST), true)?;
        let digest = pair
            .launcher_sha256
            .as_deref()
            .ok_or_else(|| std::io::Error::other("installed pair lacks launcher image"))?;
        pair.verify_image(Path::new(installed_pair::LAUNCHER), digest, true)?;
        let captured = installed_launch::capture(
            &pair.generation,
            std::env::args_os().collect(),
            std::env::vars_os().collect(),
        )?;
        let descriptors: Vec<_> = captured
            .descriptors
            .iter()
            .map(AsRawFd::as_raw_fd)
            .collect();
        let response = protocol::submit_installed_launch_at(
            Path::new(protocol::INSTALLED_SOCKET),
            &captured.spec,
            &descriptors,
        )?;
        Err(std::io::Error::other(format!(
            "broker did not admit entry: {}",
            response.trim()
        )))
    })();
    if let Err(error) = result {
        eprintln!("OULIPOLY_INSTALLED_LAUNCH_GAP={error}");
        std::process::exit(70);
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("oulipoly-installed-launcher requires Linux");
    std::process::exit(1);
}
