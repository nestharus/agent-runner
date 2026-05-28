use oulipoly_provider::{
    CapabilityError, LocatedTranscript, LocatorError, LocatorRequiredCapabilities, LocatorSource,
    ProviderCapabilities, StorageFormatDescriptor, TranscriptLocator, TranscriptLookupMode,
    TranscriptRequest,
};
use static_assertions::assert_not_impl_any;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DummyLocator;

assert_not_impl_any!(
    LocatorRequiredCapabilities<(), (), (), (), (), DummyLocator, (), ()>: Default
);

impl TranscriptLocator for DummyLocator {
    fn locate(&self, _request: TranscriptRequest<'_>) -> Result<LocatedTranscript, LocatorError> {
        Ok(LocatedTranscript {
            path: PathBuf::from("/tmp/provider-a/session-a.jsonl"),
            source: LocatorSource::Other {
                source_id: "source-a".to_string(),
            },
            storage_format: StorageFormatDescriptor {
                id: "format-a".to_string(),
                label: None,
            },
        })
    }
}

fn capabilities_with_locator_slot(
    transcript_locator: Option<DummyLocator>,
) -> ProviderCapabilities<(), (), (), (), (), DummyLocator> {
    ProviderCapabilities {
        launch: None,
        policy: None,
        terminal: None,
        quota: None,
        session: None,
        transcript_locator,
        rotation: None,
        discovery: None,
    }
}

#[test]
fn locator_required_capabilities_reject_missing_locator() {
    let result =
        LocatorRequiredCapabilities::try_from_capabilities(capabilities_with_locator_slot(None));

    assert!(matches!(
        result,
        Err(CapabilityError::LocatorRequiredButMissing)
    ));
}

#[test]
fn locator_required_capabilities_expose_present_locator() {
    let wrapper = LocatorRequiredCapabilities::try_from_capabilities(
        capabilities_with_locator_slot(Some(DummyLocator)),
    )
    .expect("present locator should satisfy required capabilities");

    assert_eq!(*wrapper.transcript_locator(), DummyLocator);
    assert!(wrapper.capabilities().transcript_locator.is_some());

    let request = TranscriptRequest {
        provider: "provider-a",
        session_id: "session-a".into(),
        lookup_mode: TranscriptLookupMode::AllowMissing,
        storage: None,
        sessions_config_locator: None,
    };
    let located = wrapper
        .transcript_locator()
        .locate(request)
        .expect("contract-local locator should be callable through required wrapper");
    assert_eq!(located.storage_format.id, "format-a");

    let capabilities = wrapper.into_capabilities();
    assert_eq!(capabilities.transcript_locator, Some(DummyLocator));
}

#[test]
fn capability_error_new_variant_is_additive_for_equality() {
    let new_left = CapabilityError::LocatorRequiredButMissing;
    let new_right = CapabilityError::LocatorRequiredButMissing;
    assert_eq!(new_left, new_right);

    let existing_left = CapabilityError::Unsupported;
    let existing_right = CapabilityError::Unsupported;
    assert_eq!(existing_left, existing_right);
}
