//! AGE-347 generation4: exact observation wire vocabulary is not routing authority.
//! This predicate recognizes closed syntax at protocol roles, NOT whole files.
//! Callers still scan the remainder of every admitted line for denied literals.
//! Authority and limitations: docs/architecture/observation-native-accounting.md.

const WIRE: &str = "codex_observation_io_v1";

pub fn vocabulary_text<'a>(path: &str, line: &'a str) -> std::borrow::Cow<'a, str> {
    if protocol_use(path.trim_start_matches("./"), line.trim()) {
        std::borrow::Cow::Owned(line.replace(WIRE, ""))
    } else {
        std::borrow::Cow::Borrowed(line)
    }
}

fn protocol_use(path: &str, line: &str) -> bool {
    match path {
        "test-support/provider_wire_policy.rs" => line == format!("const WIRE: &str = {WIRE:?};"),
        "crates/oulipoly-runtime/src/session_provider/turns_source_io.rs" => {
            line == format!("const DECLARATION: &str = {WIRE:?};")
        }
        "contract/v1/session.schema.json" => schema_pattern(line),
        "docs/architecture/observation-native-accounting.md" => {
            line == format!("{WIRE}:forward=<decimal>;reconstruction=<decimal>;metadata=<decimal>")
        }
        "crates/oulipoly-provider/tests/observation_source_contract.rs" => test_declaration(line),
        "src-tauri/src/run/resume/observation_paired_tests.rs" => {
            line == format!(".filter(|w| w.starts_with(\"{WIRE}:\"))")
        }
        "src-tauri/src/run/resume/observation_paired_boundaries.rs" => {
            line == format!(".filter_map(|w| w.strip_prefix(\"{WIRE}:\"))")
        }
        "src-tauri/src/run/resume/observation_paired_proxy.py" => proxy_warning(line),
        _ => false,
    }
}

fn schema_pattern(line: &str) -> bool {
    let namespace = format!("\"^{WIRE}\"");
    [
        format!("\"warnings\": {{ \"contains\": {{ \"pattern\": {namespace} }} }}"),
        format!("\"contains\": {{ \"pattern\": {namespace} }},"),
        format!("\"if\": {{ \"pattern\": {namespace} }},"),
        format!("\"warnings\": {{ \"items\": {{ \"not\": {{ \"pattern\": {namespace} }} }} }}"),
        format!("\"pattern\": \"^{WIRE}:forward=[0-9]{{1,20}};reconstruction=[0-9]{{1,20}};metadata=[0-9]{{1,20}}$\","),
    ].iter().any(|syntax| line == syntax)
}

fn test_declaration(line: &str) -> bool {
    // Only string data in the schema's discriminating cases; no expressions,
    // branch conditions, calls, or suffix statements are admitted here.
    let value = ["let declaration = ", "let maximum = ", "let valid = "]
        .iter()
        .find_map(|prefix| line.strip_prefix(prefix).and_then(|v| v.strip_suffix(';')))
        .or_else(|| {
            line.strip_prefix("json!([")
                .and_then(|v| v.strip_suffix("]),"))
        })
        .unwrap_or(line);
    let value = value.strip_prefix("valid, ").unwrap_or(value);
    let Some(value) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) else {
        return false;
    };
    let Some(fields) = value.strip_prefix(WIRE) else {
        return false;
    };
    fields.is_empty()
        || fields.strip_prefix(":forward=").is_some_and(|fields| {
            fields.split(';').all(|field| {
                let number = field
                    .strip_prefix("reconstruction=")
                    .or_else(|| field.strip_prefix("metadata="))
                    .unwrap_or(field);
                number
                    .bytes()
                    .all(|b| b.is_ascii_digit() || b"+- .e\\n".contains(&b))
            })
        })
}

fn proxy_warning(line: &str) -> bool {
    line == format!(
        "declaration = next(w for w in warnings if w.startswith(\"{WIRE}:\")) if observation else \"\""
    ) || line
        == format!(
            "result[\"warnings\"] = other + [\"{WIRE}:\" + \";\".join(f\"{{k}}={{v}}\" for k,v in fields.items())]"
        )
}

