use std::path::Path;
use std::process::Command;

use super::{MigrationFixture, claude_project_dir_name};

#[test]
fn age158_claude_project_dir_name_maps_mixed_characters() {
    let path = Path::new("Az-09_./Beta:gamma\\Delta!");

    assert_eq!(claude_project_dir_name(path), "Az-09---Beta-gamma-Delta-");
}

#[test]
fn age158_fake_claude_reports_missing_resume() {
    let fixture = MigrationFixture::new();
    let script = fixture.fake_claude();

    let output = Command::new(script)
        .arg("--resume")
        .arg("missing-session")
        .current_dir(&fixture.resume_workspace)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert_eq!(output.stdout, b"");
    assert_eq!(
        output.stderr,
        b"No conversation found with session ID: missing-session\n"
    );
}
