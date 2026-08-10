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
        let canonical_root = fs::canonicalize(&self.planning_root).map_err(invalid_evidence)?;
        let canonical_artifact = fs::canonicalize(&artifact.path).map_err(invalid_evidence)?;
        if !canonical_artifact.starts_with(&canonical_root) || canonical_artifact == canonical_root
        {
            return Err(ContinuationBlock {
                kind: ContinuationBlockKind::InvalidEvidence,
                message: "Continuation artifact resolves outside the planning root".to_string(),
            });
        }
        fs::read(&canonical_artifact).map_err(|error| ContinuationBlock {
            kind: ContinuationBlockKind::InvalidEvidence,
            message: format!(
                "Failed to read continuation artifact {}: {error}",
                artifact.path.display()
            ),
        })
    }
}

fn invalid_evidence(error: impl std::fmt::Display) -> ContinuationBlock {
    ContinuationBlock {
        kind: ContinuationBlockKind::InvalidEvidence,
        message: error.to_string(),
    }
}
