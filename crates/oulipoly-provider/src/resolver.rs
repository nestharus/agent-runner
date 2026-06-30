use crate::error::HostErrorKind;
use crate::process::is_executable;
use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderArtifactRef {
    Path { path: PathBuf },
    Binary { name: String },
    Script { path: PathBuf },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeDisabledArtifact {
    Crate {
        crate_name: String,
        version: Option<String>,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderResolveOptions {
    path_entries: Vec<PathBuf>,
    packaged_search_roots: Vec<PathBuf>,
    packaged_allowlist: BTreeSet<String>,
}

impl ProviderResolveOptions {
    pub fn with_path_entries<I>(mut self, entries: I) -> Self
    where
        I: IntoIterator<Item = PathBuf>,
    {
        self.path_entries.extend(entries);
        self
    }

    pub fn with_packaged_search_roots<I>(mut self, roots: I) -> Self
    where
        I: IntoIterator<Item = PathBuf>,
    {
        self.packaged_search_roots.extend(roots);
        self
    }

    pub fn with_packaged_allowlist<I, S>(mut self, names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.packaged_allowlist
            .extend(names.into_iter().map(Into::into));
        self
    }

    pub fn path_entries(&self) -> &[PathBuf] {
        &self.path_entries
    }
}

#[derive(Debug, Clone)]
pub struct ProviderResolver {
    options: ProviderResolveOptions,
}

impl ProviderResolver {
    pub fn new(options: ProviderResolveOptions) -> Self {
        Self { options }
    }

    pub fn options(&self) -> &ProviderResolveOptions {
        &self.options
    }

    pub fn resolve(
        &self,
        artifact: &ProviderArtifactRef,
        config_dir: Option<&Path>,
    ) -> Result<ResolvedProviderCommand, ProviderResolveError> {
        match artifact {
            ProviderArtifactRef::Path { path } => self.resolve_path(path, config_dir),
            ProviderArtifactRef::Script { path } => self.resolve_path(path, config_dir),
            ProviderArtifactRef::Binary { name } => self.resolve_binary(name),
        }
    }

    pub fn resolve_runtime_disabled(
        &self,
        _artifact: &RuntimeDisabledArtifact,
    ) -> Result<ResolvedProviderCommand, ProviderResolveError> {
        Err(ProviderResolveError::new(HostErrorKind::RuntimeDisabled))
    }

    fn resolve_path(
        &self,
        path: &Path,
        config_dir: Option<&Path>,
    ) -> Result<ResolvedProviderCommand, ProviderResolveError> {
        let candidate = map_provider_path_candidate(path, config_dir);
        ensure_executable(candidate)
    }

    fn resolve_binary(&self, name: &str) -> Result<ResolvedProviderCommand, ProviderResolveError> {
        let name = validate_binary_name(name)?;
        for root in binary_search_roots(&self.options, name) {
            if let Some(resolved) = resolve_binary_in_root(root, name)? {
                return Ok(resolved);
            }
        }
        Err(ProviderResolveError::new(HostErrorKind::MissingArtifact))
    }
}

fn map_provider_path_candidate(path: &Path, config_dir: Option<&Path>) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else if let Some(config_dir) = config_dir {
        config_dir.join(path)
    } else {
        path.to_path_buf()
    }
}

fn binary_search_roots<'a>(
    options: &'a ProviderResolveOptions,
    name: &'a str,
) -> impl Iterator<Item = &'a PathBuf> + 'a {
    options.path_entries.iter().chain(
        options
            .packaged_search_roots
            .iter()
            .filter(move |_| is_packaged_binary_allowed(options, name)),
    )
}

fn is_packaged_binary_allowed(options: &ProviderResolveOptions, name: &str) -> bool {
    options.packaged_allowlist.contains(name)
}

fn validate_binary_name(name: &str) -> Result<&str, ProviderResolveError> {
    if name.is_empty() || name.contains('/') || name.contains('\\') {
        return Err(ProviderResolveError::new(HostErrorKind::InvalidBinaryName));
    }
    let path = Path::new(name);
    if path.is_absolute() {
        return Err(ProviderResolveError::new(HostErrorKind::InvalidBinaryName));
    }
    let mut components = path.components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(component)), None) if !component.is_empty() => Ok(name),
        _ => Err(ProviderResolveError::new(HostErrorKind::InvalidBinaryName)),
    }
}

fn resolve_binary_in_root(
    root: &Path,
    name: &str,
) -> Result<Option<ResolvedProviderCommand>, ProviderResolveError> {
    let candidate = map_binary_candidate(root, name);
    if !binary_candidate_exists(&candidate) {
        return Ok(None);
    }
    ensure_candidate_within_root(root, &candidate)?;
    ensure_executable(candidate).map(Some)
}

fn map_binary_candidate(root: &Path, name: &str) -> PathBuf {
    root.join(name)
}

fn binary_candidate_exists(candidate: &Path) -> bool {
    candidate.exists()
}

fn ensure_candidate_within_root(root: &Path, candidate: &Path) -> Result<(), ProviderResolveError> {
    let canonical_root = root
        .canonicalize()
        .map_err(|_| ProviderResolveError::new(HostErrorKind::MissingArtifact))?;
    let canonical_candidate = candidate
        .canonicalize()
        .map_err(|_| ProviderResolveError::new(HostErrorKind::MissingArtifact))?;
    if canonical_candidate.starts_with(canonical_root) {
        Ok(())
    } else {
        Err(ProviderResolveError::new(HostErrorKind::UnsafeBinary))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProviderCommand {
    executable: PathBuf,
    uses_shell_wrapper: bool,
}

impl ResolvedProviderCommand {
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            uses_shell_wrapper: false,
        }
    }

    pub fn executable(&self) -> PathBuf {
        self.executable.clone()
    }

    pub fn argv_for_subcommand(&self, subcommand: impl AsRef<OsStr>) -> Vec<OsString> {
        vec![
            self.executable.as_os_str().to_os_string(),
            subcommand.as_ref().to_os_string(),
        ]
    }

    pub fn uses_shell_wrapper(&self) -> bool {
        self.uses_shell_wrapper
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderResolveError {
    kind: HostErrorKind,
}

impl ProviderResolveError {
    pub fn new(kind: HostErrorKind) -> Self {
        Self { kind }
    }

    pub fn kind(&self) -> &str {
        self.kind.as_str()
    }
}

fn ensure_executable(path: PathBuf) -> Result<ResolvedProviderCommand, ProviderResolveError> {
    if !path.exists() {
        return Err(ProviderResolveError::new(HostErrorKind::MissingArtifact));
    }
    if !is_executable(&path) {
        return Err(ProviderResolveError::new(HostErrorKind::NotExecutable));
    }
    Ok(ResolvedProviderCommand::new(path))
}
