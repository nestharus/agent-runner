//! Complete source membership for the vocabulary guards; receipts are external
//! invocation inputs, never repository policy. Declared roles: validator, accessor, parser.
#![allow(dead_code)] // Each integration target uses a different metric adapter.

#[path = "../../test-support/provider_wire_policy.rs"]
mod provider_wire_policy;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output};

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Reference {
    path: PathBuf,
    sha256: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    path: String,
    sha256: String,
    output_producer: Option<usize>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    version: u32,
    invocation: String,
    repository: PathBuf,
    head: String,
    index_sha256: String,
    producers: Vec<Reference>,
    entries: Vec<Entry>,
}

pub struct SourceSet {
    root: PathBuf,
    sources: BTreeSet<String>,
    untracked: BTreeSet<String>,
    historical: BTreeMap<String, HistoricalOutput>,
    residues: BTreeMap<String, ExactResidue>,
}

struct HistoricalOutput {
    sha256: String,
    full_excluded: bool,
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn git(root: &Path, args: &[&str]) -> Vec<u8> {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .expect("run git source membership");
    assert!(output.status.success(), "git {args:?}: {output:?}");
    output.stdout
}

fn paths(bytes: &[u8]) -> BTreeSet<String> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|p| !p.is_empty())
        .map(|p| {
            let path = std::str::from_utf8(p).expect("non-UTF-8 candidate path");
            assert!(
                Path::new(path)
                    .components()
                    .all(|c| matches!(c, Component::Normal(_))),
                "unsafe candidate {path:?}"
            );
            path.to_owned()
        })
        .collect()
}

fn read(root: &Path, path: &str) -> Vec<u8> {
    let full = root.join(path);
    let metadata = std::fs::symlink_metadata(&full)
        .unwrap_or_else(|e| panic!("unreadable candidate {path:?}: {e}"));
    // A tracked link is itself a configuration input. Scan its link text,
    // never recursively follow it or import external bytes into membership.
    if metadata.file_type().is_symlink() {
        return std::fs::read_link(&full)
            .expect("read source link")
            .to_str()
            .expect("non-UTF-8 source link")
            .as_bytes()
            .to_vec();
    }
    assert!(
        metadata.is_file(),
        "unsupported source input {path:?}: {metadata:?}"
    );
    std::fs::read(full).unwrap_or_else(|e| panic!("read source {path:?}: {e}"))
}

impl SourceSet {
    pub fn load(root: &Path) -> Self {
        let receipt = std::env::var_os("VOCABULARY_SOURCE_RECEIPT").map(PathBuf::from);
        let invocation = std::env::var("VOCABULARY_SOURCE_INVOCATION").ok();
        let mut selected = Self::load_input(root, receipt.as_deref(), invocation.as_deref());
        selected.historical = historical_outputs(&selected.root);
        selected.residues = exact_residue_policy();
        eprintln!(
            "historical projection entries={}",
            selected.historical.len()
        );
        selected
    }

    fn load_input(root: &Path, receipt: Option<&Path>, invocation: Option<&str>) -> Self {
        let root = root.canonicalize().expect("canonical repository");
        let tracked = paths(&git(&root, &["ls-files", "--cached", "-z"]));
        // Preserve nonignored-untracked semantics; tracked inputs remain accounted.
        let untracked = paths(&git(
            &root,
            &["ls-files", "--others", "--exclude-standard", "-z"],
        ));
        let candidates: BTreeSet<_> = tracked.union(&untracked).cloned().collect();
        let Some(receipt_path) = receipt else {
            assert!(
                invocation.is_none(),
                "invocation without membership receipt"
            );
            for path in &candidates {
                read(&root, path);
            }
            return Self {
                root,
                sources: candidates,
                untracked,
                historical: BTreeMap::new(),
                residues: BTreeMap::new(),
            };
        };
        let receipt_path = receipt_path
            .canonicalize()
            .expect("membership receipt path");
        assert!(
            !receipt_path.starts_with(&root),
            "local authority must remain outside repository"
        );
        let receipt: Receipt =
            serde_json::from_slice(&std::fs::read(receipt_path).expect("read membership receipt"))
                .expect("malformed membership receipt");
        assert_eq!(receipt.version, 1, "membership version");
        assert!(!receipt.invocation.is_empty(), "empty invocation");
        assert_eq!(
            Some(receipt.invocation.as_str()),
            invocation,
            "stale invocation"
        );
        assert_eq!(receipt.repository, root, "wrong repository");
        assert_eq!(
            receipt.head.as_bytes(),
            git(&root, &["rev-parse", "HEAD"]).trim_ascii(),
            "stale head"
        );
        assert_eq!(
            receipt.index_sha256,
            digest(&git(&root, &["ls-files", "--stage", "-z"])),
            "stale index"
        );
        let mut inventories = Vec::new();
        for reference in &receipt.producers {
            let path = reference.path.canonicalize().expect("producer reference");
            assert!(
                !path.starts_with(&root),
                "producer authority must remain external"
            );
            let bytes = std::fs::read(path).expect("read producer reference");
            assert_eq!(digest(&bytes), reference.sha256, "stale producer reference");
            inventories.push(
                serde_json::from_slice::<BTreeMap<String, String>>(&bytes)
                    .expect("malformed producer inventory"),
            );
        }
        let mut accounted = BTreeSet::new();
        let mut sources = BTreeSet::new();
        for entry in receipt.entries {
            assert!(
                candidates.contains(&entry.path),
                "unknown receipt path {:?}",
                entry.path
            );
            assert!(
                accounted.insert(entry.path.clone()),
                "duplicate receipt entry"
            );
            assert_eq!(
                digest(&read(&root, &entry.path)),
                entry.sha256,
                "changed input {:?}",
                entry.path
            );
            match entry.output_producer {
                Some(producer) => {
                    assert!(
                        !tracked.contains(&entry.path),
                        "tracked product cannot be output: {}",
                        entry.path
                    );
                    assert!(
                        producer < receipt.producers.len(),
                        "missing producer attestation"
                    );
                    assert_eq!(
                        inventories[producer].get(&entry.path),
                        Some(&entry.sha256),
                        "output not bound by producer inventory"
                    );
                }
                None => {
                    sources.insert(entry.path);
                }
            }
        }
        assert_eq!(
            accounted, candidates,
            "unaccounted source additions/removals"
        );
        let untracked = untracked.intersection(&sources).cloned().collect();
        eprintln!(
            "membership admitted={} attested_outputs={}",
            sources.len(),
            accounted.len() - sources.len()
        );
        Self {
            root,
            sources,
            untracked,
            historical: BTreeMap::new(),
            residues: BTreeMap::new(),
        }
    }

