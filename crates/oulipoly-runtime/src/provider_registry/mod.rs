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
use oulipoly_provider::resolver::ProviderArtifactRef;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

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
//       - configured account family/executable authority -> runtime provider artifact inventory
//       - oulipoly_provider resolver/client/generated/error contracts -> registry result/error types
//       - artifact-bound provider client API -> model-keyed registry lookup API
//
// ## Intrinsic-surface declarations
//
// intrinsic_surface_declarations:
//   - component: oulipoly-runtime::provider_registry
//     Domain: host-side provider-contract adaptation
//     Owns:
//       - account/family endpoint construction
//       - artifact keying and deduplication
//       - in-process describe cache
//       - describe request orchestration and error mapping
//       - registry root to internal helper module coordination

#[derive(Debug)]
pub struct ProviderRegistry {
    artifacts: BTreeMap<ArtifactKey, RuntimeProviderArtifact>,
    account_artifacts: HashMap<String, ArtifactKey>,
    account_families: HashMap<String, String>,
    family_artifacts: HashMap<String, FamilyArtifact>,
    model_artifacts: HashMap<String, ArtifactKey>,
    model_provider_artifacts: HashMap<ModelProviderKey, ArtifactKey>,
    cache: DescribeCache,
    endpoint_cache: Mutex<HashMap<String, Arc<PinnedProviderEndpoint>>>,
    client_factory: ProviderClientFactory,
    host_options: DescribeHostOptions,
}

#[derive(Debug)]
struct ArtifactInventory {
    artifacts: BTreeMap<ArtifactKey, RuntimeProviderArtifact>,
    account_artifacts: HashMap<String, ArtifactKey>,
    account_families: HashMap<String, String>,
    family_artifacts: HashMap<String, FamilyArtifact>,
    model_artifacts: HashMap<String, ArtifactKey>,
    model_provider_artifacts: HashMap<ModelProviderKey, ArtifactKey>,
}

#[derive(Debug, Clone)]
struct FamilyArtifact {
    account_name: String,
    artifact_key: ArtifactKey,
}

type ModelProviderKey = (String, String);

#[derive(Debug)]
pub struct PinnedProviderEndpoint {
    account_name: String,
    family: String,
    client: Arc<ProviderClient>,
    capabilities: DescribeResult,
}

impl PinnedProviderEndpoint {
    pub fn client(&self) -> &ProviderClient {
        self.client.as_ref()
    }

    pub fn capabilities(&self) -> &DescribeResult {
        &self.capabilities
    }

    pub fn account_name(&self) -> &str {
        &self.account_name
    }

    pub fn family(&self) -> &str {
        &self.family
    }

