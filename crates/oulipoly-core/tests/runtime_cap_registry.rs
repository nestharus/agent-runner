#[path = "support/runtime_cap_checker.rs"]
mod checker;

use checker::{Site, UseSite};
use oulipoly_core::runtime_cap::{
    RuntimeCap, RuntimeCapClass, RuntimeCapUse, RuntimeCapUseKind, registry, validate_registry,
};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("core crate must be below workspace/crates")
        .to_path_buf()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExclusionDocument {
    schema_version: u32,
    exclusions: Vec<NonCapExclusion>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NonCapExclusion {
    source: String,
    declaration_scope: String,
    symbol: String,
    value_expression: String,
    reason: String,
}

impl NonCapExclusion {
    fn site(&self) -> Site {
        Site {
            source: self.source.clone(),
            scope: self.declaration_scope.clone(),
            symbol: self.symbol.clone(),
        }
    }
}

fn exclusions(root: &Path) -> Vec<NonCapExclusion> {
    let document: ExclusionDocument = serde_json::from_str(
        &fs::read_to_string(root.join("runtime-cap-exclusions.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(document.schema_version, 1);
    document.exclusions
}

fn cap_site(cap: &RuntimeCap) -> Site {
    Site {
        source: cap.source.clone(),
        scope: cap.declaration_scope.clone(),
        symbol: cap.symbol.clone(),
    }
}

fn use_site(cap: &RuntimeCap) -> UseSite {
    UseSite {
        source: cap.controlling_use.source.clone(),
        scope: cap.controlling_use.scope.clone(),
        symbol: cap.symbol.clone(),
    }
}

#[test]
fn every_numeric_declaration_is_registered_or_explicitly_excluded() {
    let root = workspace();
    let rust = checker::scan_workspace_rust(&root);
    let scripts = checker::scan_shipped_scripts(&root);
    let caps = registry();
    let exclusions = exclusions(&root);

    let registered_rust = caps
        .iter()
        .filter(|cap| cap.source.ends_with(".rs"))
        .map(|cap| (cap_site(cap), cap))
        .collect::<BTreeMap<_, _>>();
    let excluded_rust = exclusions
        .iter()
        .filter(|entry| entry.source.ends_with(".rs"))
        .map(|entry| (entry.site(), entry))
        .collect::<BTreeMap<_, _>>();

    let overlap = registered_rust
        .keys()
        .filter(|site| excluded_rust.contains_key(*site))
        .collect::<Vec<_>>();
    assert!(
        overlap.is_empty(),
        "sites are both caps and exclusions: {overlap:#?}"
    );

    let missing = rust
        .declarations
        .iter()
        .filter(|(site, _)| {
            !registered_rust.contains_key(*site) && !excluded_rust.contains_key(*site)
        })
        .map(|(site, declaration)| format!("{site:?} = {}", declaration.value_expression))
        .collect::<Vec<_>>();
    assert!(
        missing.is_empty(),
        "numeric production declarations require cap ownership or a non-cap exclusion:\n{missing:#?}"
    );

    let stale_caps = registered_rust
        .keys()
        .filter(|site| !rust.declarations.contains_key(*site))
        .collect::<Vec<_>>();
    assert!(
        stale_caps.is_empty(),
        "stale Rust cap entries: {stale_caps:#?}"
    );
    let stale_exclusions = excluded_rust
        .keys()
        .filter(|site| !rust.declarations.contains_key(*site))
        .collect::<Vec<_>>();
    assert!(
        stale_exclusions.is_empty(),
        "stale non-cap exclusions: {stale_exclusions:#?}"
    );

    let wrong_values = registered_rust
        .iter()
        .filter_map(|(site, cap)| {
            let actual = &rust.declarations[site].value_expression;
            (cap.default_value != *actual).then(|| {
                format!(
                    "{}: registry {:?}, source {:?}",
                    cap.id, cap.default_value, actual
                )
            })
        })
        .chain(excluded_rust.iter().filter_map(|(site, exclusion)| {
            let actual = &rust.declarations[site].value_expression;
            (exclusion.value_expression != *actual).then(|| {
                format!(
                    "excluded {}#{}: registry {:?}, source {:?}",
                    exclusion.source, exclusion.symbol, exclusion.value_expression, actual
                )
            })
        }))
        .collect::<Vec<_>>();
    assert!(
        wrong_values.is_empty(),
        "registry/exclusion values drifted from source:\n{}",
        wrong_values.join("\n")
    );

    let unbound = registered_rust
        .values()
        .filter(|cap| !rust.uses.contains(&use_site(cap)))
        .map(|cap| {
            let observed = rust
                .uses
                .iter()
                .filter(|site| site.symbol == cap.symbol)
                .map(|site| format!("{}#{}", site.source, site.scope))
                .collect::<Vec<_>>();
            format!(
                "{} -> {}#{}#{}; observed={observed:?}",
                cap.id, cap.controlling_use.source, cap.controlling_use.scope, cap.symbol,
            )
        })
        .collect::<Vec<_>>();
    assert!(
        unbound.is_empty(),
        "registry entries without the declared production use:\n{}",
        unbound.join("\n")
    );

    assert!(
        rust.raw_cap_literals.is_empty(),
        "anonymous Rust cap literals must be named and registered:\n{}",
        rust.raw_cap_literals.join("\n")
    );

    check_scripts(caps, &exclusions, &scripts);
}

fn check_scripts(
    caps: &[RuntimeCap],
    exclusions: &[NonCapExclusion],
    scripts: &checker::ScriptInventory,
) {
    let registered = caps
        .iter()
        .filter(|cap| cap.source.starts_with("scripts/"))
        .map(|cap| ((cap.source.as_str(), cap.symbol.as_str()), cap))
        .collect::<BTreeMap<_, _>>();
    let excluded = exclusions
        .iter()
        .filter(|entry| entry.source.starts_with("scripts/"))
        .map(|entry| ((entry.source.as_str(), entry.symbol.as_str()), entry))
        .collect::<BTreeMap<_, _>>();
    let discovered = scripts
        .declarations
        .iter()
        .map(|entry| ((entry.source.as_str(), entry.symbol.as_str()), entry))
        .collect::<BTreeMap<_, _>>();

    let missing = discovered
        .keys()
        .filter(|site| !registered.contains_key(*site) && !excluded.contains_key(*site))
        .collect::<Vec<_>>();
    assert!(
        missing.is_empty(),
        "shipped script declarations require cap ownership or exclusion: {missing:#?}"
    );
    let stale = registered
        .keys()
        .chain(excluded.keys())
        .filter(|site| !discovered.contains_key(*site))
        .collect::<Vec<_>>();
    assert!(stale.is_empty(), "stale shipped-script entries: {stale:#?}");
    let wrong = discovered
        .iter()
        .filter_map(|(site, declaration)| {
            let expected = registered
                .get(site)
                .map(|cap| cap.default_value.as_str())
                .or_else(|| {
                    excluded
                        .get(site)
                        .map(|entry| entry.value_expression.as_str())
                })
                .unwrap();
            (expected != declaration.value_expression).then(|| {
                format!(
                    "{}#{}: registry {:?}, source {:?}",
                    site.0, site.1, expected, declaration.value_expression
                )
            })
        })
        .collect::<Vec<_>>();
    assert!(
        wrong.is_empty(),
        "script default values drifted:\n{}",
        wrong.join("\n")
    );
    let unused = discovered
        .values()
        .filter(|declaration| {
            registered.contains_key(&(declaration.source.as_str(), declaration.symbol.as_str()))
                && !declaration.used
        })
        .collect::<Vec<_>>();
    assert!(
        unused.is_empty(),
        "registered script caps are unused: {unused:#?}"
    );
    assert!(
        scripts.raw_cap_literals.is_empty(),
        "anonymous shipped-script cap literals must be named:\n{}",
        scripts.raw_cap_literals.join("\n")
    );
}

#[test]
fn non_cap_exclusions_are_specific_sorted_and_source_bound() {
    let root = workspace();
    let exclusions = exclusions(&root);
    assert!(
        exclusions
            .windows(2)
            .all(|pair| pair[0].site() < pair[1].site()),
        "non-cap exclusions must be sorted by source/scope/symbol"
    );
    let mut sites = BTreeSet::new();
    for exclusion in exclusions {
        assert!(sites.insert(exclusion.site()), "duplicate exclusion site");
        assert!(
            exclusion.reason.len() >= 24,
            "non-specific exclusion: {}",
            exclusion.reason
        );
        assert!(
            !exclusion.reason.contains("not a cap"),
            "exclusion must explain why: {}",
            exclusion.reason
        );
    }
}

#[test]
fn checker_fixtures_cover_complete_discovery_and_receiver_precision() {
    let root = workspace().join("crates/oulipoly-core/tests/fixtures/runtime_cap_checker");
    let source = fs::read_to_string(root.join("positive.rs")).unwrap();
    let scanned = checker::scan_rust_source("fixture/positive.rs", &source).unwrap();
    let symbols = scanned
        .declarations
        .keys()
        .map(|site| site.symbol.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(symbols, BTreeSet::from(["ODDLY_NAMED", "POLL_TICKS"]));
    assert!(scanned.raw_cap_literals.is_empty());
    assert!(scanned.uses.iter().any(|site| site.symbol == "POLL_TICKS"));
}

#[test]
fn checker_excludes_only_sources_proven_test_only_by_module_ownership() {
    let root =
        workspace().join("crates/oulipoly-core/tests/fixtures/runtime_cap_checker/module_graph");
    let source_root = root.join("crates/demo/src");
    let lib = source_root.join("lib.rs");
    let shared = source_root.join("shared.rs");
    let only_test = source_root.join("only_test.rs");
    let test_only =
        checker::mechanically_test_only_sources(&[lib.clone(), shared.clone(), only_test.clone()]);

    assert!(!test_only.contains(&lib));
    assert!(!test_only.contains(&shared));
    assert!(test_only.contains(&only_test));
}

#[test]
fn checker_fixtures_reject_relevant_anonymous_rust_and_script_caps() {
    let root = workspace().join("crates/oulipoly-core/tests/fixtures/runtime_cap_checker");
    let source = fs::read_to_string(root.join("negative.rs")).unwrap();
    let scanned = checker::scan_rust_source("fixture/negative.rs", &source).unwrap();
    assert_eq!(
        scanned.raw_cap_literals.len(),
        6,
        "{:#?}",
        scanned.raw_cap_literals
    );
    for expected in [
        "direct literal duration constructor",
        "literal capacity passed to std::sync::mpsc::sync_channel",
        "literal I/O bound passed to Read::take",
        "literal timeout passed to libc::poll",
        "literal cap value assigned to Limits.queue_capacity",
        "literal fixed-size allocation of at least 1 KiB",
    ] {
        assert!(
            scanned
                .raw_cap_literals
                .iter()
                .any(|literal| literal.ends_with(expected)),
            "missing {expected:?} in {:#?}",
            scanned.raw_cap_literals
        );
    }

    let positive_script = fs::read_to_string(root.join("positive.sh")).unwrap();
    let positive = checker::scan_script_source("fixture/positive.sh", &positive_script);
    assert_eq!(positive.declarations.len(), 1);
    assert!(positive.declarations.iter().next().unwrap().used);
    assert!(positive.raw_cap_literals.is_empty());

    let negative_script = fs::read_to_string(root.join("negative.sh")).unwrap();
    let negative = checker::scan_script_source("fixture/negative.sh", &negative_script);
    assert_eq!(negative.raw_cap_literals.len(), 3);
}

#[test]
fn invalid_unowned_and_generated_registry_entries_are_rejected() {
    let valid = RuntimeCap {
        id: "owner.cap".into(),
        owner: "owner/module".into(),
        class: RuntimeCapClass::ResourceGuard,
        protected_resource: "one-byte fixture allocation".into(),
        exhaustion_behavior: "reject the fixture before allocating a second byte".into(),
        observability: "FixtureError::Capacity is returned to the caller".into(),
        configurability: "fixed by the fixture source declaration".into(),
        rationale: "the fixture demonstrates a source-grounded registry entry".into(),
        default_value: "1".into(),
        source: "crates/example/src/lib.rs".into(),
        declaration_scope: "module".into(),
        symbol: "MAX_BYTES".into(),
        controlling_use: RuntimeCapUse {
            source: "crates/example/src/lib.rs".into(),
            scope: "module::fn:read".into(),
            kind: RuntimeCapUseKind::DirectControl,
        },
    };
    assert!(validate_registry(std::slice::from_ref(&valid)).is_ok());
    let mut invalid = valid.clone();
    invalid.id = "Unstable ID".into();
    assert!(validate_registry(&[invalid]).is_err());
    let mut unowned = valid.clone();
    unowned.owner.clear();
    assert!(validate_registry(&[unowned]).is_err());
    let mut generated = valid;
    generated.default_value = "source expression MAX_BYTES".into();
    assert!(validate_registry(&[generated]).is_err());
}

#[test]
fn registry_class_inventory_is_queryable_sorted_and_names_provisional_stopgaps() {
    let mut counts = BTreeMap::new();
    for cap in registry() {
        *counts.entry(cap.class).or_insert(0usize) += 1;
    }
    assert!(!counts.is_empty());
    assert_eq!(
        registry()
            .iter()
            .filter(|cap| cap.class == RuntimeCapClass::ProvisionalStopgap)
            .map(|cap| cap.id.as_str())
            .collect::<Vec<_>>(),
        vec!["runtime.executor.cli.live-session-binding.worker-join-timeout"]
    );
    assert!(registry().windows(2).all(|pair| pair[0].id < pair[1].id));
    let raw: serde_json::Value =
        serde_json::from_str(oulipoly_core::runtime_cap::registry_json()).unwrap();
    let raw_ids = raw["caps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|cap| cap["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(raw_ids.windows(2).all(|pair| pair[0] < pair[1]));
}
