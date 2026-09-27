#[cfg(target_os = "linux")]
mod linux_main;

#[cfg(target_os = "linux")]
fn main() {
    linux_main::run();
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("oulipoly-kernel-broker requires Linux");
    std::process::exit(1);
}