    fn historical_output(&self, path: &str, bytes: &[u8], full: bool) -> bool {
        self.historical
            .get(path)
            .is_some_and(|output| (!full || output.full_excluded) && digest(bytes) == output.sha256)
    }

    pub fn full_occurrences(&self, pattern: &str) -> usize {
        // Enumerate using rg's normal hidden traversal and ignore rules FIRST.
        // Explicit per-file counting must never override that eligibility.
        let output = Command::new("rg")
            .current_dir(&self.root)
            .args(["--no-config", "--files", "--hidden", "-0", "-g", "!.git/**"])
            .output()
            .expect("rg traversal membership");
        assert_search(&output);
        let traversable = paths(&output.stdout);
        let mut count = 0;
        let mut raw_count = 0;
        for path in &self.sources {
            let bytes = read(&self.root, path);
            let historical = self.historical_output(path, &bytes, true);
            if !traversable.contains(path) || historical {
                eprintln!("full ineligible {path:?} historical={historical}");
                continue;
            }
            let output = Command::new("rg")
                .current_dir(&self.root)
                .args(["--no-config", "--with-filename", "-o", pattern, "--", path])
                .output()
                .expect("rg source occurrences");
            assert_search(&output);
            let text = String::from_utf8(output.stdout).expect("non-UTF-8 occurrence output");
            eprint!("{text}");
            let raw = text.lines().count();
            raw_count += raw;
            let wire = std::str::from_utf8(&bytes).map_or(0, |text| {
                text.lines()
                    .map(|line| {
                        occurrences(line, pattern) - vocabulary_occurrences(path, line, pattern)
                    })
                    .sum::<usize>()
            });
            count += raw.checked_sub(wire).expect("wire exceeds raw full count");
        }
        eprintln!("source full raw={raw_count} vocabulary={count}");
        count
    }

    pub fn added_occurrences(&self, base: Option<&str>, pattern: &str) -> usize {
        let mut args = vec!["diff", "--no-ext-diff", "--name-only", "-z"];
        if let Some(base) = base {
            args.push(base);
        }
        args.extend(["--", "."]);
        let exclusions = self.diff_exclusions(base);
        args.extend(exclusions.iter().map(String::as_str));
        let changed = paths(&git(&self.root, &args));
        let mut tracked_count = 0;
        let mut raw_tracked = 0;
        // Literal per-path diffs retain identity without parsing quoted diff
        // headers; plain-index and historical-base comparisons remain distinct.
        for path in changed.intersection(&self.sources) {
            let literal = format!(":(literal){path}");
            let mut args = vec!["diff", "--no-ext-diff", "--unified=0"];
            if let Some(base) = base {
                args.push(base);
            }
            args.extend(["--", &literal]);
            let diff = String::from_utf8(git(&self.root, &args)).expect("source diff UTF-8");
            for (_, line) in added_rows(&diff) {
                let raw = occurrences(&line, pattern);
                raw_tracked += raw;
                tracked_count += vocabulary_occurrences(path, &line, pattern);
                if raw != 0 {
                    eprintln!("tracked added {path}:{line}");
                }
            }
        }
        let mut untracked_count = 0;
        let mut raw_untracked = 0;
        for path in &self.untracked {
            let bytes = read(&self.root, path);
            if self.historical_output(path, &bytes, false) {
                continue;
            }
            let text = String::from_utf8(bytes).unwrap_or_else(|e| {
                panic!("unsupported non-UTF-8 untracked product {path:?}: {e}")
            });
            for line in text.lines().filter(|line| occurrences(line, pattern) != 0) {
                eprintln!("untracked {path}:{line}");
            }
            raw_untracked += occurrences(&text, pattern);
            untracked_count += text
                .lines()
                .map(|line| vocabulary_occurrences(path, line, pattern))
                .sum::<usize>();
        }
        eprintln!(
            "source added base={base:?} raw_tracked={raw_tracked} raw_untracked={raw_untracked} vocabulary_tracked={tracked_count} vocabulary_untracked={untracked_count}"
        );
        let approved = self.approved_added_occurrences(base, pattern);
        let raw = tracked_count + untracked_count;
        let rejected = raw
            .checked_sub(approved)
            .expect("residue exceeds raw delta");
        eprintln!("source delta raw={raw} exact_approved={approved} rejected={rejected}");
        rejected
    }

