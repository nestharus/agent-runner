//! Complete source membership for the vocabulary guards; receipts are external
//! invocation inputs, never repository policy. Declared roles: validator, accessor.
#![allow(dead_code)] // Each integration target uses a different metric adapter.

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
    // Do not follow source symlinks outside the admitted input set.
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
        Self::load_input(root, receipt.as_deref(), invocation.as_deref())
    }

    fn load_input(root: &Path, receipt: Option<&Path>, invocation: Option<&str>) -> Self {
        let root = root.canonicalize().expect("canonical repository");
        let tracked = paths(&git(&root, &["ls-files", "--cached", "-z"]));
        // Deliberately no --exclude-standard: ignore rules cannot exempt product.
        let untracked = paths(&git(&root, &["ls-files", "--others", "-z"]));
        let candidates: BTreeSet<_> = tracked.union(&untracked).cloned().collect();
        let Some(receipt_path) = receipt else {
            assert!(
                invocation.is_none(),
                "invocation without membership receipt"
            );
            return Self {
                root,
                sources: candidates,
                untracked,
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
        }
    }

    pub fn full_occurrences(&self, pattern: &str) -> usize {
        let mut count = 0;
        for path in &self.sources {
            // Explicit paths bypass ignore/hidden traversal heuristics; retain rg's
            // case-sensitive -o occurrence metric and binary handling.
            read(&self.root, path);
            let output = Command::new("rg")
                .current_dir(&self.root)
                .args(["--no-config", "--with-filename", "-o", pattern, "--", path])
                .output()
                .expect("rg source occurrences");
            assert_search(&output);
            let text = String::from_utf8(output.stdout).expect("non-UTF-8 occurrence output");
            eprint!("{text}");
            count += text.lines().count();
        }
        eprintln!("source full occurrences={count}");
        count
    }

    pub fn added_occurrences(&self, base: Option<&str>, pattern: &str) -> usize {
        let mut args = vec!["diff", "--no-ext-diff", "--unified=0"];
        if let Some(base) = base {
            args.push(base);
        }
        args.push("--");
        // All tracked product is admitted: do not suppress deleted/renamed paths.
        let diff = String::from_utf8(git(&self.root, &args)).expect("non-UTF-8 source diff");
        let added: Vec<_> = diff
            .lines()
            .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
            .collect();
        let tracked_count: usize = added.iter().map(|line| occurrences(line, pattern)).sum();
        for line in added.iter().filter(|line| occurrences(line, pattern) != 0) {
            eprintln!("tracked added {line}");
        }
        let mut untracked_count = 0;
        for path in &self.untracked {
            let text = String::from_utf8(read(&self.root, path)).unwrap_or_else(|e| {
                panic!("unsupported non-UTF-8 untracked product {path:?}: {e}")
            });
            for line in text.lines().filter(|line| occurrences(line, pattern) != 0) {
                eprintln!("untracked {path}:{line}");
            }
            untracked_count += occurrences(&text, pattern);
        }
        eprintln!("source added base={base:?} tracked={tracked_count} untracked={untracked_count}");
        tracked_count + untracked_count
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
            // AGE244 explicitly used git grep -I: binary inputs are admitted,
            // but do not contribute line-set rows. Text decoding never fails open.
            if bytes.iter().take(8000).any(|b| *b == 0) {
                continue;
            }
            let text = String::from_utf8(bytes)
                .unwrap_or_else(|e| panic!("unsupported text {path:?}: {e}"));
            for line in text.lines().filter(|line| occurrences(line, pattern) != 0) {
                hits.insert(format!("{path}:{line}"));
            }
        }
        eprintln!("source line-set base={base:?} rows={}", hits.len());
        hits
    }
}

fn assert_search(output: &Output) {
    assert!(
        output.status.success() || output.status.code() == Some(1),
        "source search failed: {output:?}"
    );
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
        let entries: Vec<_> = paths(&git(root, &["ls-files", "--cached", "--others", "-z"]))
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
}
