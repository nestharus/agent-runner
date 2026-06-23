//! Declared role: mapper

use oulipoly_config::ProviderImplementationRef;
use oulipoly_provider::resolver::ProviderArtifactRef;
use std::path::PathBuf;

pub(crate) fn provider_artifact_from_ref(
    artifact: &ProviderImplementationRef,
) -> Result<ProviderArtifactRef, String> {
    artifact.validate().map_err(|error| error.to_string())?;
    if let Some(path) = &artifact.path {
        Ok(ProviderArtifactRef::Path {
            path: PathBuf::from(path),
        })
    } else if let Some(binary) = &artifact.binary {
        Ok(ProviderArtifactRef::Binary {
            name: binary.clone(),
        })
    } else if let Some(script) = &artifact.script {
        Ok(ProviderArtifactRef::Script {
            path: PathBuf::from(script),
        })
    } else {
        Err("unsupported setup brain artifact flavor".to_string())
    }
}
