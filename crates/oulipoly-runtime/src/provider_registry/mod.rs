mod artifact_key;
mod cache;
mod client_factory;
mod conversion;
mod describe;
mod error;
mod handle;
mod options;

use artifact_key::{ArtifactKey, artifact_key};
use cache::DescribeCache;
pub(crate) use describe::DescribeHostOptions;
use describe::describe_provider;
use oulipoly_config::{ModelConfig, provider_implementation_ref::ProviderImplementationRef};
use oulipoly_provider::generated::DescribeResult;
use std::collections::{BTreeMap, HashMap};

pub use client_factory::ProviderClientFactory;
pub use conversion::{ArtifactKind, RuntimeProviderArtifact};
pub use error::ProviderRegistryError;
pub use handle::ProviderRegistryHandle;
pub use options::ProviderRegistryOptions;

// ## Adapter declarations
//
// adapter_declarations:
//   - component: oulipoly-runtime::provider_registry
//     role: adapter
//     Translates:
//       - oulipoly_config::ProviderImplementationRef -> runtime provider artifact inventory
//       - oulipoly_provider resolver/client/generated/error contracts -> registry result/error types
//       - artifact-bound provider client API -> model-keyed registry lookup API
//
// ## Intrinsic-surface declarations
//
// intrinsic_surface_declarations:
//   - component: oulipoly-runtime::provider_registry
//     Domain: host-side provider-contract adaptation
//     Owns:
//       - implementation-ref conversion
//       - artifact keying and deduplication
//       - in-process describe cache
//       - describe request orchestration and error mapping
//       - registry root to internal helper module coordination

#[derive(Debug)]
pub struct ProviderRegistry {
    artifacts: BTreeMap<ArtifactKey, RuntimeProviderArtifact>,
    model_artifacts: HashMap<String, ArtifactKey>,
    cache: DescribeCache,
    client_factory: ProviderClientFactory,
    host_options: DescribeHostOptions,
}

#[derive(Debug)]
struct ArtifactInventory {
    artifacts: BTreeMap<ArtifactKey, RuntimeProviderArtifact>,
    model_artifacts: HashMap<String, ArtifactKey>,
}

impl ProviderRegistry {
    pub fn from_model_configs(
        models: &[ModelConfig],
        options: ProviderRegistryOptions,
    ) -> Result<Self, ProviderRegistryError> {
        let inventory = artifact_inventory(configured_provider_refs(models))?;
        Ok(Self::from_inventory(inventory, options))
    }

    fn from_inventory(inventory: ArtifactInventory, options: ProviderRegistryOptions) -> Self {
        Self {
            artifacts: inventory.artifacts,
            model_artifacts: inventory.model_artifacts,
            cache: DescribeCache::default(),
            client_factory: ProviderClientFactory::new(options.client),
            host_options: DescribeHostOptions {
                config_root: options.config_root,
                data_root: options.data_root,
            },
        }
    }

    pub fn empty(options: ProviderRegistryOptions) -> Self {
        Self {
            artifacts: BTreeMap::new(),
            model_artifacts: HashMap::new(),
            cache: DescribeCache::default(),
            client_factory: ProviderClientFactory::new(options.client),
            host_options: DescribeHostOptions {
                config_root: options.config_root,
                data_root: options.data_root,
            },
        }
    }

    pub fn convert_ref(
        provider_ref: &ProviderImplementationRef,
    ) -> Result<RuntimeProviderArtifact, ProviderRegistryError> {
        conversion::convert_ref(provider_ref)
    }

    pub fn configured_artifact_keys(&self) -> Vec<ArtifactKey> {
        self.artifacts.keys().cloned().collect()
    }

    pub fn artifact_key_for_model(&self, model_name: &str) -> Option<ArtifactKey> {
        self.model_artifacts.get(model_name).cloned()
    }

    pub fn configured_model_names(&self) -> Vec<String> {
        let mut names = self.model_artifacts.keys().cloned().collect::<Vec<_>>();
        names.sort();
        names
    }

