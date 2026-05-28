use oulipoly_provider::{
    LocatedTranscript, LocatorError, LocatorSource, ProviderStorageDescriptor, ScriptKind,
    SessionsLocatorDescriptor, StorageFormatDescriptor, TranscriptLocator, TranscriptLookupMode,
    TranscriptRequest, UnsupportedStorageReason,
};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy)]
struct DummyLocator;

impl TranscriptLocator for DummyLocator {
    fn locate(&self, request: TranscriptRequest<'_>) -> Result<LocatedTranscript, LocatorError> {
        Ok(LocatedTranscript {
            path: PathBuf::from("/tmp/provider-a/session-a.jsonl"),
            source: LocatorSource::ProviderStorage {
                source_id: request
                    .storage
                    .expect("dummy request should carry storage")
                    .source_id,
            },
            storage_format: StorageFormatDescriptor {
                id: "format-a".to_string(),
                label: Some("Format A".to_string()),
            },
        })
    }
}

#[test]
fn locator_contract_types_are_reachable_from_provider_root() {
    let storage_format = StorageFormatDescriptor {
        id: "format-a".to_string(),
        label: Some("Format A".to_string()),
    };
    let request = TranscriptRequest {
        provider: "provider-a",
        session_id: "session-a".into(),
        lookup_mode: TranscriptLookupMode::AllowMissing,
        storage: Some(ProviderStorageDescriptor {
            source_id: "source-a".to_string(),
            root: Some(PathBuf::from("/tmp/provider-a")),
            format: Some(storage_format.clone()),
            script: Some(ScriptKind::TranscriptScript),
        }),
        sessions_config_locator: Some(SessionsLocatorDescriptor {
            source_id: "source-b".to_string(),
            command: "locate-session-a".to_string(),
            state_dir: Some(PathBuf::from("/tmp/provider-a/state")),
        }),
    };

    let result = DummyLocator
        .locate(request)
        .expect("dummy locator should return a path");
    assert!(matches!(
        result.source,
        LocatorSource::ProviderStorage { ref source_id } if source_id == "source-a"
    ));
    assert_eq!(result.storage_format, storage_format);
}

#[test]
fn neutral_unsupported_storage_reasons_are_constructible() {
    let reasons = [
        UnsupportedStorageReason::NoLocator,
        UnsupportedStorageReason::ProviderStorageUnavailable {
            source_id: "source-a".to_string(),
            path: Some(PathBuf::from("/tmp/provider-a")),
            io_error: None,
        },
        UnsupportedStorageReason::ProviderStorageScanNotFound {
            source_id: "source-a".to_string(),
        },
        UnsupportedStorageReason::ProviderStorageScanAmbiguous {
            source_id: "source-a".to_string(),
            candidates: vec![PathBuf::from("/tmp/provider-a/a.jsonl")],
        },
        UnsupportedStorageReason::ProviderUnsupported {
            code: "unsupported-a".to_string(),
            message: "provider contract rejected storage".to_string(),
        },
    ];

    for reason in reasons {
        let error = LocatorError::UnsupportedStorage(reason);
        assert!(
            format!("{error:?}").contains("source-a") || format!("{error:?}").contains("NoLocator")
        );
    }
}