    pub fn canonical_executable(&self) -> &Path {
        self.client
            .resolved_executable()
            .expect("preflighted provider endpoint must retain a resolved executable")
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

    pub fn from_configs(
        models: &[ModelConfig],
        providers: &ProvidersConfig,
        options: ProviderRegistryOptions,
    ) -> Result<Self, ProviderRegistryError> {
        let mut inventory = account_artifact_inventory(providers)?;
        add_model_account_artifact_keys(&mut inventory, models);
        Ok(Self::from_inventory(inventory, options))
    }

    fn from_inventory(inventory: ArtifactInventory, options: ProviderRegistryOptions) -> Self {
        Self {
            artifacts: inventory.artifacts,
            account_artifacts: inventory.account_artifacts,
            account_families: inventory.account_families,
            family_artifacts: inventory.family_artifacts,
            model_artifacts: inventory.model_artifacts,
            model_provider_artifacts: inventory.model_provider_artifacts,
            cache: DescribeCache::default(),
            endpoint_cache: Mutex::new(HashMap::new()),
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
            account_artifacts: HashMap::new(),
            account_families: HashMap::new(),
            family_artifacts: HashMap::new(),
            model_artifacts: HashMap::new(),
            model_provider_artifacts: HashMap::new(),
            cache: DescribeCache::default(),
            endpoint_cache: Mutex::new(HashMap::new()),
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

    pub fn configured_account_names(&self) -> Vec<String> {
        let mut names = self.account_artifacts.keys().cloned().collect::<Vec<_>>();
        names.sort();
        names
    }

    pub fn artifact_key_for_account(&self, account_name: &str) -> Option<ArtifactKey> {
        self.account_artifacts.get(account_name).cloned()
    }

    pub fn configured_family_names(&self) -> Vec<String> {
        let mut names = self.family_artifacts.keys().cloned().collect::<Vec<_>>();
        names.sort();
        names
    }

    pub fn artifact_key_for_family(&self, family: &str) -> Option<ArtifactKey> {
        self.family_artifacts
            .get(family)
            .map(|entry| entry.artifact_key.clone())
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

    pub fn preflight_account(
        &self,
        account_name: &str,
    ) -> Result<Arc<PinnedProviderEndpoint>, ProviderRegistryError> {
        let mut endpoints = self
            .endpoint_cache
            .lock()
            .expect("provider endpoint cache mutex should not be poisoned");
        if let Some(endpoint) = endpoints.get(account_name) {
            return Ok(endpoint.clone());
        }
        let key = self.lookup_account_artifact_key(account_name)?;
        let artifact = match self.lookup_artifact(account_name, &key)? {
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
        let endpoint = Arc::new(PinnedProviderEndpoint {
            account_name: account_name.to_string(),
            family: self
                .account_families
                .get(account_name)
                .cloned()
                .expect("configured account endpoint must retain its family"),
            client,
            capabilities,
        });
        endpoints.insert(account_name.to_string(), endpoint.clone());
        Ok(endpoint)
    }

    fn lookup_account_artifact_key(
        &self,
        account_name: &str,
    ) -> Result<ArtifactKey, ProviderRegistryError> {
        self.account_artifacts
            .get(account_name)
            .cloned()
            .ok_or_else(
                || ProviderRegistryError::AccountImplementationNotConfigured {
                    account_name: account_name.to_string(),
                },
            )
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
        account_artifacts: HashMap::new(),
        account_families: HashMap::new(),
        family_artifacts: HashMap::new(),
        model_artifacts,
        model_provider_artifacts: HashMap::new(),
    })
}

fn empty_artifact_inventory() -> ArtifactInventory {
    ArtifactInventory {
        artifacts: BTreeMap::new(),
        account_artifacts: HashMap::new(),
        account_families: HashMap::new(),
        family_artifacts: HashMap::new(),
        model_artifacts: HashMap::new(),
        model_provider_artifacts: HashMap::new(),
    }
}

fn account_artifact_inventory(
    providers: &ProvidersConfig,
) -> Result<ArtifactInventory, ProviderRegistryError> {
    let mut inventory = empty_artifact_inventory();
    let mut account_names = providers.entries.keys().collect::<Vec<_>>();
    account_names.sort();
    for account_name in account_names {
        let entry = providers
            .get(account_name)
            .expect("provider account name came from the same config");
        let implementation = entry.implementation.as_ref().ok_or_else(|| {
            ProviderRegistryError::AccountImplementationNotConfigured {
                account_name: account_name.clone(),
            }
        })?;
        let artifact = RuntimeProviderArtifact::Enabled(ProviderArtifactRef::Path {
            path: PathBuf::from(&implementation.executable),
        });
        let key = artifact_key(&artifact);
        validate_family_artifact(
            &inventory.family_artifacts,
            &implementation.family,
            account_name,
            &key,
        )?;
        inventory.artifacts.entry(key.clone()).or_insert(artifact);
        inventory
            .account_artifacts
            .insert(account_name.clone(), key.clone());
        inventory
            .account_families
            .insert(account_name.clone(), implementation.family.clone());
        inventory
            .family_artifacts
            .entry(implementation.family.clone())
            .or_insert_with(|| FamilyArtifact {
                account_name: account_name.clone(),
                artifact_key: key,
            });
    }
    Ok(inventory)
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

fn add_model_account_artifact_keys(inventory: &mut ArtifactInventory, models: &[ModelConfig]) {
    for model in models {
        let mut shared_model_key = None;
        let mut complete_model_mapping = !model.providers.is_empty();
        for provider in &model.providers {
            let Some(key) = inventory.account_artifacts.get(&provider.name).cloned() else {
                complete_model_mapping = false;
                continue;
            };
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
    }
}

fn validate_family_artifact(
    families: &HashMap<String, FamilyArtifact>,
    family: &str,
    account_name: &str,
    artifact_key: &str,
) -> Result<(), ProviderRegistryError> {
    let Some(existing) = families.get(family) else {
        return Ok(());
    };
    if existing.artifact_key == artifact_key {
        return Ok(());
    }
    Err(ProviderRegistryError::FamilyImplementationConflict {
        family: family.to_string(),
        first_account: existing.account_name.clone(),
        second_account: account_name.to_string(),
    })
}

fn model_provider_key(model_name: &str, provider_name: &str) -> ModelProviderKey {
    (model_name.to_string(), provider_name.to_string())
}
