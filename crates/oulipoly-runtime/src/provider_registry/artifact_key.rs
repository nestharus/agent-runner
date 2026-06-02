use super::RuntimeProviderArtifact;
use oulipoly_provider::resolver::{ProviderArtifactRef, RuntimeDisabledArtifact};
use std::path::Path;

pub type ArtifactKey = String;

pub fn artifact_key(artifact: &RuntimeProviderArtifact) -> ArtifactKey {
    match artifact {
        RuntimeProviderArtifact::Enabled(ProviderArtifactRef::Path { path }) => {
            format!("path:{}", path_key(path))
        }
        RuntimeProviderArtifact::Enabled(ProviderArtifactRef::Binary { name }) => {
            format!("binary:{name}")
        }
        RuntimeProviderArtifact::Enabled(ProviderArtifactRef::Script { path }) => {
            format!("script:{}", path_key(path))
        }
        RuntimeProviderArtifact::RuntimeDisabled(RuntimeDisabledArtifact::Crate {
            crate_name,
            version,
        }) => {
            let version = version.as_deref().unwrap_or("");
            format!("crate:{crate_name}@{version}")
        }
    }
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
