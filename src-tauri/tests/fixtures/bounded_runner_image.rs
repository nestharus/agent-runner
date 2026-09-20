//! Linux paired tests execute a private debug-stripped image, never mutate Cargo's
//! report/debug artifact. LLVM coverage sections and instrumentation are not debug
//! information; keep the original object for cargo-llvm-cov report discovery.
use std::path::Path;
use std::sync::OnceLock;

pub fn runner_bin() -> &'static str {
    static IMAGE: OnceLock<(tempfile::TempDir, String)> = OnceLock::new();
    if !cfg!(target_os = "linux") {
        return env!("CARGO_BIN_EXE_oulipoly-agent-runner");
    }
    &IMAGE
        .get_or_init(|| {
            let source = std::env::var_os("AGE360_RUNNER_BIN")
                .unwrap_or_else(|| env!("CARGO_BIN_EXE_oulipoly-agent-runner").into());
            let directory = tempfile::tempdir().unwrap();
            let destination = directory.path().join("oulipoly-agent-runner");
            provision(Path::new(&source), &destination);
            (directory, destination.to_str().unwrap().to_owned())
        })
        .1
}

pub fn provision(source: &Path, destination: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let original = std::fs::read(source).expect("read exact Cargo/candidate image");
    assert!(
        original.starts_with(b"\x7fELF"),
        "Linux test image must be ELF"
    );
    assert!(std::fs::metadata(source).unwrap().permissions().mode() & 0o111 != 0);
    // create_new precludes aliasing/truncating the Cargo source or another image.
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .unwrap();
    std::io::Write::write_all(&mut output, &original).unwrap();
    drop(output);
    std::fs::set_permissions(destination, std::fs::Permissions::from_mode(0o700)).unwrap();
    let stripped = std::process::Command::new("/usr/bin/strip")
        .arg("--strip-debug")
        .arg(destination)
        .output()
        .expect("GNU debug-only strip required");
    assert!(
        stripped.status.success(),
        "debug-only strip failed: {stripped:?}"
    );
    let image = std::fs::read(destination).unwrap();
    assert!(
        image.len() <= 256 * 1024 * 1024,
        "paired helper exceeds unchanged 256MiB budget"
    );
    assert_eq!(
        std::fs::read(source).unwrap(),
        original,
        "source image changed while provisioning"
    );
    println!(
        "paired image source={} bytes={} sha256={} transform=/usr/bin/strip--strip-debug destination={} bytes={} sha256={}",
        source.display(),
        original.len(),
        oulipoly_state::completion_continuation::sha256(&original),
        destination.display(),
        image.len(),
        oulipoly_state::completion_continuation::sha256(&image)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn rejects_non_executable_and_existing_destination() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    std::fs::write(&source, b"\x7fELF not executable").unwrap();
    assert!(std::panic::catch_unwind(|| provision(&source, &root.path().join("new"))).is_err());
    let destination = root.path().join("existing");
    std::fs::write(&destination, b"preserve").unwrap();
    assert!(
        std::panic::catch_unwind(|| provision(Path::new("/usr/bin/true"), &destination)).is_err()
    );
    assert_eq!(std::fs::read(destination).unwrap(), b"preserve");
}

#[cfg(target_os = "linux")]
#[test]
fn private_copy_executes_and_preserves_source() {
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("runner");
    provision(Path::new("/usr/bin/true"), &destination);
    assert!(
        std::process::Command::new(destination)
            .status()
            .unwrap()
            .success()
    );
}

#[cfg(target_os = "linux")]
#[test]
fn rejects_oversized_non_debug_image_after_transform() {
    let root = tempfile::tempdir().unwrap();
    let padding = root.path().join("padding");
    std::fs::File::create(&padding)
        .unwrap()
        .set_len(256 * 1024 * 1024)
        .unwrap();
    let source = root.path().join("oversized");
    let status = std::process::Command::new("/usr/bin/objcopy")
        .arg("--add-section")
        .arg(format!(".fixture_padding={}", padding.display()))
        .arg("/usr/bin/true")
        .arg(&source)
        .status()
        .unwrap();
    assert!(status.success());
    let destination = root.path().join("copy");
    let rejected = std::panic::catch_unwind(|| provision(&source, &destination));
    assert!(rejected.is_err());
    assert!(
        std::fs::metadata(destination).unwrap().len() > 256 * 1024 * 1024,
        "negative control must reach the unchanged post-transform size guard"
    );
}
