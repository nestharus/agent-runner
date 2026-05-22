use oulipoly_provider::ProviderCapabilities;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DummyLaunch;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DummyPolicy;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DummyLocator;

fn locator_payload<Locator>(
    capabilities: ProviderCapabilities<(), (), (), (), (), Locator>,
) -> Option<Locator> {
    capabilities.transcript_locator
}

// Risk: generic-bundle construction could accidentally force every capability
// concern to be implemented as one monolithic surface.
// Level: integration. Source: contract C5.3/C6 and proposal test-intent item 3.
#[test]
fn capabilities_bundle_represents_partial_implementations() {
    let capabilities: ProviderCapabilities<DummyLaunch, DummyPolicy> = ProviderCapabilities {
        launch: Some(DummyLaunch),
        policy: Some(DummyPolicy),
        terminal: None,
        quota: None,
        session: None,
        transcript_locator: None,
        rotation: None,
        discovery: None,
    };

    assert_eq!(capabilities.launch, Some(DummyLaunch));
    assert_eq!(capabilities.policy, Some(DummyPolicy));
    assert!(capabilities.terminal.is_none());
    assert!(capabilities.quota.is_none());
    assert!(capabilities.session.is_none());
    assert!(capabilities.transcript_locator.is_none());
    assert!(capabilities.rotation.is_none());
    assert!(capabilities.discovery.is_none());
}

// Risk: generic-bundle Default could regress and stop providing an all-absent
// constructor for low-surface providers.
// Level: integration. Source: contract C3/C6 and proposal test-intent item 3.
#[test]
fn capabilities_bundle_default_has_no_populated_slots() {
    let capabilities: ProviderCapabilities = ProviderCapabilities::default();

    assert!(capabilities.launch.is_none());
    assert!(capabilities.policy.is_none());
    assert!(capabilities.terminal.is_none());
    assert!(capabilities.quota.is_none());
    assert!(capabilities.session.is_none());
    assert!(capabilities.transcript_locator.is_none());
    assert!(capabilities.rotation.is_none());
    assert!(capabilities.discovery.is_none());
}

// Risk: the locator slot could grow an unintended dependency or trait bound
// before the later bridge work owns that integration.
// Level: integration. Source: contract C5.4/C6 and proposal test-intent item 4.
#[test]
fn locator_slot_accepts_test_local_marker_type() {
    let capabilities: ProviderCapabilities<(), (), (), (), (), DummyLocator> =
        ProviderCapabilities {
            launch: None,
            policy: None,
            terminal: None,
            quota: None,
            session: None,
            transcript_locator: Some(DummyLocator),
            rotation: None,
            discovery: None,
        };

    assert_eq!(locator_payload(capabilities), Some(DummyLocator));
}