    // Whole-file digest is an invalidation fence, NOT a whole-file exemption.
    // Only the enumerated physical rows can contribute to a delta deduction.
    // Unrelated edits conservatively revoke the allowance; no automatic rebase.
    fn exact_rows(&self, path: &str, policy: &ExactResidue) -> BTreeMap<usize, String> {
        if !self.sources.contains(path) || self.untracked.contains(path) {
            return BTreeMap::new();
        }
        let metadata = std::fs::symlink_metadata(self.root.join(path)).expect("residue metadata");
        let bytes = read(&self.root, path);
        if !metadata.is_file() || digest(&bytes) != policy.sha256 {
            eprintln!("exact residue invalidated {path:?}");
            return BTreeMap::new();
        }
        let text = String::from_utf8(bytes).expect("residue UTF-8");
        let lines: Vec<_> = text.lines().collect();
        policy
            .lines
            .iter()
            .map(|&number| {
                assert!(number > 0 && number <= lines.len(), "invalid residue row");
                (number, lines[number - 1].to_owned())
            })
            .collect()
    }

    fn approved_added_occurrences(&self, base: Option<&str>, pattern: &str) -> usize {
        let mut count = 0;
        for (path, policy) in &self.residues {
            let rows = self.exact_rows(path, policy);
            if rows.is_empty() {
                continue;
            }
            let literal = format!(":(literal){path}");
            let mut args = vec!["diff", "--no-ext-diff", "--unified=0"];
            if let Some(base) = base {
                args.push(base);
            }
            args.extend(["--", &literal]);
            let diff = String::from_utf8(git(&self.root, &args)).expect("residue diff UTF-8");
            for (number, line) in added_rows(&diff) {
                if rows.get(&number) == Some(&line) {
                    let hits = vocabulary_occurrences(path, &line, pattern);
                    eprintln!("exact approved added {path}:{number}:{line} occurrences={hits}");
                    count += hits;
                }
            }
        }
        count
    }

    pub fn unapproved_new_rows(
        &self,
        base: &BTreeSet<String>,
        current: &BTreeSet<String>,
    ) -> Vec<String> {
        let raw: BTreeSet<_> = current.difference(base).cloned().collect();
        eprintln!("source raw new rows={} {raw:#?}", raw.len());
        let mut approved = BTreeSet::new();
        for (path, policy) in &self.residues {
            for line in self.exact_rows(path, policy).values() {
                let row = format!("{path}:{line}");
                if raw.contains(&row) {
                    approved.insert(row);
                }
            }
        }
        let rejected: Vec<_> = raw.difference(&approved).cloned().collect();
        eprintln!(
            "source row delta raw={} exact_approved={} rejected={} approved={approved:#?}",
            raw.len(),
            approved.len(),
            rejected.len()
        );
        rejected
    }

    fn diff_exclusions(&self, base: Option<&str>) -> Vec<String> {
        let mut exclusions = Vec::new();
        for (path, output) in &self.historical {
            if !self.sources.contains(path) {
                continue;
            }
            if !self.historical_output(path, &read(&self.root, path), false) {
                continue;
            }
            let object = format!("{}:{path}", base.unwrap_or(""));
            // A missing preimage is an error, not authority to omit a path.
            let bytes = git(&self.root, &["show", &object]);
            if digest(&bytes) == output.sha256 {
                exclusions.push(format!(":(exclude,literal){path}"));
            }
        }
        exclusions
    }

    pub fn line_set(&self, base: Option<&str>, pattern: &str) -> BTreeSet<String> {
        let selected = match base {
            Some(base) => paths(&git(
                &self.root,
                &["ls-tree", "-r", "--name-only", "-z", base],
            )),
            None => self.sources.clone(),
        };
        let mut hits = BTreeSet::new();
        for path in selected {
            let bytes = match base {
                Some(base) => git(&self.root, &["show", &format!("{base}:{path}")]),
                None => read(&self.root, &path),
            };
            if self.historical_output(&path, &bytes, false) {
                eprintln!("line-set historical output base={base:?} {path:?}");
                continue;
            }
            // AGE244 explicitly used git grep -I: binary inputs are admitted,
            // but do not contribute line-set rows. Text decoding never fails open.
            if bytes.iter().take(8000).any(|b| *b == 0) {
                continue;
            }
            let text = String::from_utf8(bytes)
                .unwrap_or_else(|e| panic!("unsupported text {path:?}: {e}"));
            for line in text.lines().filter(|line| occurrences(line, pattern) != 0) {
                eprintln!("source raw row base={base:?} {path}:{line}");
                if vocabulary_occurrences(&path, line, pattern) != 0 {
                    hits.insert(format!("{path}:{line}"));
                }
            }
        }
        eprintln!("source line-set base={base:?} rows={}", hits.len());
        hits
    }
}

