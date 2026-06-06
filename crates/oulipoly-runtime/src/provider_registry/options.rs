use oulipoly_provider::client::ProviderClientOptions;
use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
pub struct ProviderRegistryOptions {
    pub(crate) client: ProviderClientOptions,
    pub(crate) config_root: Option<PathBuf>,
    pub(crate) data_root: Option<PathBuf>,
    pub(crate) cache_root: Option<PathBuf>,
}

impl ProviderRegistryOptions {
    pub fn with_client_options(mut self, client: ProviderClientOptions) -> Self {
        self.client = client;
        self
    }

    pub fn with_path_entries<I>(mut self, entries: I) -> Self
    where
        I: IntoIterator<Item = PathBuf>,
    {
        self.client.resolver = self.client.resolver.with_path_entries(entries);
        self
    }

    pub fn with_path_entries_from_process_path(self) -> Self {
        self.with_path_entries_from_path_env(std::env::var_os("PATH"))
    }

    pub fn with_path_entries_from_path_env(self, path_env: Option<OsString>) -> Self {
        match path_env {
            Some(path_env) => self.with_path_entries(std::env::split_paths(&path_env)),
            None => self,
        }
    }

    pub fn with_config_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.config_root = Some(root.into());
        self
    }

    pub fn with_data_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.data_root = Some(root.into());
        self
    }

    pub fn with_cache_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.cache_root = Some(root.into());
        self
    }
}
