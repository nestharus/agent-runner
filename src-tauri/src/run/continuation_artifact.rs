//! ## Declared roles
//!
//! `orchestration`, `validator`, `accessor`, `mapper`, `formatter`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/run/continuation_artifact.rs
//!     role: adapter
//!     Translates:
//!       - oulipoly-runtime-continuation-artifact-source-contract
//!       - planning-root-filesystem-evidence-contract
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use oulipoly_runtime::fresh_continuation::{
    ArtifactIdentity, ContinuationArtifactSource, ContinuationBlock, ContinuationBlockKind,
};

pub(super) struct FilesystemContinuationArtifactSource {
    planning_root: PathBuf,
}

impl FilesystemContinuationArtifactSource {
    pub(super) fn new(planning_root: &Path) -> Self {
        Self {
            planning_root: planning_root.to_path_buf(),
        }
    }
}

impl ContinuationArtifactSource for FilesystemContinuationArtifactSource {
    fn read(&mut self, artifact: &ArtifactIdentity) -> Result<Vec<u8>, ContinuationBlock> {
        let canonical_artifact = canonical_artifact_path(&self.planning_root, &artifact.path)?;
        read_artifact(&canonical_artifact).map_err(invalid_evidence)
    }
}

fn canonical_artifact_path(
    planning_root: &Path,
    artifact_path: &Path,
) -> Result<PathBuf, ContinuationBlock> {
    let canonical_root = canonical_path(planning_root).map_err(invalid_evidence)?;
    let canonical_artifact = canonical_path(artifact_path).map_err(invalid_evidence)?;
    validate_artifact_containment(&canonical_root, canonical_artifact)
}

fn canonical_path(path: &Path) -> Result<PathBuf, std::io::Error> {
    fs::canonicalize(path)
}

fn validate_artifact_containment(
    canonical_root: &Path,
    canonical_artifact: PathBuf,
) -> Result<PathBuf, ContinuationBlock> {
    if !canonical_artifact.starts_with(canonical_root) || canonical_artifact == canonical_root {
        return Err(ContinuationBlock {
            kind: ContinuationBlockKind::InvalidEvidence,
            message: "Continuation artifact resolves outside the planning root".to_string(),
        });
    }
    Ok(canonical_artifact)
}

fn read_artifact(path: &Path) -> Result<Vec<u8>, std::io::Error> {
    fs::read(path)
}

fn invalid_evidence(error: impl std::fmt::Display) -> ContinuationBlock {
    ContinuationBlock {
        kind: ContinuationBlockKind::InvalidEvidence,
        message: error.to_string(),
    }
}
