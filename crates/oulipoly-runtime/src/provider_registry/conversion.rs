use super::ProviderRegistryError;
use oulipoly_config::provider_implementation_ref::{
    ProviderImplementationFlavor, ProviderImplementationRef,
};
use oulipoly_provider::resolver::{ProviderArtifactRef, RuntimeDisabledArtifact};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeProviderArtifact {
    Enabled(ProviderArtifactRef),
    RuntimeDisabled(RuntimeDisabledArtifact),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactKind {
    Path,
    Binary,
    Script,
    Crate,
}

impl RuntimeProviderArtifact {
    pub fn kind(&self) -> ArtifactKind {
        match self {
            Self::Enabled(ProviderArtifactRef::Path { .. }) => ArtifactKind::Path,
            Self::Enabled(ProviderArtifactRef::Binary { .. }) => ArtifactKind::Binary,
            Self::Enabled(ProviderArtifactRef::Script { .. }) => ArtifactKind::Script,
            Self::RuntimeDisabled(RuntimeDisabledArtifact::Crate { .. }) => ArtifactKind::Crate,
        }
    }
}

pub fn convert_ref(
    provider_ref: &ProviderImplementationRef,
) -> Result<RuntimeProviderArtifact, ProviderRegistryError> {
    let flavor = provider_ref
        .flavor()
        .map_err(|source| ProviderRegistryError::InvalidImplementationRef { source })?;
    match flavor {
        ProviderImplementationFlavor::Path => Ok(RuntimeProviderArtifact::Enabled(
            ProviderArtifactRef::Path {
                path: PathBuf::from(provider_ref.path.as_deref().unwrap_or_default()),
            },
        )),
        ProviderImplementationFlavor::Binary => Ok(RuntimeProviderArtifact::Enabled(
            ProviderArtifactRef::Binary {
                name: provider_ref.binary.clone().unwrap_or_default(),
            },
        )),
        ProviderImplementationFlavor::Script => Ok(RuntimeProviderArtifact::Enabled(
            ProviderArtifactRef::Script {
                path: PathBuf::from(provider_ref.script.as_deref().unwrap_or_default()),
            },
        )),
        ProviderImplementationFlavor::Crate => Ok(RuntimeProviderArtifact::RuntimeDisabled(
            RuntimeDisabledArtifact::Crate {
                crate_name: provider_ref.crate_name.clone().unwrap_or_default(),
                version: provider_ref.version.clone(),
            },
        )),
    }
}
