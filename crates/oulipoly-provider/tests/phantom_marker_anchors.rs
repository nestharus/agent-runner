const PROVIDER_SOURCE: &str = include_str!("../src/lib.rs");
const MARKER_FIELD: &str = "marker: PhantomData<&'a ()>";
const MARKER_FIELD_WITH_COMMA: &str = "marker: PhantomData<&'a ()>,";

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
    let struct_names = [
        "LaunchRequest",
        "PolicyRequest",
        "TerminalSignalEvidence",
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
        let body = struct_body(PROVIDER_SOURCE, struct_name);
        assert!(
            body.contains(MARKER_FIELD),
            "{struct_name} must retain the borrow marker field"
        );
    }

    assert_eq!(PROVIDER_SOURCE.matches(MARKER_FIELD_WITH_COMMA).count(), 11);
}
