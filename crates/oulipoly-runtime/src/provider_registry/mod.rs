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
pub(crate) use describe::{DescribeHostOptions, describe_provider_client};
use describe::{describe_provider, describe_provider_with_cancellation};
use oulipoly_config::{
    ModelConfig, ProvidersConfig, provider_implementation_ref::ProviderImplementationRef,
};
use oulipoly_provider::client::{CancellationToken, ProviderClient};
use oulipoly_provider::generated::DescribeResult;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

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
    model_provider_artifacts: HashMap<ModelProviderKey, ArtifactKey>,
    cache: DescribeCache,
    client_factory: ProviderClientFactory,
    host_options: DescribeHostOptions,
}

#[derive(Debug)]
struct ArtifactInventory {
    artifacts: BTreeMap<ArtifactKey, RuntimeProviderArtifact>,
    model_artifacts: HashMap<String, ArtifactKey>,
    model_provider_artifacts: HashMap<ModelProviderKey, ArtifactKey>,
}

type ModelProviderKey = (String, String);

#[derive(Debug)]
pub(crate) struct PinnedProviderEndpoint {
    client: Arc<ProviderClient>,
    capabilities: DescribeResult,
}

impl PinnedProviderEndpoint {
    pub(crate) fn client(&self) -> &ProviderClient {
        self.client.as_ref()
    }

    pub(crate) fn capabilities(&self) -> &DescribeResult {
        &self.capabilities
    }
}

impl ProviderRegistry {
    pub fn from_model_configs(
        models: &[ModelConfig],
        options: ProviderRegistryOptions,
    ) -> Result<Self, ProviderRegistryError> {
        let mut inventory = artifact_inventory(configured_provider_refs(models))?;
        for model in models {
            if let Some(model_key) = inventory.model_artifacts.get(&model.name).cloned() {
                add_model_provider_artifact_keys(&mut inventory, model, model_key);
            }
        }
        Ok(Self::from_inventory(inventory, options))
    }

    pub fn from_model_configs_with_provider_config(
        models: &[ModelConfig],
        providers: &ProvidersConfig,
        options: ProviderRegistryOptions,
    ) -> Result<Self, ProviderRegistryError> {
        let mut inventory = empty_artifact_inventory();
        add_provider_instance_artifacts(&mut inventory, models, providers)?;
        Ok(Self::from_inventory(inventory, options))
    }