// AGE323 generation 2: exact-existing semantic residues authorized at
// 78e01bc65741b268c314cab504ef693f364e8b4c. Pins below are source SHA-256
// plus 1-based physical rows from membership-correction-2/source-boundary.json.
// Six persisted reason roles: 7 physical rows / 8 occurrences. Endpoint
// policy/storage: 12 unique rows / 15 physical rows and occurrences.
// Hashes reference honest source bytes; raw scans still print every literal.
// New files, same-role additions, relocated or mutated bytes get no allowance.
struct ExactResidue {
    sha256: String,
    lines: BTreeSet<usize>,
}

fn exact_residue_policy() -> BTreeMap<String, ExactResidue> {
    let entries: [(&str, &str, &[usize]); 5] = [
        (
            "crates/oulipoly-runtime/src/session_provider.rs",
            "f9de5001fe1fadb0b2a5f6cc937f7066138ea1452dddce1e11899bd29b6d59d0",
            &[53],
        ),
        (
            "crates/oulipoly-runtime/src/session_provider/worker.rs",
            "821283ea59003f6ac4167372ac60ad5eabded7c38a7ccfba2d3f3fc0225e315e",
            &[200, 298, 300],
        ),
        (
            "crates/oulipoly-runtime/tests/session_import.rs",
            "0646bfe92c802e3163d2b09a56ee49ab6c66cab1f3ea81bb15b602eb100fee56",
            &[712],
        ),
        (
            "crates/oulipoly-state/src/db/session_turn_pages.rs",
            "73d6c9bdd5ce432a8caab79f428ef734e99ba7ead1913b415182ef3001bd3277",
            &[188, 782],
        ),
        (
            "src-tauri/tests/fixtures/provider-authority-endpoint.py",
            "63bf6a522f937cb0edc8e87c59077aebfa9b26892e996fd0acee88acec14662b",
            &[
                291, 294, 301, 302, 437, 438, 439, 440, 509, 518, 808, 810, 893, 899, 911,
            ],
        ),
    ];
    entries
        .into_iter()
        .map(|(path, sha256, lines)| {
            (
                path.to_owned(),
                ExactResidue {
                    sha256: sha256.to_owned(),
                    lines: lines.iter().copied().collect(),
                },
            )
        })
        .collect()
}

// Parse only new-side hunk coordinates, not filenames or payload-like headers.
fn added_rows(diff: &str) -> Vec<(usize, String)> {
    let mut number = None;
    let mut rows = Vec::new();
    for line in diff.lines() {
        if line.starts_with("diff --git ") {
            number = None;
        }
        if line.starts_with("@@ ") {
            let range = line.split_whitespace().nth(2).expect("new hunk range");
            let start = range
                .strip_prefix('+')
                .expect("new hunk sign")
                .split(',')
                .next()
                .unwrap();
            number = Some(start.parse::<usize>().expect("new hunk coordinate"));
            continue;
        }
        let Some(current) = number.as_mut() else {
            continue;
        };
        if let Some(text) = line.strip_prefix('+') {
            rows.push((*current, text.to_owned()));
            *current += 1;
        } else if line.starts_with(' ') {
            *current += 1;
        }
    }
    rows
}

// Positive historical authority: ba61435d6b68674901427da4cf61d7def10a6b39
// planning/s10-gate/contracts/plk.contract.md assigns generated moveout planning
// exclusions to all three guards. e5e41653f8339d3a4c3dc0f8d5c7e12d26686d6b
// and 4d8a0815c23c7d378f2c123481c92672885fc784 extend ONLY line/delta
// projection to the sweep. Resolve their concrete retained material at the
// existing comparison identity, NOT future files matching a directory glob.
fn historical_outputs(root: &Path) -> BTreeMap<String, HistoricalOutput> {
    const SNAPSHOT: &str = "f0844a90d73c9196fc6fe53d510caf4d2c56c076";
    let selected = paths(&git(
        root,
        &["ls-tree", "-r", "--name-only", "-z", SNAPSHOT],
    ));
    let mut outputs = BTreeMap::new();
    for path in selected {
        let Some(full_excluded) = historical_planning_scope(&path) else {
            continue;
        };
        let bytes = git(root, &["show", &format!("{SNAPSHOT}:{path}")]);
        outputs.insert(
            path,
            HistoricalOutput {
                sha256: digest(&bytes),
                full_excluded,
            },
        );
    }
    outputs
}

fn historical_planning_scope(path: &str) -> Option<bool> {
    let rest = path.strip_prefix("planning/")?;
    let (directory, _) = rest.split_once('/')?;
    match directory {
        "code-quality-sweep" => Some(false),
        "wu-e" | "opencode-contract" | "s10-moveout" => Some(true),
        _ if directory.ends_with("-gate") => Some(true),
        _ => None,
    }
}

fn assert_search(output: &Output) {
    assert!(
        output.status.success() || output.status.code() == Some(1),
        "source search failed: {output:?}"
    );
}

fn vocabulary_occurrences(path: &str, line: &str, pattern: &str) -> usize {
    occurrences(&provider_wire_policy::vocabulary_text(path, line), pattern)
}