#[test]
fn exact_wire_permission_is_not_a_file_or_line_amnesty() {
    let validator = "crates/oulipoly-runtime/src/session_provider/turns_source_io.rs";
    let declaration = format!("const DECLARATION: &str = {WIRE:?};");
    assert!(!vocabulary_text(validator, &declaration).contains(WIRE));
    let denied = WIRE.split_once('_').unwrap().0;
    for line in [
        format!("{declaration} let arbitrary = {denied:?};"),
        format!("let arbitrary = {denied:?};"),
        format!("if provider == {denied:?} {{ route(); }}"),
        format!("if provider == {WIRE:?} {{ route(); }}"),
        format!("match account {{ {WIRE:?} => route(), _ => other() }}"),
        format!("const ROUTE: &str = {WIRE:?};"),
    ] {
        assert!(vocabulary_text(validator, &line).contains(denied), "{line}");
        assert!(
            vocabulary_text(
                "crates/oulipoly-provider/tests/observation_source_contract.rs",
                &line
            )
            .contains(denied),
            "{line}"
        );
    }
    assert!(vocabulary_text("unrelated.rs", &declaration).contains(denied));
    let schema = "contract/v1/session.schema.json";
    let allowed = format!("\"contains\": {{ \"pattern\": \"^{WIRE}\" }},");
    assert!(!vocabulary_text(schema, &allowed).contains(denied));
    assert!(vocabulary_text(schema, &format!("{allowed} \"route\": {denied:?}")).contains(denied));
    assert!(vocabulary_text(schema, &format!("\"route\": {WIRE:?}")).contains(denied));
}

// The lexical exception alone cannot tell a warnings schema from a routing
// schema, or keep a private constant from becoming an alias used for routing.
// Check those causal roles as well; callers run these tests in all three targets.
fn validator_roles(source: &str) -> bool {
    let expected_uses = [
        format!("const DECLARATION: &str = {WIRE:?};"),
        ".filter(|value| value.starts_with(DECLARATION));".into(),
        ".strip_prefix(DECLARATION)?".into(),
        "format!(\"{DECLARATION}:{fields}\")".into(),
        "validate_accounting(&[valid.clone(), DECLARATION.into()], UserObservation, 1, 1)".into(),
    ];
    let actual = source
        .lines()
        .map(str::trim)
        .filter(|line| line.contains("DECLARATION"))
        .collect::<Vec<_>>();
    if actual != expected_uses {
        return false;
    }
    let compact = source.split_whitespace().collect::<String>();
    compact.contains("pub(super)fnvalidate_source_io(result:&ProviderReadPageResult,request:&SessionProviderReadPageRequest<'_>,)->Result<(),SessionProviderError>{validate_accounting(&result.warnings,request.projection,request.max_source_bytes,result.source_bytes_examined,)}")
        && compact.contains("fnvalidate_accounting(warnings:&[String],projection:SessionProviderTurnProjection,quantum:u64,total:u64,)->Result<(),SessionProviderError>{letmutdeclarations=warnings.iter().filter(|value|value.starts_with(DECLARATION));")
}

fn schema_roles(schema: &serde_json::Value) -> bool {
    fn collect(value: &serde_json::Value, path: &str, found: &mut Vec<String>) {
        match value {
            serde_json::Value::Object(map) => {
                for (key, value) in map {
                    collect(value, &format!("{path}/{key}"), found);
                }
            }
            serde_json::Value::Array(values) => {
                for (index, value) in values.iter().enumerate() {
                    collect(value, &format!("{path}/{index}"), found);
                }
            }
            serde_json::Value::String(value) if value.contains(WIRE) => found.push(path.into()),
            _ => {}
        }
    }
    let mut actual = Vec::new();
    collect(schema, "", &mut actual);
    let mut expected = [
        "then/if/properties/warnings/contains/pattern",
        "then/then/properties/warnings/contains/pattern",
        "then/then/properties/warnings/items/if/pattern",
        "then/then/properties/warnings/items/then/pattern",
        "else/properties/warnings/items/not/pattern",
    ]
    .map(|suffix| format!("/$defs/SessionReadTurnsResult/allOf/0/{suffix}"));
    actual.sort();
    expected.sort();
    actual == expected
}

#[test]
fn wire_usage_stays_in_warning_validation_not_identity_or_routing() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|path| path.join("contract/v1/session.schema.json").is_file())
        .unwrap();
    let source = std::fs::read_to_string(
        root.join("crates/oulipoly-runtime/src/session_provider/turns_source_io.rs"),
    )
    .unwrap();
    assert!(
        validator_roles(&source),
        "wire alias or its warning-only dataflow escaped validation"
    );
    assert!(!validator_roles(
        &source.replace("&result.warnings", "&request.provider_names")
    ));
    assert!(!validator_roles(&source.replace(
        "warnings\n        .iter()",
        "providers\n        .iter()"
    )));
    assert!(!validator_roles(&format!(
        "{source}\nfn route() {{ select(DECLARATION); }}"
    )));
    let schema: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("contract/v1/session.schema.json")).unwrap(),
    )
    .unwrap();
    assert!(
        schema_roles(&schema),
        "wire vocabulary escaped the response warnings schema"
    );
    let mut routing = schema.clone();
    routing["$defs"]["SessionReadTurnsResult"]["properties"]["provider_instance_id"]["const"] =
        WIRE.into();
    assert!(!schema_roles(&routing));
    let moved = serde_json::json!({"provider_id": schema});
    assert!(
        !schema_roles(&moved),
        "moving authentic syntax into routing is not permitted"
    );
}
