use std::path::Path;

use super::expected_claude_code_project_dir;

#[test]
fn age158_expected_claude_code_project_dir_maps_mixed_characters() {
    let path = Path::new("Az-09_./Beta:gamma\\Delta!");

    assert_eq!(
        expected_claude_code_project_dir(path),
        "Az-09---Beta-gamma-Delta-"
    );
}
