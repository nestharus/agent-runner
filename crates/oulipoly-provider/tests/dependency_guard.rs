use std::process::Command;

#[test]
fn provider_crate_has_only_neutral_normal_dependencies() {
    let output = Command::new("cargo")
        .args(["tree", "-p", "oulipoly-provider", "--edges", "normal"])
        .output()
        .expect("cargo tree must run");
    assert!(
        output.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let tree = String::from_utf8(output.stdout).expect("cargo tree output must be UTF-8");
    let forbidden = [
        "oulipoly-runtime",
        "oulipoly-config",
        "oulipoly-state",
        "oulipoly-provider-",
        "provider-implementation",
        "reqwest",
        "hyper",
        "tokio",
        "rustls",
        "aws-lc",
    ];
    let violations = forbidden
        .iter()
        .filter(|needle| tree.contains(**needle))
        .copied()
        .collect::<Vec<_>>();

    assert!(
        violations.is_empty(),
        "oulipoly-provider normal dependencies must remain neutral; forbidden deps {violations:?} in:\n{tree}"
    );
}
