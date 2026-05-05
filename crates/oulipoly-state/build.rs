fn main() {
    if let Some(head_path) = git_path("HEAD") {
        println!("cargo:rerun-if-changed={}", head_path.display());
    }
    if let Some(ref_path) = git_symbolic_ref()
        && let Some(ref_file) = git_path(&ref_path)
    {
        println!("cargo:rerun-if-changed={}", ref_file.display());
    }

    let commit = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout).ok()
            } else {
                None
            }
        })
        .map(|commit| commit.trim().to_string())
        .filter(|commit| !commit.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=BUILD_COMMIT={commit}");
}

fn git_path(path: &str) -> Option<std::path::PathBuf> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--git-path", path])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|path| std::path::PathBuf::from(path.trim()))
        .filter(|path| !path.as_os_str().is_empty())
}

fn git_symbolic_ref() -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["symbolic-ref", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|ref_path| ref_path.trim().to_string())
        .filter(|ref_path| !ref_path.is_empty())
}
