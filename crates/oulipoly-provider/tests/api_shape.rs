use oulipoly_provider::{
    AuthRefreshRequest, AuthRefreshStatus, CapabilityError, DiscoveryReport, DiscoveryRequest,
    LaunchPlan, LaunchRequest, PolicyRequest, PolicyTransform, ProviderContext, ProviderDiscovery,
    ProviderLaunch, ProviderPolicy, ProviderQuota, ProviderRotation, ProviderSession, QuotaRequest,
    QuotaSnapshot, RotationAssessment, RotationMaterialization, RotationMaterializationRequest,
    RotationRequest, SessionCapture, SessionCaptureRequest, SessionTurnBatch, SessionTurnRequest,
    TerminalSignal, TerminalSignalEvidence, TerminalSignalRecognizer,
};

struct DummyProvider;

impl ProviderLaunch for DummyProvider {
    fn prepare_launch(&self, _request: LaunchRequest<'_>) -> Result<LaunchPlan, CapabilityError> {
        Ok(LaunchPlan::default())
    }
}

impl ProviderPolicy for DummyProvider {
    fn evaluate_policy(
        &self,
        _request: PolicyRequest<'_>,
    ) -> Result<PolicyTransform, CapabilityError> {
        Ok(PolicyTransform::default())
    }
}

impl TerminalSignalRecognizer for DummyProvider {
    fn recognize(&self, _evidence: &TerminalSignalEvidence<'_>) -> TerminalSignal {
        TerminalSignal::default()
    }
}

impl ProviderQuota for DummyProvider {
    fn has_quota_source(&self, _context: ProviderContext<'_>) -> bool {
        true
    }

    fn probe_quota(&self, _request: QuotaRequest<'_>) -> Result<QuotaSnapshot, CapabilityError> {
        Ok(QuotaSnapshot::default())
    }

    fn refresh_auth(
        &self,
        _request: AuthRefreshRequest<'_>,
    ) -> Result<AuthRefreshStatus, CapabilityError> {
        Ok(AuthRefreshStatus::default())
    }
}

impl ProviderSession for DummyProvider {
    fn read_session_turns(
        &self,
        _request: SessionTurnRequest<'_>,
    ) -> Result<SessionTurnBatch, CapabilityError> {
        Ok(SessionTurnBatch::default())
    }

    fn capture_session(
        &self,
        _request: SessionCaptureRequest<'_>,
    ) -> Result<SessionCapture, CapabilityError> {
        Ok(SessionCapture::default())
    }
}

impl ProviderRotation for DummyProvider {
    fn assess_rotation(
        &self,
        _request: RotationRequest<'_>,
    ) -> Result<RotationAssessment, CapabilityError> {
        Ok(RotationAssessment::default())
    }

    fn materialize_rotation(
        &self,
        _request: RotationMaterializationRequest<'_>,
    ) -> Result<RotationMaterialization, CapabilityError> {
        Ok(RotationMaterialization::default())
    }
}

impl ProviderDiscovery for DummyProvider {
    fn discover(&self, _request: DiscoveryRequest<'_>) -> Result<DiscoveryReport, CapabilityError> {
        Ok(DiscoveryReport::default())
    }
}

fn assert_error_contract<E>()
where
    E: std::error::Error + Clone + PartialEq + std::fmt::Debug,
{
}

// Risk: trait-shape drift would make neutral provider implementations unusable.
// Level: integration. Source: contract C5.2 and proposal test-intent item 2.
#[test]
fn dummy_provider_can_implement_and_call_each_trait_method() {
    let provider = DummyProvider;

    assert!(provider.prepare_launch(LaunchRequest::default()).is_ok());
    assert!(provider.evaluate_policy(PolicyRequest::default()).is_ok());

    let signal = provider.recognize(&TerminalSignalEvidence::default());
    assert!(!format!("{signal:?}").is_empty());

    assert!(provider.has_quota_source(ProviderContext::default()));
    assert!(provider.probe_quota(QuotaRequest::default()).is_ok());
    assert!(provider.refresh_auth(AuthRefreshRequest::default()).is_ok());

    assert!(
        provider
            .read_session_turns(SessionTurnRequest::default())
            .is_ok()
    );
    assert!(
        provider
            .capture_session(SessionCaptureRequest::default())
            .is_ok()
    );

    assert!(provider.assess_rotation(RotationRequest::default()).is_ok());
    assert!(
        provider
            .materialize_rotation(RotationMaterializationRequest::default())
            .is_ok()
    );

    assert!(provider.discover(DiscoveryRequest::default()).is_ok());
    assert_error_contract::<CapabilityError>();
}