    fn from_inventory(inventory: ArtifactInventory, options: ProviderRegistryOptions) -> Self {
        Self {
            artifacts: inventory.artifacts,
            model_artifacts: inventory.model_artifacts,
            model_provider_artifacts: inventory.model_provider_artifacts,
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
            model_provider_artifacts: HashMap::new(),
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

    pub fn artifact_key_for_model_provider(
        &self,
        model_name: &str,
        provider_name: &str,
    ) -> Option<ArtifactKey> {
        self.model_provider_artifacts
            .get(&model_provider_key(model_name, provider_name))
            .cloned()
            .or_else(|| self.artifact_key_for_model(model_name))
    }

    pub fn resolve_model_name_for_provider_instance(
        &self,
        model_name: &str,
        provider_name: &str,
    ) -> Option<String> {
        if self
            .artifact_key_for_model_provider(model_name, provider_name)
            .is_some()
        {
            return Some(model_name.to_string());
        }

        self.resolve_model_name_for_provider(provider_name)
    }

    pub fn resolve_model_name_for_provider(&self, provider_name: &str) -> Option<String> {
        let mut candidates = self
            .model_provider_artifacts
            .iter()
            .filter(|((_, candidate_provider), _)| candidate_provider == provider_name);
        let ((first_model, _), first_artifact) = candidates.next()?;
        let mut resolved_model = first_model;
        for ((candidate_model, _), candidate_artifact) in candidates {
            if candidate_artifact != first_artifact {
                return None;
            }
            if candidate_model < resolved_model {
                resolved_model = candidate_model;
            }
        }
        Some(resolved_model.clone())
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

    pub fn describe_model_provider_instance(
        &self,
        model_name: &str,
        provider_name: &str,
    ) -> Result<DescribeResult, ProviderRegistryError> {
        let key = self.lookup_model_provider_artifact_key(model_name, provider_name)?;
        if let Some(result) = self.cached_describe(&key) {
            return Ok(result);
        }

        self.describe_uncached_model_artifact(model_name, &key)
    }

    pub(crate) fn describe_model_provider_instance_with_cancellation(
        &self,
        model_name: &str,
        provider_name: &str,
        cancellation: &CancellationToken,
    ) -> Result<DescribeResult, ProviderRegistryError> {
        let key = self.lookup_model_provider_artifact_key(model_name, provider_name)?;
        if let Some(result) = self.cached_describe(&key) {
            return Ok(result);
        }

        self.describe_uncached_model_artifact_with_cancellation(model_name, &key, cancellation)
    }

    pub(crate) fn preflight_model_provider_instance(
        &self,
        model_name: &str,
        provider_name: &str,
    ) -> Result<Arc<PinnedProviderEndpoint>, ProviderRegistryError> {
        let key = self.lookup_model_provider_artifact_key(model_name, provider_name)?;
        let artifact = match self.lookup_artifact(model_name, &key)? {
            RuntimeProviderArtifact::Enabled(artifact) => artifact,
            RuntimeProviderArtifact::RuntimeDisabled(artifact) => {
                return Err(ProviderRegistryError::RuntimeDisabledArtifact {
                    kind: "runtime_disabled".to_string(),
                    artifact,
                });
            }
        };
        let client = Arc::new(self.client_factory.client_for(artifact));
        let capabilities = describe_provider_client(client.as_ref(), &self.host_options)?;
        self.store_describe(&key, capabilities.clone());
        Ok(Arc::new(PinnedProviderEndpoint {
            client,
            capabilities,
        }))
    }

    fn lookup_artifact_key(&self, model_name: &str) -> Result<ArtifactKey, ProviderRegistryError> {
        self.model_artifacts
            .get(model_name)
            .cloned()
            .ok_or_else(|| ProviderRegistryError::ModelProviderNotConfigured {
                model_name: model_name.to_string(),
            })
    }

    fn lookup_model_provider_artifact_key(
        &self,
        model_name: &str,
        provider_name: &str,
    ) -> Result<ArtifactKey, ProviderRegistryError> {
        self.artifact_key_for_model_provider(model_name, provider_name)
            .ok_or_else(|| ProviderRegistryError::ModelProviderNotConfigured {
                model_name: format!("{model_name}/{provider_name}"),
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

    pub(crate) fn enabled_artifact_for_model_provider(
        &self,
        model_name: &str,
        provider_name: &str,
    ) -> Result<oulipoly_provider::resolver::ProviderArtifactRef, ProviderRegistryError> {
        let key = self.lookup_model_provider_artifact_key(model_name, provider_name)?;
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
        self.describe_uncached_model_artifact_inner(model_name, key, None)
    }

    fn describe_uncached_model_artifact_with_cancellation(
        &self,
        model_name: &str,
        key: &ArtifactKey,
        cancellation: &CancellationToken,
    ) -> Result<DescribeResult, ProviderRegistryError> {
        self.describe_uncached_model_artifact_inner(model_name, key, Some(cancellation))
    }

    fn describe_uncached_model_artifact_inner(
        &self,
        model_name: &str,
        key: &ArtifactKey,
        cancellation: Option<&CancellationToken>,
    ) -> Result<DescribeResult, ProviderRegistryError> {
        match self.lookup_artifact(model_name, key)? {
            RuntimeProviderArtifact::Enabled(artifact) => {
                let result = match cancellation {
                    Some(cancellation) => describe_provider_with_cancellation(
                        &self.client_factory,
                        artifact,
                        &self.host_options,
                        cancellation,
                    ),
                    None => describe_provider(&self.client_factory, artifact, &self.host_options),
                }?;
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
        model_provider_artifacts: HashMap::new(),
    })
}

fn empty_artifact_inventory() -> ArtifactInventory {
    ArtifactInventory {
        artifacts: BTreeMap::new(),
        model_artifacts: HashMap::new(),
        model_provider_artifacts: HashMap::new(),
    }
}

fn add_provider_instance_artifacts(
    inventory: &mut ArtifactInventory,
    models: &[ModelConfig],
    providers: &ProvidersConfig,
) -> Result<(), ProviderRegistryError> {
    for model in models {
        add_provider_account_artifacts(inventory, model, providers)?;
    }
    Ok(())
}

fn add_model_provider_artifact_keys(
    inventory: &mut ArtifactInventory,
    model: &ModelConfig,
    artifact_key: ArtifactKey,
) {
    for provider in &model.providers {
        inventory.model_provider_artifacts.insert(
            model_provider_key(&model.name, &provider.name),
            artifact_key.clone(),
        );
    }
}

fn add_provider_account_artifacts(
    inventory: &mut ArtifactInventory,
    model: &ModelConfig,
    providers: &ProvidersConfig,
) -> Result<(), ProviderRegistryError> {
    let mut shared_model_key = None;
    let mut complete_model_mapping = !model.providers.is_empty();
    for provider in &model.providers {
        let Some(entry) = providers.get(&provider.name) else {
            complete_model_mapping = false;
            continue;
        };
        let Some(provider_ref) = entry.implementation.as_ref() else {
            complete_model_mapping = false;
            continue;
        };
        let artifact = ProviderRegistry::convert_ref(provider_ref)?;
        let key = artifact_key(&artifact);
        inventory.artifacts.entry(key.clone()).or_insert(artifact);
        inventory
            .model_provider_artifacts
            .insert(model_provider_key(&model.name, &provider.name), key.clone());
        match shared_model_key.as_ref() {
            None => shared_model_key = Some(key),
            Some(existing) if existing == &key => {}
            Some(_) => complete_model_mapping = false,
        }
    }
    if complete_model_mapping && let Some(key) = shared_model_key {
        inventory.model_artifacts.insert(model.name.clone(), key);
    }
    Ok(())
}

fn model_provider_key(model_name: &str, provider_name: &str) -> ModelProviderKey {
    (model_name.to_string(), provider_name.to_string())
}
