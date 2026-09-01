use crate::error::HostErrorKind;
use crate::process::is_executable;
use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

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
            ProviderArtifactRef::Path { path } => self.resolve_path(path, config_dir, false),
            ProviderArtifactRef::Script { path } => self.resolve_path(path, config_dir, true),
            ProviderArtifactRef::Binary { name } => {
                self.resolve_binary(validate_binary_name(name)?)
            }
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
        is_script: bool,
    ) -> Result<ResolvedProviderCommand, ProviderResolveError> {
        let candidate = map_provider_path_candidate(path, config_dir);
        if !path.is_absolute() {
            let root = config_dir
                .ok_or_else(|| ProviderResolveError::new(HostErrorKind::MissingArtifact))?;
            ensure_candidate_within_root(root, &candidate)?;
        }
        let canonical_candidate = candidate
            .canonicalize()
            .map_err(|_| ProviderResolveError::new(HostErrorKind::MissingArtifact))?;
        ensure_executable(canonical_candidate, is_script)
    }

    fn resolve_binary(
        &self,
        name: ValidatedBinaryName<'_>,
    ) -> Result<ResolvedProviderCommand, ProviderResolveError> {
        let name = name.as_str();
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

#[derive(Debug, Clone, Copy)]
struct ValidatedBinaryName<'a> {
    name: &'a str,
}

impl<'a> ValidatedBinaryName<'a> {
    fn new(name: &'a str) -> Self {
        Self { name }
    }

    fn as_str(self) -> &'a str {
        self.name
    }
}

fn validate_binary_name(name: &str) -> Result<ValidatedBinaryName<'_>, ProviderResolveError> {
    if name.is_empty() || name.contains('/') || name.contains('\\') {
        return Err(ProviderResolveError::new(HostErrorKind::InvalidBinaryName));
    }
    let path = Path::new(name);
    if path.is_absolute() {
        return Err(ProviderResolveError::new(HostErrorKind::InvalidBinaryName));
    }
    let mut components = path.components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(component)), None) if !component.is_empty() => {
            Ok(ValidatedBinaryName::new(name))
        }
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
    let canonical_candidate = candidate
        .canonicalize()
        .map_err(|_| ProviderResolveError::new(HostErrorKind::MissingArtifact))?;
    ensure_executable(canonical_candidate, false).map(Some)
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

#[derive(Debug, Clone)]
pub struct ResolvedProviderCommand {
    pub(crate) executable: PathBuf,
    pinned_executable: Arc<File>,
    is_script: bool,
    uses_shell_wrapper: bool,
}

impl ResolvedProviderCommand {
    fn pinned(executable: PathBuf, pinned_executable: File, is_script: bool) -> Self {
        Self {
            executable,
            pinned_executable: Arc::new(pinned_executable),
            is_script,
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

    pub(crate) fn pinned_executable(&self) -> Arc<File> {
        self.pinned_executable.clone()
    }

    pub(crate) fn is_script(&self) -> bool {
        self.is_script
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

fn ensure_executable(
    path: PathBuf,
    is_script: bool,
) -> Result<ResolvedProviderCommand, ProviderResolveError> {
    if !path.exists() {
        return Err(ProviderResolveError::new(HostErrorKind::MissingArtifact));
    }
    if !is_executable(&path) {
        return Err(ProviderResolveError::new(HostErrorKind::NotExecutable));
    }
    let pinned_executable = open_pinned_executable(&path, is_script)?;
    Ok(ResolvedProviderCommand::pinned(
        path,
        pinned_executable,
        is_script,
    ))
}

#[cfg(windows)]
fn open_pinned_executable(path: &Path, _is_script: bool) -> Result<File, ProviderResolveError> {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_SHARE_READ: u32 = 0x00000001;
    OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(path)
        .map_err(|_| ProviderResolveError::new(HostErrorKind::MissingArtifact))
}

#[cfg(unix)]
fn open_pinned_executable(path: &Path, is_script: bool) -> Result<File, ProviderResolveError> {
    if is_script {
        return OpenOptions::new()
            .read(true)
            .open(path)
            .map_err(|_| ProviderResolveError::new(HostErrorKind::MissingArtifact));
    }
    open_execute_handle(path)
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn open_execute_handle(path: &Path) -> Result<File, ProviderResolveError> {
    use std::ffi::CString;
    use std::os::fd::FromRawFd;
    use std::os::unix::ffi::OsStrExt;

    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| ProviderResolveError::new(HostErrorKind::MissingArtifact))?;
    let fd = unsafe { libc::open(path.as_ptr(), execute_open_flags()) };
    if fd == -1 {
        Err(ProviderResolveError::new(HostErrorKind::MissingArtifact))
    } else {
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}

#[cfg(target_os = "linux")]
fn execute_open_flags() -> libc::c_int {
    libc::O_PATH | libc::O_CLOEXEC
}

#[cfg(target_vendor = "apple")]
fn execute_open_flags() -> libc::c_int {
    libc::O_EXEC | libc::O_CLOEXEC
}

#[cfg(all(unix, not(any(target_os = "linux", target_vendor = "apple"))))]
fn open_execute_handle(path: &Path) -> Result<File, ProviderResolveError> {
    OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|_| ProviderResolveError::new(HostErrorKind::MissingArtifact))
}

#[cfg(not(any(unix, windows)))]
fn open_pinned_executable(path: &Path, _is_script: bool) -> Result<File, ProviderResolveError> {
    OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|_| ProviderResolveError::new(HostErrorKind::MissingArtifact))
}
