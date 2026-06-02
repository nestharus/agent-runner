const MARKER_FIELD: &str = "marker: PhantomData<&'a ()>";
const MARKER_FIELD_WITH_COMMA: &str = "marker: PhantomData<&'a ()>,";

fn provider_source() -> String {
    let src_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut source = String::new();

    for entry in std::fs::read_dir(&src_dir)
        .unwrap_or_else(|error| panic!("failed to read provider source directory: {error}"))
    {
        let path = entry
            .expect("provider source entry should be readable")
            .path();
        if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
            source.push_str(&std::fs::read_to_string(&path).unwrap_or_else(|error| {
                panic!(
                    "failed to read provider source file {}: {error}",
                    path.display()
                )
            }));
            source.push('\n');
        }
    }

    source
}

fn struct_body<'a>(source: &'a str, struct_name: &str) -> &'a str {
    let declaration = format!("pub struct {struct_name}<'a>");
    let start = source
        .find(&declaration)
        .unwrap_or_else(|| panic!("missing declaration for {struct_name}"));
    let remainder = &source[start..];
    let body_start = remainder
        .find('{')
        .unwrap_or_else(|| panic!("missing body start for {struct_name}"));
    let body_end = remainder[body_start..]
        .find("\n}")
        .unwrap_or_else(|| panic!("missing body end for {struct_name}"));

    &remainder[body_start..body_start + body_end]
}

#[test]
fn borrow_carrying_request_structs_keep_marker_anchors() {
    let provider_source = provider_source();
    let struct_names = [
        "LaunchRequest",
        "PolicyRequest",
        "ProviderContext",
        "QuotaRequest",
        "AuthRefreshRequest",
        "SessionTurnRequest",
        "SessionCaptureRequest",
        "RotationRequest",
        "RotationMaterializationRequest",
        "DiscoveryRequest",
    ];

    for struct_name in struct_names {
        let body = struct_body(&provider_source, struct_name);
        assert!(
            body.contains(MARKER_FIELD),
            "{struct_name} must retain the borrow marker field"
        );
    }

    assert_eq!(provider_source.matches(MARKER_FIELD_WITH_COMMA).count(), 10);
}
