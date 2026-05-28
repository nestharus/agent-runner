use oulipoly_provider::{
    LocatedTranscript, LocatorError, LocatorSource, ProviderStorageDescriptor,
    StorageFormatDescriptor, TranscriptLocator, TranscriptLookupMode, TranscriptRequest,
    UnsupportedStorageReason,
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
                    .expect("runtime bridge request should include storage")
                    .source_id,
            },
            storage_format: StorageFormatDescriptor {
                id: "format-a".to_string(),
                label: None,
            },
        })
    }
}

#[test]
fn runtime_can_dispatch_locator_trait_imported_from_provider_crate() {
    let dummy = DummyLocator;
    let request = TranscriptRequest {
        provider: "provider-a",
        session_id: "session-a".into(),
        lookup_mode: TranscriptLookupMode::AllowMissing,
        storage: Some(ProviderStorageDescriptor {
            source_id: "source-a".to_string(),
            root: Some(PathBuf::from("/tmp/provider-a")),
            format: Some(StorageFormatDescriptor {
                id: "format-a".to_string(),
                label: None,
            }),
            script: None,
        }),
        sessions_config_locator: None,
    };

    let result = TranscriptLocator::locate(&dummy, request)
        .expect("runtime should dispatch through provider-owned locator trait");
    assert!(matches!(
        result.source,
        LocatorSource::ProviderStorage { ref source_id } if source_id == "source-a"
    ));
    assert_eq!(result.storage_format.id, "format-a");

    let _neutral_reason = UnsupportedStorageReason::ProviderStorageScanNotFound {
        source_id: "source-a".to_string(),
    };
}
