use oulipoly_provider::{
    LocatedTranscript, LocatorError, LocatorSource, ScriptKind, TranscriptLocator,
    TranscriptLookupMode, TranscriptRequest, UnsupportedStorageReason,
};

#[derive(Debug, Clone, Copy)]
struct DummyLocator;

impl TranscriptLocator for DummyLocator {
    fn locate_jsonl(&self, request: &TranscriptRequest) -> Result<LocatedTranscript, LocatorError> {
        Err(LocatorError::NotFound {
            source: LocatorSource::SessionsLocator,
            session_id: request.session_id.clone(),
        })
    }
}

#[test]
fn locator_contract_types_are_reachable_from_provider_root() {
    let request = TranscriptRequest {
        provider: "neutral-test-provider",
        session_id: "neutral-session".to_string(),
        storage: None,
        sessions_config_locator: None,
        mode: TranscriptLookupMode::AllowMissing,
    };

    let result = DummyLocator.locate_jsonl(&request);
    assert!(matches!(
        result,
        Err(LocatorError::NotFound {
            source: LocatorSource::SessionsLocator,
            session_id,
        }) if session_id == "neutral-session"
    ));

    let _neutral_reason = UnsupportedStorageReason::NoLocatorForUnknownStorage;
    let _neutral_kind = ScriptKind::TranscriptScript;
}
