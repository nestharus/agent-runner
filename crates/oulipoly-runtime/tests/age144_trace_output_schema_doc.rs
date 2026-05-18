use std::path::Path;

#[test]
fn t1_doc_exists_at_expected_path() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../conventions/agents-trace-output.md");

    assert!(path.exists());
}

#[test]
fn t2_required_sections_present() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../conventions/agents-trace-output.md")
        .canonicalize()
        .expect("trace output convention doc path should canonicalize");
    let doc =
        std::fs::read_to_string(&path).expect("trace output convention doc should be readable");

    for heading in [
        "## Purpose",
        "## Schema version",
        "## Source",
        "## Field catalog",
        "## Parse semantics",
        "## Stability guarantee",
        "## Consumer registry",
        "## Pull-site closure mechanism",
        "## Schema future / deferred",
        "## Declared roles",
        "## Intrinsic-surface declarations",
        "## Out-of-scope",
    ] {
        assert!(doc.contains(heading), "missing section heading: {heading}");
    }

    let mut previous_offset = None;
    for heading in [
        "## Purpose",
        "## Declared roles",
        "## Schema version",
        "## Source",
        "## Field catalog",
        "## Parse semantics",
        "## Stability guarantee",
        "## Consumer registry",
        "## Pull-site closure mechanism",
        "## Schema future / deferred",
        "## Intrinsic-surface declarations",
        "## Out-of-scope",
    ] {
        let offset = doc
            .find(heading)
            .unwrap_or_else(|| panic!("missing section heading: {heading}"));
        if let Some(previous_offset) = previous_offset {
            assert!(
                offset > previous_offset,
                "section heading is out of order: {heading}"
            );
        }
        previous_offset = Some(offset);
    }
}

#[test]
fn t3_v1_field_catalog_has_exactly_four_required_rows() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../conventions/agents-trace-output.md")
        .canonicalize()
        .expect("trace output convention doc path should canonicalize");
    let doc =
        std::fs::read_to_string(&path).expect("trace output convention doc should be readable");

    let field_catalog_offset = doc
        .find("## Field catalog")
        .expect("missing Field catalog heading");
    let field_catalog_heading_end = field_catalog_offset + "## Field catalog".len();
    let next_heading_offset = doc[field_catalog_heading_end..]
        .find("\n## ")
        .map(|relative_offset| field_catalog_heading_end + relative_offset + 1)
        .expect("missing heading after Field catalog");
    let field_catalog_region = &doc[field_catalog_offset..next_heading_offset];

    for field in [
        "invocation_uuid",
        "parent_invocation_uuid",
        "completion_status",
        "completion_timestamp",
    ] {
        let row = format!("| `{field}` |");
        assert_eq!(
            field_catalog_region.matches(&row).count(),
            1,
            "field row count for {field}"
        );
    }

    let canonical_row_count = field_catalog_region
        .lines()
        .filter(|line| {
            let Some(rest) = line.strip_prefix("| `") else {
                return false;
            };
            let Some((field_name, _)) = rest.split_once("` |") else {
                return false;
            };
            !field_name.is_empty()
                && field_name
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
        })
        .count();
    assert_eq!(canonical_row_count, 4, "v1 field catalog row count");

    assert!(
        !field_catalog_region.contains("| `declared_output_paths` |"),
        "declared_output_paths must not be in the v1 field catalog"
    );
}

#[test]
fn t4_declared_output_paths_is_v2_deferred() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../conventions/agents-trace-output.md")
        .canonicalize()
        .expect("trace output convention doc path should canonicalize");
    let doc =
        std::fs::read_to_string(&path).expect("trace output convention doc should be readable");

    assert!(doc.contains("## Schema future / deferred"));
    assert!(doc.contains("declared_output_paths"));
    assert!(doc.contains("v1 does NOT declare"));
    assert!(doc.contains(
        "until then, consumers needing path-to-invocation joins MUST continue using their existing topology + canonical-output-path matching mechanisms"
    ));
}

#[test]
fn t5_completion_status_enum_vocabulary() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../conventions/agents-trace-output.md")
        .canonicalize()
        .expect("trace output convention doc path should canonicalize");
    let doc =
        std::fs::read_to_string(&path).expect("trace output convention doc should be readable");

    for status in ["running", "succeeded", "failed", "legacy"] {
        assert!(
            doc.contains(status),
            "missing v1 completion_status value: {status}"
        );
    }
    assert!(
        !doc.contains("| `BLOCKED` |"),
        "BLOCKED must not appear in the v1 completion_status enum vocabulary"
    );
}

#[test]
fn t6_returned_artifacts_name_non_inference_rule() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../conventions/agents-trace-output.md")
        .canonicalize()
        .expect("trace output convention doc path should canonicalize");
    let doc =
        std::fs::read_to_string(&path).expect("trace output convention doc should be readable");

    assert!(doc.contains("## Parse semantics"));
    assert!(doc.contains("returned_artifacts.name"));
    assert!(doc.contains("MUST NOT infer `declared_output_paths` from `returned_artifacts.name`"));
}

#[test]
fn t7_stability_guarantee_additive_only() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../conventions/agents-trace-output.md")
        .canonicalize()
        .expect("trace output convention doc path should canonicalize");
    let doc =
        std::fs::read_to_string(&path).expect("trace output convention doc should be readable");

    assert!(doc.contains("## Stability guarantee"));
    assert!(doc.contains("additive"));
    assert!(doc.contains("breaking"));
    assert!(doc.contains("renames require a `schema_version` bump"));
}

#[test]
fn t8_closure_mechanism_common_interface_proof() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../conventions/agents-trace-output.md")
        .canonicalize()
        .expect("trace output convention doc path should canonicalize");
    let doc =
        std::fs::read_to_string(&path).expect("trace output convention doc should be readable");

    assert!(doc.contains("## Pull-site closure mechanism"));
    assert!(doc.contains("common-interface proof"));
    assert!(doc.contains("canonical-doc-as-schema recipe is NOT the closure mechanism"));
    assert!(doc.contains("repo-root"));
    assert!(doc.contains("not `~/ai/`"));
}