fn occurrences(text: &str, pattern: &str) -> usize {
    pattern
        .split('|')
        .map(|token| text.matches(token).count())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        std::fs::write(dir.path().join("input.txt"), "ordinary source\n").unwrap();
        git(dir.path(), &["add", "input.txt"]);
        git(
            dir.path(),
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-qm",
                "fixture",
            ],
        );
        dir
    }

    fn receipt(root: &Path, external: &Path, output: Option<&str>) -> PathBuf {
        let producer = external.join("producer.json");
        let entries: Vec<_> = paths(&git(
            root,
            &[
                "ls-files",
                "--cached",
                "--others",
                "--exclude-standard",
                "-z",
            ],
        ))
        .into_iter()
        .map(|path| Entry {
            sha256: digest(&read(root, &path)),
            output_producer: (Some(path.as_str()) == output).then_some(0),
            path,
        })
        .collect();
        let inventory: BTreeMap<_, _> = entries
            .iter()
            .filter(|e| e.output_producer.is_some())
            .map(|e| (e.path.clone(), e.sha256.clone()))
            .collect();
        std::fs::write(&producer, serde_json::to_vec(&inventory).unwrap()).unwrap();
        let receipt = Receipt {
            version: 1,
            invocation: "synthetic-1".into(),
            repository: root.canonicalize().unwrap(),
            head: String::from_utf8(git(root, &["rev-parse", "HEAD"]))
                .unwrap()
                .trim()
                .into(),
            index_sha256: digest(&git(root, &["ls-files", "--stage", "-z"])),
            producers: vec![Reference {
                sha256: digest(&std::fs::read(&producer).unwrap()),
                path: producer,
            }],
            entries,
        };
        let path = external.join("receipt.json");
        std::fs::write(&path, serde_json::to_vec(&receipt).unwrap()).unwrap();
        path
    }

    #[test]
    fn membership_rejects_unaccounted_stale_and_malformed_inputs() {
        let root = fixture();
        let external = tempfile::tempdir().unwrap();
        let path = receipt(root.path(), external.path(), None);
        SourceSet::load_input(root.path(), Some(&path), Some("synthetic-1"));
        assert!(
            std::panic::catch_unwind(|| SourceSet::load_input(
                root.path(),
                Some(&path),
                Some("other")
            ))
            .is_err()
        );
        std::fs::write(root.path().join("new.md"), "new source").unwrap();
        assert!(
            std::panic::catch_unwind(|| SourceSet::load_input(
                root.path(),
                Some(&path),
                Some("synthetic-1")
            ))
            .is_err()
        );
        std::fs::remove_file(root.path().join("new.md")).unwrap();
        std::fs::write(root.path().join("input.txt"), "changed").unwrap();
        assert!(
            std::panic::catch_unwind(|| SourceSet::load_input(
                root.path(),
                Some(&path),
                Some("synthetic-1")
            ))
            .is_err()
        );
        std::fs::write(&path, "{bad receipt").unwrap();
        assert!(
            std::panic::catch_unwind(|| SourceSet::load_input(
                root.path(),
                Some(&path),
                Some("synthetic-1")
            ))
            .is_err()
        );
    }

    #[test]
    fn membership_rejects_tracked_outputs_and_preserves_attested_bytes() {
        let root = fixture();
        let external = tempfile::tempdir().unwrap();
        let path = receipt(root.path(), external.path(), Some("input.txt"));
        assert!(
            std::panic::catch_unwind(|| SourceSet::load_input(
                root.path(),
                Some(&path),
                Some("synthetic-1")
            ))
            .is_err()
        );
        for bytes in [
            b"needle generated log".as_slice(),
            b"\xff\0needle binary output".as_slice(),
        ] {
            std::fs::write(root.path().join("output.bin"), bytes).unwrap();
            let path = receipt(root.path(), external.path(), Some("output.bin"));
            let sources = SourceSet::load_input(root.path(), Some(&path), Some("synthetic-1"));
            assert_eq!(sources.full_occurrences("needle"), 0);
            assert_eq!(sources.added_occurrences(None, "needle"), 0);
            assert_eq!(
                std::fs::read(root.path().join("output.bin")).unwrap(),
                bytes
            );
        }
    }

    #[test]
    fn membership_guards_all_product_kinds_even_under_output_like_names() {
        let root = fixture();
        let base = String::from_utf8(git(root.path(), &["rev-parse", "HEAD"])).unwrap();
        for name in [
            "source.rs",
            "script.sh",
            "document.md",
            "fixture.txt",
            "config.toml",
            "evidence-like/tracked.rs",
            "target-like/script.py",
        ] {
            let path = root.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "needle\n").unwrap();
            let sources = SourceSet::load_input(root.path(), None, None);
            assert!(sources.full_occurrences("needle") > 0);
            assert!(sources.added_occurrences(None, "needle") > 0);
            assert!(!sources.line_set(None, "needle").is_empty());
            git(root.path(), &["add", name]);
            let sources = SourceSet::load_input(root.path(), None, None);
            assert!(sources.full_occurrences("needle") > 0);
            assert!(sources.added_occurrences(Some(base.trim()), "needle") > 0);
        }
    }

    #[test]
    fn membership_rejects_missing_producer_inventory_binding_and_nontext_product() {
        let root = fixture();
        let external = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("output.bin"), b"\xff\0needle").unwrap();
        let sources = SourceSet::load_input(root.path(), None, None);
        assert!(std::panic::catch_unwind(|| sources.added_occurrences(None, "needle")).is_err());
        let path = receipt(root.path(), external.path(), Some("output.bin"));
        let mut value: Receipt = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        std::fs::write(&value.producers[0].path, "{}").unwrap();
        value.producers[0].sha256 = digest(b"{}");
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(
            std::panic::catch_unwind(|| SourceSet::load_input(
                root.path(),
                Some(&path),
                Some("synthetic-1")
            ))
            .is_err()
        );
    }
    fn historical_fixture(root: &Path) -> SourceSet {
        let mut sources = SourceSet::load_input(root, None, None);
        for (path, full_excluded) in [
            ("planning/old-gate/report.md", true),
            ("planning/code-quality-sweep/report.md", false),
        ] {
            sources.historical.insert(
                path.into(),
                HistoricalOutput {
                    sha256: digest(b"needle\n"),
                    full_excluded,
                },
            );
        }
        sources
    }

    #[test]
    fn membership_preserves_metric_specific_ignore_and_index_semantics() {
        let root = fixture();
        let base = String::from_utf8(git(root.path(), &["rev-parse", "HEAD"])).unwrap();
        std::fs::write(root.path().join(".gitignore"), "ignored*\n").unwrap();
        std::fs::write(root.path().join("ignored-untracked"), b"\xffneedle").unwrap();
        let sources = SourceSet::load_input(root.path(), None, None);
        assert!(!sources.sources.contains("ignored-untracked"));
        assert_eq!(sources.full_occurrences("needle"), 0);
        assert_eq!(sources.added_occurrences(None, "needle"), 0);
        std::fs::write(root.path().join("ignored-tracked"), "needle\n").unwrap();
        git(root.path(), &["add", "-f", "ignored-tracked"]);
        let sources = SourceSet::load_input(root.path(), None, None);
        assert!(sources.sources.contains("ignored-tracked"));
        assert_eq!(sources.full_occurrences("needle"), 0);
        assert_eq!(sources.line_set(None, "needle").len(), 1);
        assert_eq!(sources.added_occurrences(None, "needle"), 0);
        assert_eq!(sources.added_occurrences(Some(base.trim()), "needle"), 1);
        std::fs::write(root.path().join("ignored-tracked"), "needle needle\n").unwrap();
        let sources = SourceSet::load_input(root.path(), None, None);
        assert_eq!(sources.added_occurrences(None, "needle"), 2);
    }

    #[test]
    fn membership_projects_only_exact_historical_material_per_metric() {
        let root = fixture();
        for path in [
            "planning/old-gate/report.md",
            "planning/code-quality-sweep/report.md",
        ] {
            let full = root.path().join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, "needle\n").unwrap();
        }
        git(root.path(), &["add", "."]);
        git(
            root.path(),
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-qm",
                "historical outputs",
            ],
        );
        let base = String::from_utf8(git(root.path(), &["rev-parse", "HEAD"])).unwrap();
        let sources = historical_fixture(root.path());
        // The sweep never acquired a full-cap exemption.
        assert_eq!(sources.full_occurrences("needle"), 1);
        assert!(sources.line_set(Some(base.trim()), "needle").is_empty());
        assert!(sources.line_set(None, "needle").is_empty());
        assert_eq!(sources.added_occurrences(Some(base.trim()), "needle"), 0);
        assert_eq!(sources.added_occurrences(None, "needle"), 0);
        // New source cannot inherit an output classification from its name.
        std::fs::write(root.path().join("planning/old-gate/source.rs"), "needle\n").unwrap();
        let sources = historical_fixture(root.path());
        assert_eq!(sources.full_occurrences("needle"), 2);
        assert_eq!(sources.added_occurrences(None, "needle"), 1);
        assert_eq!(sources.line_set(None, "needle").len(), 1);
        git(root.path(), &["add", "planning/old-gate/source.rs"]);
        assert_eq!(
            historical_fixture(root.path()).added_occurrences(Some(base.trim()), "needle"),
            1
        );
        // Replacing a historical file invalidates its exact-byte classification.
        std::fs::write(
            root.path().join("planning/old-gate/report.md"),
            "needle needle\n",
        )
        .unwrap();
        let sources = historical_fixture(root.path());
        assert_eq!(sources.full_occurrences("needle"), 4);
        assert_eq!(sources.added_occurrences(Some(base.trim()), "needle"), 3);
        assert_eq!(sources.added_occurrences(None, "needle"), 2);
        assert_eq!(sources.line_set(None, "needle").len(), 2);
        assert!(sources.line_set(Some(base.trim()), "needle").is_empty());
    }

    #[test]
    fn membership_missing_source_and_incomplete_inventory_fail_closed() {
        let root = fixture();
        let external = tempfile::tempdir().unwrap();
        let path = receipt(root.path(), external.path(), None);
        let mut value: Receipt = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        value.entries.clear();
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(
            std::panic::catch_unwind(|| SourceSet::load_input(
                root.path(),
                Some(&path),
                Some("synthetic-1")
            ))
            .is_err()
        );
        let sources = SourceSet::load_input(root.path(), None, None);
        std::fs::remove_file(root.path().join("input.txt")).unwrap();
        assert!(std::panic::catch_unwind(|| sources.full_occurrences("needle")).is_err());
        assert!(std::panic::catch_unwind(|| sources.line_set(None, "needle")).is_err());
    }
    #[test]
    fn wire_projection_reaches_active_metrics_without_membership_or_residue_amnesty() {
        let root = fixture();
        let base = String::from_utf8(git(root.path(), &["rev-parse", "HEAD"])).unwrap();
        let location = git(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            &["rev-parse", "--show-toplevel"],
        );
        let repository = PathBuf::from(String::from_utf8(location).unwrap().trim());
        let path = "crates/oulipoly-runtime/src/session_provider/turns_source_io.rs";
        let source = std::fs::read_to_string(repository.join(path)).unwrap();
        let declaration = source
            .lines()
            .find(|line| line.starts_with("const DECLARATION:"))
            .unwrap();
        let wire = declaration.split('"').nth(1).unwrap();
        let denied = wire.split('_').next().unwrap();
        let file = root.path().join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, format!("{declaration}\n")).unwrap();
        // Exact protocol syntax is admitted even before staging, but remains
        // source membership, not an output receipt or exact-residue allowance.
        let sources = SourceSet::load_input(root.path(), None, None);
        assert!(sources.sources.contains(path));
        assert_eq!(sources.full_occurrences(denied), 0);
        assert_eq!(sources.added_occurrences(None, denied), 0);
        assert!(sources.line_set(None, denied).is_empty());
        git(root.path(), &["add", path]);
        let sources = SourceSet::load_input(root.path(), None, None);
        assert_eq!(sources.added_occurrences(Some(base.trim()), denied), 0);
        // Same line suffix, routing use and unapproved path cannot inherit it.
        for text in [
            format!("{declaration} route({wire:?});\n"),
            format!("if provider == {wire:?} {{ route(); }}\n"),
            format!("{declaration}\nlet unrelated = {denied:?};\n"),
        ] {
            std::fs::write(&file, &text).unwrap();
            let sources = SourceSet::load_input(root.path(), None, None);
            assert!(sources.full_occurrences(denied) > 0);
            assert!(sources.added_occurrences(None, denied) > 0);
            assert!(sources.added_occurrences(Some(base.trim()), denied) > 0);
            assert!(
                !sources
                    .unapproved_new_rows(&BTreeSet::new(), &sources.line_set(None, denied))
                    .is_empty()
            );
        }
        std::fs::write(&file, format!("{declaration}\n")).unwrap();
        std::fs::write(root.path().join("unrelated.rs"), format!("{declaration}\n")).unwrap();
        let sources = SourceSet::load_input(root.path(), None, None);
        assert_eq!(sources.full_occurrences(denied), 1);
        assert_eq!(sources.added_occurrences(None, denied), 1);
        assert_eq!(sources.added_occurrences(Some(base.trim()), denied), 1);
        assert_eq!(sources.line_set(None, denied).len(), 1);
        // Exact residues still deduct only their physical pinned rows, while
        // wire admission cannot subsidize an unrelated row in either metric.
        stage_roles(root.path());
        let pattern = format!("needle|{denied}");
        let sources = residue_fixture(root.path());
        assert_eq!(sources.full_occurrences(&pattern), 6);
        assert_eq!(sources.added_occurrences(Some(base.trim()), &pattern), 2);
        assert_eq!(
            sources
                .unapproved_new_rows(&BTreeSet::new(), &sources.line_set(None, &pattern))
                .len(),
            2
        );
        std::fs::write(
            root.path().join("roles.txt"),
            "needle needle\nneedle\nneedle\nother needle\nneedle\n",
        )
        .unwrap();
        let sources = residue_fixture(root.path());
        assert_eq!(
            sources.approved_added_occurrences(Some(base.trim()), &pattern),
            0
        );
        assert_eq!(sources.added_occurrences(Some(base.trim()), &pattern), 7);
    }

    fn residue_fixture(root: &Path) -> SourceSet {
        let mut sources = SourceSet::load_input(root, None, None);
        sources.residues.insert(
            "roles.txt".into(),
            ExactResidue {
                sha256: digest(b"needle needle\nneedle\nneedle\nother needle\n"),
                lines: [1, 2, 3].into_iter().collect(),
            },
        );
        sources
    }

    fn stage_roles(root: &Path) {
        std::fs::write(
            root.join("roles.txt"),
            b"needle needle\nneedle\nneedle\nother needle\n",
        )
        .unwrap();
        git(root, &["add", "roles.txt"]);
    }

    #[test]
    fn exact_residues_deduct_only_listed_added_rows_and_keep_raw_metrics() {
        let root = fixture();
        let base = String::from_utf8(git(root.path(), &["rev-parse", "HEAD"])).unwrap();
        stage_roles(root.path());
        let sources = residue_fixture(root.path());
        assert_eq!(sources.full_occurrences("needle"), 5);
        let current = sources.line_set(None, "needle");
        assert_eq!(current.len(), 3);
        assert_eq!(
            sources.unapproved_new_rows(&BTreeSet::new(), &current),
            ["roles.txt:other needle"]
        );
        assert_eq!(sources.added_occurrences(Some(base.trim()), "needle"), 1);
        // A clean working copy is not a committed-range proof. No fixed budget
        // is subtracted when the selected index comparison has no additions.
        assert_eq!(sources.added_occurrences(None, "needle"), 0);
        assert_eq!(sources.approved_added_occurrences(None, "needle"), 0);
        // Already-present set rows must not create deductions either.
        assert!(sources.unapproved_new_rows(&current, &current).is_empty());
    }

    #[test]
    fn exact_residues_revoke_on_same_role_additions_and_any_byte_mutation() {
        let root = fixture();
        let base = String::from_utf8(git(root.path(), &["rev-parse", "HEAD"])).unwrap();
        stage_roles(root.path());
        for bytes in [
            "needle needle\nneedle\nneedle\nother needle\nneedle\n",
            "needle needle\nneedle\nneedle\nother needle\nnew needle\n",
            "needle needle\nneedle \nneedle\nother needle\n",
            "needle needle\r\nneedle\r\nneedle\r\nother needle\r\n",
            "needle needle\nneedle\nneedle\nother needle",
            "ordinary\nneedle needle\nneedle\nneedle\nother needle\n",
            "needle needle\nneedle\nneedle\nother needle\nordinary\n",
        ] {
            std::fs::write(root.path().join("roles.txt"), bytes).unwrap();
            let sources = residue_fixture(root.path());
            let raw = occurrences(bytes, "needle");
            assert_eq!(sources.full_occurrences("needle"), raw);
            assert_eq!(
                sources.approved_added_occurrences(Some(base.trim()), "needle"),
                0
            );
            assert_eq!(sources.added_occurrences(Some(base.trim()), "needle"), raw);
            assert_eq!(
                sources.added_occurrences(None, "needle"),
                SourceSet::load_input(root.path(), None, None).added_occurrences(None, "needle")
            );
            let rows = sources.line_set(None, "needle");
            assert_eq!(
                sources.unapproved_new_rows(&BTreeSet::new(), &rows).len(),
                rows.len()
            );
        }
    }

    #[test]
    fn exact_residues_reject_path_widening_copies_and_untracked_replacements() {
        let root = fixture();
        let base = String::from_utf8(git(root.path(), &["rev-parse", "HEAD"])).unwrap();
        stage_roles(root.path());
        // Keep the genuine approved source alongside each copy: its allowance
        // cannot subsidize any additional path, staged or untracked.
        for name in ["roles.txt.extra", "Roles.txt", "nested/roles.txt"] {
            let path = root.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, read(root.path(), "roles.txt")).unwrap();
            let sources = residue_fixture(root.path());
            assert!(sources.added_occurrences(Some(base.trim()), "needle") >= 6);
            assert!(sources.added_occurrences(None, "needle") >= 5);
            let rows = sources.line_set(None, "needle");
            assert!(
                sources
                    .unapproved_new_rows(&BTreeSet::new(), &rows)
                    .contains(&format!("{name}:needle"))
            );
            git(root.path(), &["add", name]);
            assert!(
                residue_fixture(root.path()).added_occurrences(Some(base.trim()), "needle") >= 6
            );
        }
        git(root.path(), &["rm", "--cached", "roles.txt"]);
        let sources = residue_fixture(root.path());
        assert_eq!(
            sources.approved_added_occurrences(Some(base.trim()), "needle"),
            0
        );
        assert_eq!(sources.added_occurrences(None, "needle"), 5);
        // Relocation never imports the original path's allowance.
        std::fs::rename(
            root.path().join("roles.txt"),
            root.path().join("relocated.txt"),
        )
        .unwrap();
        let sources = residue_fixture(root.path());
        assert_eq!(
            sources.approved_added_occurrences(Some(base.trim()), "needle"),
            0
        );
        assert!(
            sources
                .unapproved_new_rows(&BTreeSet::new(), &sources.line_set(None, "needle"))
                .contains(&"relocated.txt:needle".into())
        );
    }

    #[test]
    fn exact_residues_parse_physical_additions_not_deletions_or_headers() {
        let diff = "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1,2 +1,3 @@\n-old\n+++payload\n+needle\n context\n@@ -9 +10,0 @@\n-needle\n@@ -12 +13 @@\n-old\n+needle needle\n\\ No newline at end of file\n";
        assert_eq!(
            added_rows(diff),
            [
                (1, "++payload".into()),
                (2, "needle".into()),
                (13, "needle needle".into())
            ]
        );
    }

    #[test]
    fn exact_residue_pins_match_frozen_source_without_expanding_row_scope() {
        // Read immutable Git objects, not the mutable worktree, to validate the
        // hand-encoded authority independently of the eligibility fence.
        let location = git(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            &["rev-parse", "--show-toplevel"],
        );
        let root = PathBuf::from(String::from_utf8(location).unwrap().trim());
        let policy = exact_residue_policy();
        assert_eq!(policy.len(), 5);
        let mut physical = 0;
        let mut unique = BTreeSet::new();
        for (path, entry) in policy {
            let bytes = git(
                &root,
                &[
                    "show",
                    &format!("78e01bc65741b268c314cab504ef693f364e8b4c:{path}"),
                ],
            );
            assert_eq!(digest(&bytes), entry.sha256);
            let text = String::from_utf8(bytes).unwrap();
            let lines: Vec<_> = text.lines().collect();
            for number in entry.lines {
                physical += 1;
                unique.insert(format!("{path}:{}", lines[number - 1]));
            }
        }
        assert_eq!(physical, 22);
        assert_eq!(unique.len(), 18);
    }
}
