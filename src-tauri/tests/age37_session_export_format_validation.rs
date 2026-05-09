#![cfg(unix)]

mod fixtures;

use fixtures::initiative_06_export::{ExportFixture, assert_json_error};

/// Risk: AGE-37 cut-over could remap unsupported export formats through a
/// future service seam.
/// Level: CLI characterization.
/// Observable: current `session export --format other` exits 2, emits
/// invalid-format JSON on stderr, and writes no stdout.
#[test]
fn export_unsupported_format_exits_2_with_invalid_format_json() {
    let fixture = ExportFixture::new();

    let output = fixture.run_export(
        "5169694d-de0f-40d1-890c-6e28e55bab27",
        &["--format", "other"],
    );

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let error = assert_json_error(&output, "invalid-format");
    assert_eq!(
        error["error"]["message"],
        "unsupported export format other; expected canonical-jsonl"
    );
}
