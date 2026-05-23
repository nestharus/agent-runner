use oulipoly_provider::{
    CapabilityError, LocatedTranscript, LocatorError, LocatorRequiredCapabilities, LocatorSource,
    ProviderCapabilities, TranscriptLocator, TranscriptLookupMode, TranscriptRequest,
};
use static_assertions::assert_not_impl_any;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DummyLocator;

assert_not_impl_any!(
    LocatorRequiredCapabilities<(), (), (), (), (), DummyLocator, (), ()>: Default
);

impl TranscriptLocator for DummyLocator {
    fn locate_jsonl(&self, request: &TranscriptRequest) -> Result<LocatedTranscript, LocatorError> {
        Err(LocatorError::NotFound {
            source: LocatorSource::SessionsLocator,
            session_id: request.session_id.clone(),
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
        provider: "neutral-test-provider",
        session_id: "neutral-session".to_string(),
        storage: None,
        sessions_config_locator: None,
        mode: TranscriptLookupMode::AllowMissing,
    };
    assert!(matches!(
        wrapper.transcript_locator().locate_jsonl(&request),
        Err(LocatorError::NotFound { .. })
    ));

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