    pub fn describe_model_provider(
        &self,
        model_name: &str,
    ) -> Result<DescribeResult, ProviderRegistryError> {
        let key = self.lookup_artifact_key(model_name)?;
        if let Some(result) = self.cached_describe(&key) {
            return Ok(result);
        }

        self.describe_uncached_model_artifact(model_name, &key)
    }

    fn lookup_artifact_key(&self, model_name: &str) -> Result<ArtifactKey, ProviderRegistryError> {
        self.model_artifacts
            .get(model_name)
            .cloned()
            .ok_or_else(|| ProviderRegistryError::ModelProviderNotConfigured {
                model_name: model_name.to_string(),
            })
    }

    pub(crate) fn enabled_artifact_for_model(
        &self,
        model_name: &str,
    ) -> Result<oulipoly_provider::resolver::ProviderArtifactRef, ProviderRegistryError> {
        let key = self.lookup_artifact_key(model_name)?;
        match self.lookup_artifact(model_name, &key)? {
            RuntimeProviderArtifact::Enabled(artifact) => Ok(artifact),
            RuntimeProviderArtifact::RuntimeDisabled(artifact) => {
                Err(ProviderRegistryError::RuntimeDisabledArtifact {
                    kind: "runtime_disabled".to_string(),
                    artifact,
                })
            }
        }
    }

    pub(crate) fn client_factory(&self) -> &ProviderClientFactory {
        &self.client_factory
    }

    pub(crate) fn host_options(&self) -> &DescribeHostOptions {
        &self.host_options
    }

    fn cached_describe(&self, key: &ArtifactKey) -> Option<DescribeResult> {
        self.cache.get(key)
    }

    fn describe_uncached_model_artifact(
        &self,
        model_name: &str,
        key: &ArtifactKey,
    ) -> Result<DescribeResult, ProviderRegistryError> {
        match self.lookup_artifact(model_name, key)? {
            RuntimeProviderArtifact::Enabled(artifact) => {
                let result = describe_provider(&self.client_factory, artifact, &self.host_options)?;
                self.store_describe(key, result.clone());
                Ok(result)
            }
            RuntimeProviderArtifact::RuntimeDisabled(artifact) => {
                Err(ProviderRegistryError::RuntimeDisabledArtifact {
                    kind: "runtime_disabled".to_string(),
                    artifact,
                })
            }
        }
    }

    fn lookup_artifact(
        &self,
        model_name: &str,
        key: &ArtifactKey,
    ) -> Result<RuntimeProviderArtifact, ProviderRegistryError> {
        self.artifacts.get(key).cloned().ok_or_else(|| {
            ProviderRegistryError::ModelProviderNotConfigured {
                model_name: model_name.to_string(),
            }
        })
    }

    fn store_describe(&self, key: &ArtifactKey, result: DescribeResult) {
        self.cache.insert(key.clone(), result);
    }
}

fn configured_provider_refs(
    models: &[ModelConfig],
) -> impl Iterator<Item = (&str, &ProviderImplementationRef)> {
    models.iter().filter_map(|model| {
        model
            .provider
            .as_ref()
            .map(|provider_ref| (model.name.as_str(), provider_ref))
    })
}

fn artifact_inventory<'a>(
    provider_refs: impl IntoIterator<Item = (&'a str, &'a ProviderImplementationRef)>,
) -> Result<ArtifactInventory, ProviderRegistryError> {
    let mut artifacts = BTreeMap::new();
    let mut model_artifacts = HashMap::new();

    for (model_name, provider_ref) in provider_refs {
        let artifact = ProviderRegistry::convert_ref(provider_ref)?;
        let key = artifact_key(&artifact);
        artifacts.entry(key.clone()).or_insert(artifact);
        model_artifacts.insert(model_name.to_string(), key);
    }

    Ok(ArtifactInventory {
        artifacts,
        model_artifacts,
    })
}
