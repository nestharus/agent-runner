//! AGE-160 risk: cohesion fingerprint for db.rs/lib.rs declared roles.
//! Selected level: unit.
//! Source: the AGE-160 proposal § Test-intent track.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

fn first_lines(path: &Path, count: usize) -> String {
    fs::read_to_string(path)
        .expect("read source")
        .lines()
        .take(count)
        .collect::<Vec<_>>()
        .join("\n")
}

fn declared_roles(source: &str) -> BTreeSet<String> {
    let Some(start) = source.find("## Declared roles") else {
        return BTreeSet::new();
    };
    source[start..]
        .lines()
        .skip(1)
        .take_while(|line| {
            let trimmed = line.trim_start_matches('/').trim_start_matches('!').trim();
            trimmed.is_empty() || trimmed.starts_with('-')
        })
        .filter_map(|line| {
            let trimmed = line.trim_start_matches('/').trim_start_matches('!').trim();
            trimmed.strip_prefix("- ").map(str::to_string)
        })
        .collect()
}

/// AGE-160 risk: cohesion fingerprint for declared multi-role state DB and root API carrier.
/// Selected level: unit.
/// Source: the AGE-160 proposal § Test-intent track.
#[test]
fn age160_db_and_lib_declared_roles_are_present_for_cohesion_auditor() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let db_rs = first_lines(&manifest_dir.join("src/db.rs"), 200);
    let lib_rs = first_lines(&manifest_dir.join("src/lib.rs"), 200);

    assert_eq!(
        declared_roles(&db_rs),
        BTreeSet::from([
            "accessor".to_string(),
            "filter".to_string(),
            "formatter".to_string(),
            "mapper".to_string(),
            "orchestration".to_string(),
            "parser".to_string(),
            "predicate".to_string(),
            "validator".to_string(),
        ])
    );
    assert_eq!(
        declared_roles(&lib_rs),
        BTreeSet::from(["accessor".to_string(), "validator".to_string()])
    );
}
