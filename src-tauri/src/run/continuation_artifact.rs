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

use std::io::Read;
use std::path::{Component, Path, PathBuf};

use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt, OpenOptionsMaybeDirExt};
use cap_std::fs::{Dir, OpenOptions};
use oulipoly_runtime::fresh_continuation::{
    ArtifactIdentity, ContinuationArtifactSource, ContinuationBlock, ContinuationBlockKind,
};

pub(super) struct FilesystemContinuationArtifactSource {
    planning_root: PathBuf,
    planning_dir: Dir,
}

impl FilesystemContinuationArtifactSource {
    pub(super) fn new(planning_root: &Path) -> Result<Self, ContinuationBlock> {
        let planning_dir = Dir::open_ambient_dir(planning_root, cap_std::ambient_authority())
            .map_err(invalid_evidence)?;
        Ok(Self {
            planning_root: planning_root.to_path_buf(),
            planning_dir,
        })
    }
}

impl ContinuationArtifactSource for FilesystemContinuationArtifactSource {
    fn read(&mut self, artifact: &ArtifactIdentity) -> Result<Vec<u8>, ContinuationBlock> {
        let relative_path = relative_artifact_path(&self.planning_root, &artifact.path)?;
        read_artifact(&self.planning_dir, relative_path).map_err(invalid_evidence)
    }
}

fn relative_artifact_path<'a>(
    planning_root: &Path,
    artifact_path: &'a Path,
) -> Result<&'a Path, ContinuationBlock> {
    let relative_path = artifact_path
        .strip_prefix(planning_root)
        .map_err(|_| invalid_artifact_path())?;
    if relative_path.as_os_str().is_empty() {
        return Err(invalid_artifact_path());
    }
    Ok(relative_path)
}

fn read_artifact(planning_dir: &Dir, relative_path: &Path) -> Result<Vec<u8>, std::io::Error> {
    let mut directory = None;
    let mut components = relative_path.components().peekable();
    while let Some(component) = components.next() {
        let Component::Normal(name) = component else {
            return Err(std::io::Error::other(
                "Continuation artifact path is not normalized",
            ));
        };
        let parent = directory.as_ref().unwrap_or(planning_dir);
        if components.peek().is_none() {
            return read_file(parent, Path::new(name));
        }
        directory = Some(open_directory(parent, Path::new(name))?);
    }
    Err(std::io::Error::other("Continuation artifact path is empty"))
}

fn open_directory(parent: &Dir, name: &Path) -> Result<Dir, std::io::Error> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .follow(FollowSymlinks::No)
        .maybe_dir(true);
    let file = parent.open_with(name, &options)?;
    if !file.metadata()?.is_dir() {
        return Err(std::io::Error::other(
            "Continuation artifact parent is not a directory",
        ));
    }
    Ok(Dir::from_std_file(file.into_std()))
}

fn read_file(parent: &Dir, name: &Path) -> Result<Vec<u8>, std::io::Error> {
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    let mut file = parent.open_with(name, &options)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other(
            "Continuation artifact is not a regular file",
        ));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn invalid_artifact_path() -> ContinuationBlock {
    ContinuationBlock {
        kind: ContinuationBlockKind::InvalidEvidence,
        message: "Continuation artifact is outside the planning root".to_string(),
    }
}

fn invalid_evidence(error: impl std::fmt::Display) -> ContinuationBlock {
    ContinuationBlock {
        kind: ContinuationBlockKind::InvalidEvidence,
        message: error.to_string(),
    }
}
