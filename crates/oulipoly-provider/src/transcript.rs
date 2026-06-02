use std::fmt;
use std::io::ErrorKind as StdIoErrorKind;
use std::path::PathBuf;

pub type ProviderName<'a> = &'a str;
pub type SessionId = String;
pub type IoErrorKind = StdIoErrorKind;

pub trait TranscriptLocator {
    fn locate(&self, request: TranscriptRequest<'_>) -> Result<LocatedTranscript, LocatorError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptRequest<'a> {
    pub provider: ProviderName<'a>,
    pub session_id: SessionId,
    pub lookup_mode: TranscriptLookupMode,
    pub storage: Option<ProviderStorageDescriptor>,
    pub sessions_config_locator: Option<SessionsLocatorDescriptor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptLookupMode {
    RequireExisting,
    AllowMissing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocatedTranscript {
    pub path: PathBuf,
    pub source: LocatorSource,
    pub storage_format: StorageFormatDescriptor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocatorSource {
    ConfiguredLocator { source_id: String },
    ProviderStorage { source_id: String },
    ContentFallback { source_id: String },
    PrivateLayout { source_id: String },
    Other { source_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderStorageDescriptor {
    pub source_id: String,
    pub root: Option<PathBuf>,
    pub format: Option<StorageFormatDescriptor>,
    pub script: Option<ScriptKind>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionsLocatorDescriptor {
    pub source_id: String,
    pub command: String,
    pub state_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageFormatDescriptor {
    pub id: String,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptKind {
    TranscriptScript,
    SessionsTranscriptLocator,
}

#[derive(Clone, PartialEq, Eq)]
pub enum UnsupportedStorageReason {
    NoLocator,
    ProviderStorageUnavailable {
        source_id: String,
        path: Option<PathBuf>,
        io_error: Option<IoErrorKind>,
    },
    ProviderStorageScanNotFound {
        source_id: String,
    },
    ProviderStorageScanAmbiguous {
        source_id: String,
        candidates: Vec<PathBuf>,
    },
    ProviderUnsupported {
        code: String,
        message: String,
    },
}

impl fmt::Debug for UnsupportedStorageReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoLocator => formatter.write_str("NoLocator"),
            Self::ProviderStorageUnavailable {
                source_id,
                path,
                io_error,
            } => formatter
                .debug_struct("ProviderStorageUnavailable")
                .field("source_id", source_id)
                .field("path", path)
                .field("io_error", io_error)
                .finish(),
            Self::ProviderStorageScanNotFound { source_id } => formatter
                .debug_struct("ProviderStorageScanNotFound")
                .field("source_id", source_id)
                .finish(),
            Self::ProviderStorageScanAmbiguous {
                source_id,
                candidates,
            } => formatter
                .debug_struct("ProviderStorageScanAmbiguous")
                .field("source_id", source_id)
                .field("candidates", candidates)
                .finish(),
            Self::ProviderUnsupported { code, message } => formatter
                .debug_struct("ProviderUnsupported")
                .field("code", code)
                .field("message", message)
                .field("fallback", &"NoLocator")
                .finish(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocatorError {
    UnsupportedStorage(UnsupportedStorageReason),
    InvalidPath {
        path: PathBuf,
        reason: String,
    },
    Io {
        source_id: String,
        kind: IoErrorKind,
        message: String,
    },
    SpawnFailed {
        source_id: String,
        message: String,
    },
    ScriptFailed {
        source_id: String,
        status: Option<i32>,
        stderr: String,
    },
    InvalidOutput {
        source_id: String,
        message: String,
    },
    Other {
        code: String,
        message: String,
    },
}
