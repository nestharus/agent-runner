use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use chrono::{DateTime, TimeDelta, Utc};
use oulipoly_agent_store::{ArtifactKey, PutReceipt, PutRequest, Store, TombstoneMeta};
use rusqlite::params;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use uuid::Uuid;

use super::*;
use crate::retirement_status::RetirementStatus;
use crate::{
    CanonicalAddress, DeleteReceipt, DeleteRequest, DeleteSelector, GcReport, GcRequest,
    GcSelector, InvocationScope, ListRequest, PublishReceipt, PublishRequest, ReadRequest,
    ScratchpadAddress, ScratchpadError, ScratchpadMeta, ScratchpadName, ScratchpadRecord,
    StoreScratchpadPersistence, WriteReceipt, WriteRequest,
};

type SharedEvents = Rc<RefCell<VecDeque<ScriptedEvent>>>;

#[derive(Debug)]
enum ScriptedEvent {
    ObserveUtc {
        returns: DateTime<Utc>,
    },
    AppendPrivateVersion {
        expected_draft: PrivateVersionDraft,
        result: Result<PrivateAppendOutcome, ScratchpadError>,
    },
    LoadActivePrivateRecord {
        expected_address: ScratchpadAddress,
        expected_version: Option<u64>,
        result: Result<PrivateRecordData, ScratchpadError>,
    },
    AcquireNewestActiveTarget {
        expected_address: ScratchpadAddress,
        result: Result<PrivateVersionTarget, ScratchpadError>,
    },
    AcquireExistingTarget {
        expected_address: ScratchpadAddress,
        expected_version: u64,
        result: Result<PrivateVersionTarget, ScratchpadError>,
    },
    EnumeratePrivateVersions {
        expected_invocation: Uuid,
        expected_name: Option<ScratchpadName>,
        expected_visibility: PrivateVisibility,
        result: Result<Vec<PrivateVersionMeta>, ScratchpadError>,
    },
    CollectCleanupEligibleVersions {
        offered_created_at: Vec<DateTime<Utc>>,
        expected_decisions: Vec<bool>,
        result: Result<Vec<PrivateVersionMeta>, ScratchpadError>,
    },
    RetirePrivateVersion {
        expected_target: PrivateVersionTarget,
        expected_actor: String,
        expected_reason: String,
        result: Result<PrivateRetirementOutcome, ScratchpadError>,
    },
    AppendCanonicalPublication {
        expected_draft: CanonicalPublicationDraft,
        result: Result<PublicationAppendOutcome, ScratchpadError>,
    },
}

struct StrictFakePersistence {
    events: SharedEvents,
}

impl ScratchpadPersistence for StrictFakePersistence {
    fn append_private_version(
        &self,
        draft: PrivateVersionDraft,
    ) -> Result<PrivateAppendOutcome, ScratchpadError> {
        match pop_event(&self.events, "append_private_version") {
            ScriptedEvent::AppendPrivateVersion {
                expected_draft,
                result,
            } => {
                assert_eq!(draft, expected_draft, "private append draft");
                result
            }
            other => unexpected_event("append_private_version", other),
        }
    }

    fn load_active_private_record(
        &self,
        address: &ScratchpadAddress,
        version: Option<u64>,
    ) -> Result<PrivateRecordData, ScratchpadError> {
        match pop_event(&self.events, "load_active_private_record") {
            ScriptedEvent::LoadActivePrivateRecord {
                expected_address,
                expected_version,
                result,
            } => {
                assert_eq!(address, &expected_address, "active record address");
                assert_eq!(version, expected_version, "active record version");
                result
            }
            other => unexpected_event("load_active_private_record", other),
        }
    }

    fn acquire_newest_active_target(
        &self,
        address: &ScratchpadAddress,
    ) -> Result<PrivateVersionTarget, ScratchpadError> {
        match pop_event(&self.events, "acquire_newest_active_target") {
            ScriptedEvent::AcquireNewestActiveTarget {
                expected_address,
                result,
            } => {
                assert_eq!(address, &expected_address, "newest target address");
                result
            }
            other => unexpected_event("acquire_newest_active_target", other),
        }
    }

    fn acquire_existing_target(
        &self,
        address: &ScratchpadAddress,
        version: u64,
    ) -> Result<PrivateVersionTarget, ScratchpadError> {
        match pop_event(&self.events, "acquire_existing_target") {
            ScriptedEvent::AcquireExistingTarget {
                expected_address,
                expected_version,
                result,
            } => {
                assert_eq!(address, &expected_address, "existing target address");
                assert_eq!(version, expected_version, "existing target version");
                result
            }
            other => unexpected_event("acquire_existing_target", other),
        }
    }

    fn enumerate_private_versions(
        &self,
        invocation_uuid: Uuid,
        name: Option<&ScratchpadName>,
        visibility: PrivateVisibility,
    ) -> Result<Vec<PrivateVersionMeta>, ScratchpadError> {
        match pop_event(&self.events, "enumerate_private_versions") {
            ScriptedEvent::EnumeratePrivateVersions {
                expected_invocation,
                expected_name,
                expected_visibility,
                result,
            } => {
                assert_eq!(
                    invocation_uuid, expected_invocation,
                    "enumeration invocation"
                );
                assert_eq!(name, expected_name.as_ref(), "enumeration name");
                assert_eq!(visibility, expected_visibility, "enumeration visibility");
                result
            }
            other => unexpected_event("enumerate_private_versions", other),
        }
    }

    fn collect_cleanup_eligible_versions(
        &self,
        eligible_by_age: &mut dyn FnMut(DateTime<Utc>) -> bool,
    ) -> Result<Vec<PrivateVersionMeta>, ScratchpadError> {
        match pop_event(&self.events, "collect_cleanup_eligible_versions") {
            ScriptedEvent::CollectCleanupEligibleVersions {
                offered_created_at,
                expected_decisions,
                result,
            } => {
                assert_eq!(
                    offered_created_at.len(),
                    expected_decisions.len(),
                    "every offered cleanup timestamp has one expected decision"
                );
                let actual_decisions = offered_created_at
                    .into_iter()
                    .map(eligible_by_age)
                    .collect::<Vec<_>>();
                assert_eq!(
                    actual_decisions, expected_decisions,
                    "cleanup callback decisions"
                );
                result
            }
            other => unexpected_event("collect_cleanup_eligible_versions", other),
        }
    }

    fn retire_private_version(
        &self,
        target: &PrivateVersionTarget,
        actor: &str,
        reason: &str,
    ) -> Result<PrivateRetirementOutcome, ScratchpadError> {
        match pop_event(&self.events, "retire_private_version") {
            ScriptedEvent::RetirePrivateVersion {
                expected_target,
                expected_actor,
                expected_reason,
                result,
            } => {
                assert_eq!(target, &expected_target, "retirement target");
                assert_eq!(actor, expected_actor, "retirement actor");
                assert_eq!(reason, expected_reason, "retirement reason");
                result
            }
            other => unexpected_event("retire_private_version", other),
        }
    }

    fn append_canonical_publication(
        &self,
        draft: CanonicalPublicationDraft,
    ) -> Result<PublicationAppendOutcome, ScratchpadError> {
        match pop_event(&self.events, "append_canonical_publication") {
            ScriptedEvent::AppendCanonicalPublication {
                expected_draft,
                result,
            } => {
                assert_eq!(draft, expected_draft, "canonical publication draft");
                result
            }
            other => unexpected_event("append_canonical_publication", other),
        }
    }
}

fn pop_event(events: &SharedEvents, operation: &str) -> ScriptedEvent {
    events.borrow_mut().pop_front().unwrap_or_else(|| {
        panic!("unexpected or post-failure {operation} call: script is exhausted")
    })
}

fn unexpected_event<T>(operation: &str, event: ScriptedEvent) -> T {
    panic!("unexpected or reordered {operation} call; next event was {event:?}")
}

struct DirectFixture {
    application: ScratchpadApplication<StrictFakePersistence, Box<dyn Fn() -> DateTime<Utc>>>,
    events: SharedEvents,
    utc_count: Rc<Cell<usize>>,
}

impl DirectFixture {
    fn assert_complete(&self, expected_utc_count: usize) {
        assert_eq!(
            self.utc_count.get(),
            expected_utc_count,
            "UTC observation count"
        );
        let remaining = self.events.borrow();
        assert!(
            remaining.is_empty(),
            "unconsumed scripted events: {remaining:?}"
        );
    }
}

fn direct_fixture(events: Vec<ScriptedEvent>) -> DirectFixture {
    let events = Rc::new(RefCell::new(events.into_iter().collect()));
    let utc_count = Rc::new(Cell::new(0));
    let fake = StrictFakePersistence {
        events: Rc::clone(&events),
    };
    let utc_events = Rc::clone(&events);
    let observed_count = Rc::clone(&utc_count);
    let observe_current_utc: Box<dyn Fn() -> DateTime<Utc>> = Box::new(move || {
        observed_count.set(observed_count.get() + 1);
        match pop_event(&utc_events, "observe_current_utc") {
            ScriptedEvent::ObserveUtc { returns } => returns,
            other => unexpected_event("observe_current_utc", other),
        }
    });

    DirectFixture {
        application: ScratchpadApplication::new(fake, observe_current_utc),
        events,
        utc_count,
    }
}

fn fixed_uuid(suffix: u128) -> Uuid {
    Uuid::from_u128(0x00000000000040008000000000000000 + suffix)
}

fn fixed_time(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .expect("valid fixed RFC3339 timestamp")
        .with_timezone(&Utc)
}

fn name(value: &str) -> ScratchpadName {
    ScratchpadName::new(value).expect("valid fixed scratchpad name")
}

fn address(invocation_uuid: Uuid, artifact_name: &str) -> ScratchpadAddress {
    ScratchpadAddress {
        invocation_uuid,
        name: name(artifact_name),
    }
}

fn canonical(workflow_run_id: &str, artifact_name: &str) -> CanonicalAddress {
    CanonicalAddress {
        workflow_run_id: workflow_run_id.to_string(),
        artifact_name: artifact_name.to_string(),
    }
}

fn private_meta(
    address: ScratchpadAddress,
    version: u64,
    producer_invocation_uuid: Option<Uuid>,
    created_at: DateTime<Utc>,
) -> PrivateVersionMeta {
    PrivateVersionMeta {
        address,
        version,
        sha256: format!("{version:064x}"),
        content_len: version * 10,
        producer_invocation_uuid,
        format_hint: Some(format!("format/{version}")),
        verdict_line: Some(format!("verdict {version}")),
        predecessor_version: (version > 1).then_some(version - 1),
        created_at,
        tombstone: None,
    }
}

fn private_record(meta: PrivateVersionMeta, content: &[u8]) -> PrivateRecordData {
    PrivateRecordData {
        meta,
        content: content.to_vec(),
    }
}

fn target(address: ScratchpadAddress, version: u64) -> PrivateVersionTarget {
    PrivateVersionTarget { address, version }
}

fn retirement(status: RetirementStatus, tombstoned_at: DateTime<Utc>) -> PrivateRetirementOutcome {
    PrivateRetirementOutcome {
        status,
        tombstoned_at,
    }
}

fn expected_public_meta(meta: &PrivateVersionMeta) -> ScratchpadMeta {
    let address = meta.address.clone();
    ScratchpadMeta {
        invocation_uuid: address.invocation_uuid,
        name: address.name.clone(),
        address,
        version: meta.version,
        sha256: meta.sha256.clone(),
        content_len: meta.content_len,
        producer_invocation_uuid: meta.producer_invocation_uuid,
        format_hint: meta.format_hint.clone(),
        verdict_line: meta.verdict_line.clone(),
        predecessor_version: meta.predecessor_version,
        created_at: meta.created_at,
        tombstone: meta.tombstone.as_ref().map(|value| TombstoneMeta {
            tombstoned_at: value.tombstoned_at,
            actor: value.actor.clone(),
            reason: value.reason.clone(),
        }),
    }
}

fn write_request(invocation_uuid: Uuid) -> WriteRequest {
    WriteRequest {
        scope: InvocationScope { invocation_uuid },
        name: name("write.md"),
        content: b"exact private bytes\0\n".to_vec(),
        format_hint: Some("application/octet-stream".to_string()),
        verdict_line: Some("WRITE_OK".to_string()),
        predecessor_version: Some(41),
    }
}

fn read_request(invocation_uuid: Uuid, artifact_name: &str, version: Option<u64>) -> ReadRequest {
    ReadRequest {
        scope: InvocationScope { invocation_uuid },
        name: name(artifact_name),
        version,
    }
}

fn delete_request(
    invocation_uuid: Uuid,
    artifact_name: &str,
    selector: DeleteSelector,
    actor: Option<&str>,
    reason: Option<&str>,
) -> DeleteRequest {
    DeleteRequest {
        scope: InvocationScope { invocation_uuid },
        name: name(artifact_name),
        selector,
        actor: actor.map(str::to_string),
        reason: reason.map(str::to_string),
    }
}

fn gc_request(
    selector: GcSelector,
    dry_run: bool,
    actor: Option<&str>,
    reason: Option<&str>,
) -> GcRequest {
    GcRequest {
        selector,
        dry_run,
        actor: actor.map(str::to_string),
        reason: reason.map(str::to_string),
    }
}

/// Intent: CORE-DIRECT-01.
/// Risk/finding lineage: S2, S3, nullable/authored lineage; R1-F01; R5-F02/PR-001.
/// Fixture source/application: fixed builders plus strict FIFO AppendPrivateVersion at write.
/// Observable: exact draft and result/error, optional outcome lineage, no later event.
mod core_direct_01 {
    use super::*;

    struct WriteSuccessScenario {
        fixture: DirectFixture,
        request: WriteRequest,
        expected: WriteReceipt,
    }

    fn write_success(producer_outcome: Option<Uuid>) -> WriteSuccessScenario {
        let invocation = fixed_uuid(1);
        let request = write_request(invocation);
        let address = address(invocation, "write.md");
        let created_at = fixed_time("2026-06-01T02:03:04.000000005Z");
        let expected_draft = PrivateVersionDraft {
            address: address.clone(),
            content: request.content.clone(),
            producer_invocation_uuid: invocation,
            format_hint: request.format_hint.clone(),
            verdict_line: request.verdict_line.clone(),
            predecessor_version: request.predecessor_version,
        };
        let outcome = PrivateAppendOutcome {
            version: 42,
            producer_invocation_uuid: producer_outcome,
            sha256: "write-sha".to_string(),
            content_len: request.content.len() as u64,
            format_hint: request.format_hint.clone(),
            verdict_line: request.verdict_line.clone(),
            predecessor_version: request.predecessor_version,
            created_at,
        };
        let expected = WriteReceipt {
            address,
            version: outcome.version,
            producer_invocation_uuid: producer_outcome,
            sha256: outcome.sha256.clone(),
            content_len: outcome.content_len,
            format_hint: outcome.format_hint.clone(),
            verdict_line: outcome.verdict_line.clone(),
            predecessor_version: outcome.predecessor_version,
            created_at,
        };
        WriteSuccessScenario {
            fixture: direct_fixture(vec![ScriptedEvent::AppendPrivateVersion {
                expected_draft,
                result: Ok(outcome),
            }]),
            request,
            expected,
        }
    }

    fn write_failure() -> (DirectFixture, WriteRequest) {
        let request = write_request(fixed_uuid(2));
        let expected_draft = PrivateVersionDraft {
            address: address(request.scope.invocation_uuid, "write.md"),
            content: request.content.clone(),
            producer_invocation_uuid: request.scope.invocation_uuid,
            format_hint: request.format_hint.clone(),
            verdict_line: request.verdict_line.clone(),
            predecessor_version: request.predecessor_version,
        };
        (
            direct_fixture(vec![ScriptedEvent::AppendPrivateVersion {
                expected_draft,
                result: Err(ScratchpadError::InvalidInput(
                    "scripted append failure".to_string(),
                )),
            }]),
            request,
        )
    }

    #[test]
    fn write_authors_request_lineage_and_copies_some_outcome_lineage() {
        let scenario = write_success(Some(fixed_uuid(91)));

        let actual = scenario
            .fixture
            .application
            .write(scenario.request)
            .expect("scripted write succeeds");

        assert_eq!(actual, scenario.expected);
        scenario.fixture.assert_complete(0);
    }

    #[test]
    fn write_preserves_none_append_outcome_lineage() {
        let scenario = write_success(None);

        let actual = scenario
            .fixture
            .application
            .write(scenario.request)
            .expect("scripted write succeeds");

        assert_eq!(actual, scenario.expected);
        assert_eq!(actual.producer_invocation_uuid, None);
        scenario.fixture.assert_complete(0);
    }

    #[test]
    fn write_returns_exact_capability_error_without_a_later_event() {
        let (fixture, request) = write_failure();

        let error = fixture
            .application
            .write(request)
            .expect_err("scripted write fails");

        assert!(matches!(
            error,
            ScratchpadError::InvalidInput(message) if message == "scripted append failure"
        ));
        fixture.assert_complete(0);
    }
}

/// Intent: CORE-DIRECT-02.
/// Risk/finding lineage: S2, S4, nullable lineage; R1-F01; R5-F02/PR-001.
/// Fixture source/application: fixed builders plus strict FIFO LoadActivePrivateRecord at read.
/// Observable: latest/exact arguments, exact bytes/public record, loaded None, stop on failure.
mod core_direct_02 {
    use super::*;

    struct ReadScenario {
        fixture: DirectFixture,
        request: ReadRequest,
        expected: ScratchpadRecord,
    }

    fn read_success(version: Option<u64>, returned_version: u64) -> ReadScenario {
        let invocation = fixed_uuid(3 + returned_version as u128);
        let request = read_request(invocation, "read.md", version);
        let address = address(invocation, "read.md");
        let mut meta = private_meta(
            address.clone(),
            returned_version,
            None,
            fixed_time("2026-06-02T03:04:05.000000006Z"),
        );
        meta.format_hint = None;
        meta.predecessor_version = None;
        let data = private_record(meta.clone(), b"exact read bytes\0\n");
        let expected = ScratchpadRecord {
            meta: expected_public_meta(&meta),
            content: data.content.clone(),
        };
        ReadScenario {
            fixture: direct_fixture(vec![ScriptedEvent::LoadActivePrivateRecord {
                expected_address: address,
                expected_version: version,
                result: Ok(data),
            }]),
            request,
            expected,
        }
    }

    fn read_failure() -> (DirectFixture, ReadRequest) {
        let invocation = fixed_uuid(8);
        let request = read_request(invocation, "missing.md", Some(77));
        (
            direct_fixture(vec![ScriptedEvent::LoadActivePrivateRecord {
                expected_address: address(invocation, "missing.md"),
                expected_version: Some(77),
                result: Err(ScratchpadError::NotFound),
            }]),
            request,
        )
    }

    #[test]
    fn read_latest_active_preserves_exact_bytes_and_none_lineage() {
        let scenario = read_success(None, 9);

        let actual = scenario
            .fixture
            .application
            .read(scenario.request)
            .expect("latest active read succeeds");

        assert_eq!(actual, scenario.expected);
        assert_eq!(actual.meta.producer_invocation_uuid, None);
        scenario.fixture.assert_complete(0);
    }

    #[test]
    fn read_exact_active_passes_the_requested_version() {
        let scenario = read_success(Some(4), 4);

        let actual = scenario
            .fixture
            .application
            .read(scenario.request)
            .expect("exact active read succeeds");

        assert_eq!(actual, scenario.expected);
        scenario.fixture.assert_complete(0);
    }

    #[test]
    fn read_returns_exact_load_error_without_a_later_event() {
        let (fixture, request) = read_failure();

        let error = fixture
            .application
            .read(request)
            .expect_err("scripted read fails");

        assert!(matches!(error, ScratchpadError::NotFound));
        fixture.assert_complete(0);
    }
}

/// Intent: CORE-DIRECT-03.
/// Risk/finding lineage: S2, S4, nullable lineage; R1-F01; R5-F02/PR-001.
/// Fixture source/application: fixed builders plus strict FIFO EnumeratePrivateVersions at list.
/// Observable: exact scope/name/visibility, supplied order, nullable lineage, all-or-error.
mod core_direct_03 {
    use super::*;

    struct ListScenario {
        fixture: DirectFixture,
        request: ListRequest,
        expected: Vec<ScratchpadMeta>,
    }

    fn ordered_list(include_tombstoned: bool, with_name: bool) -> ListScenario {
        let invocation = fixed_uuid(9);
        let requested_name = with_name.then(|| name("ordered.md"));
        let request = ListRequest {
            scope: InvocationScope {
                invocation_uuid: invocation,
            },
            name: requested_name.clone(),
            include_tombstoned,
        };
        let first = private_meta(
            address(invocation, "ordered.md"),
            3,
            None,
            fixed_time("2026-06-03T00:00:03Z"),
        );
        let mut second = private_meta(
            address(invocation, "ordered.md"),
            1,
            Some(fixed_uuid(99)),
            fixed_time("2026-06-03T00:00:01Z"),
        );
        if include_tombstoned {
            second.tombstone = Some(PrivateTombstone {
                tombstoned_at: fixed_time("2026-06-04T00:00:00Z"),
                actor: "list-actor".to_string(),
                reason: "list-reason".to_string(),
            });
        }
        let third = private_meta(
            address(invocation, "z-last.md"),
            2,
            None,
            fixed_time("2026-06-03T00:00:02Z"),
        );
        let returned = vec![first, second, third];
        let expected = returned.iter().map(expected_public_meta).collect();
        ListScenario {
            fixture: direct_fixture(vec![ScriptedEvent::EnumeratePrivateVersions {
                expected_invocation: invocation,
                expected_name: requested_name,
                expected_visibility: if include_tombstoned {
                    PrivateVisibility::IncludeTombstoned
                } else {
                    PrivateVisibility::ActiveOnly
                },
                result: Ok(returned),
            }]),
            request,
            expected,
        }
    }

    fn list_failure() -> (DirectFixture, ListRequest) {
        let invocation = fixed_uuid(10);
        let request = ListRequest {
            scope: InvocationScope {
                invocation_uuid: invocation,
            },
            name: None,
            include_tombstoned: false,
        };
        (
            direct_fixture(vec![ScriptedEvent::EnumeratePrivateVersions {
                expected_invocation: invocation,
                expected_name: None,
                expected_visibility: PrivateVisibility::ActiveOnly,
                result: Err(ScratchpadError::MetadataDecode(
                    "first ordered decode failure".to_string(),
                )),
            }]),
            request,
        )
    }

    #[test]
    fn list_active_only_preserves_supplied_order_and_nullable_lineage() {
        let scenario = ordered_list(false, true);

        let actual = scenario
            .fixture
            .application
            .list(scenario.request)
            .expect("active list succeeds");

        assert_eq!(actual, scenario.expected);
        assert_eq!(actual[0].producer_invocation_uuid, None);
        assert_eq!(actual[2].producer_invocation_uuid, None);
        scenario.fixture.assert_complete(0);
    }

    #[test]
    fn list_include_tombstoned_preserves_tombstone_and_complete_vector() {
        let scenario = ordered_list(true, false);

        let actual = scenario
            .fixture
            .application
            .list(scenario.request)
            .expect("include-tombstoned list succeeds");

        assert_eq!(actual, scenario.expected);
        assert!(actual[1].tombstone.is_some());
        scenario.fixture.assert_complete(0);
    }

    #[test]
    fn list_returns_first_capability_error_without_a_partial_vector() {
        let (fixture, request) = list_failure();

        let error = fixture
            .application
            .list(request)
            .expect_err("scripted enumeration fails");

        assert!(matches!(
            error,
            ScratchpadError::MetadataDecode(message)
                if message == "first ordered decode failure"
        ));
        fixture.assert_complete(0);
    }
}

/// Intent: CORE-DIRECT-04.
/// Risk/finding lineage: S5; R1-F02 -> R2-F01/SCOPE-RISK-001/CQ-C2-F01; R3-F02.
/// Fixture source/application: three strict selector-specific acquisition events at delete.
/// Observable: newest/exact-existing/all-active stay distinct; defaults and empty strings survive.
mod core_direct_04 {
    use super::*;

    struct DeleteScenario {
        fixture: DirectFixture,
        request: DeleteRequest,
        expected: DeleteReceipt,
    }

    fn latest_with_defaults() -> DeleteScenario {
        let invocation = fixed_uuid(11);
        let address = address(invocation, "latest.md");
        let selected = target(address.clone(), 7);
        let tombstoned_at = fixed_time("2026-06-05T00:00:07Z");
        DeleteScenario {
            fixture: direct_fixture(vec![
                ScriptedEvent::AcquireNewestActiveTarget {
                    expected_address: address.clone(),
                    result: Ok(selected.clone()),
                },
                ScriptedEvent::RetirePrivateVersion {
                    expected_target: selected,
                    expected_actor: "agent-scratchpad".to_string(),
                    expected_reason: "scratchpad delete".to_string(),
                    result: Ok(retirement(RetirementStatus::Retired, tombstoned_at)),
                },
            ]),
            request: delete_request(invocation, "latest.md", DeleteSelector::Latest, None, None),
            expected: DeleteReceipt {
                address,
                selector: DeleteSelector::Latest,
                tombstoned_versions: vec![7],
                already_tombstoned_versions: Vec::new(),
                actor: "agent-scratchpad".to_string(),
                reason: "scratchpad delete".to_string(),
                tombstoned_at: Some(tombstoned_at),
            },
        }
    }

    fn exact_existing_with_empty_values() -> DeleteScenario {
        let invocation = fixed_uuid(12);
        let address = address(invocation, "exact.md");
        let selected = target(address.clone(), 8);
        let persisted_time = fixed_time("2026-01-01T00:00:00.000000008Z");
        DeleteScenario {
            fixture: direct_fixture(vec![
                ScriptedEvent::AcquireExistingTarget {
                    expected_address: address.clone(),
                    expected_version: 8,
                    result: Ok(selected.clone()),
                },
                ScriptedEvent::RetirePrivateVersion {
                    expected_target: selected,
                    expected_actor: String::new(),
                    expected_reason: String::new(),
                    result: Ok(retirement(RetirementStatus::AlreadyRetired, persisted_time)),
                },
            ]),
            request: delete_request(
                invocation,
                "exact.md",
                DeleteSelector::Version(8),
                Some(""),
                Some(""),
            ),
            expected: DeleteReceipt {
                address,
                selector: DeleteSelector::Version(8),
                tombstoned_versions: Vec::new(),
                already_tombstoned_versions: vec![8],
                actor: String::new(),
                reason: String::new(),
                tombstoned_at: Some(persisted_time),
            },
        }
    }

    fn all_active_with_nullable_lineage() -> DeleteScenario {
        let invocation = fixed_uuid(13);
        let address = address(invocation, "all.md");
        let first_meta = private_meta(address.clone(), 2, None, fixed_time("2026-06-05T00:00:02Z"));
        let second_meta = private_meta(
            address.clone(),
            5,
            Some(fixed_uuid(105)),
            fixed_time("2026-06-05T00:00:05Z"),
        );
        let first_target = target(address.clone(), 2);
        let second_target = target(address.clone(), 5);
        let first_time = fixed_time("2026-06-06T00:00:02Z");
        let second_time = fixed_time("2026-06-06T00:00:05Z");
        DeleteScenario {
            fixture: direct_fixture(vec![
                ScriptedEvent::EnumeratePrivateVersions {
                    expected_invocation: invocation,
                    expected_name: Some(name("all.md")),
                    expected_visibility: PrivateVisibility::ActiveOnly,
                    result: Ok(vec![first_meta, second_meta]),
                },
                ScriptedEvent::RetirePrivateVersion {
                    expected_target: first_target,
                    expected_actor: "custom-delete".to_string(),
                    expected_reason: "all active".to_string(),
                    result: Ok(retirement(RetirementStatus::Retired, first_time)),
                },
                ScriptedEvent::RetirePrivateVersion {
                    expected_target: second_target,
                    expected_actor: "custom-delete".to_string(),
                    expected_reason: "all active".to_string(),
                    result: Ok(retirement(RetirementStatus::Retired, second_time)),
                },
            ]),
            request: delete_request(
                invocation,
                "all.md",
                DeleteSelector::AllVersions,
                Some("custom-delete"),
                Some("all active"),
            ),
            expected: DeleteReceipt {
                address,
                selector: DeleteSelector::AllVersions,
                tombstoned_versions: vec![2, 5],
                already_tombstoned_versions: Vec::new(),
                actor: "custom-delete".to_string(),
                reason: "all active".to_string(),
                tombstoned_at: Some(second_time),
            },
        }
    }

    fn failed_single_acquisition(selector: DeleteSelector) -> (DirectFixture, DeleteRequest) {
        let invocation = fixed_uuid(14);
        let artifact_name = "missing.md";
        let expected_address = address(invocation, artifact_name);
        let event = match selector {
            DeleteSelector::Latest => ScriptedEvent::AcquireNewestActiveTarget {
                expected_address,
                result: Err(ScratchpadError::NotFound),
            },
            DeleteSelector::Version(version) => ScriptedEvent::AcquireExistingTarget {
                expected_address,
                expected_version: version,
                result: Err(ScratchpadError::NotFound),
            },
            DeleteSelector::AllVersions => unreachable!("covered by CORE-DIRECT-06"),
        };
        (
            direct_fixture(vec![event]),
            delete_request(invocation, artifact_name, selector, None, None),
        )
    }

    #[test]
    fn delete_latest_uses_newest_active_acquisition_and_defaults() {
        let scenario = latest_with_defaults();

        let actual = scenario
            .fixture
            .application
            .delete(scenario.request)
            .expect("latest delete succeeds");

        assert_eq!(actual, scenario.expected);
        scenario.fixture.assert_complete(0);
    }

    #[test]
    fn delete_exact_existing_accepts_already_tombstoned_and_preserves_empty_values() {
        let scenario = exact_existing_with_empty_values();

        let actual = scenario
            .fixture
            .application
            .delete(scenario.request)
            .expect("exact-existing delete succeeds");

        assert_eq!(actual, scenario.expected);
        scenario.fixture.assert_complete(0);
    }

    #[test]
    fn delete_all_uses_active_enumeration_and_nullable_lineage_is_not_target_identity() {
        let scenario = all_active_with_nullable_lineage();

        let actual = scenario
            .fixture
            .application
            .delete(scenario.request)
            .expect("all-active delete succeeds");

        assert_eq!(actual, scenario.expected);
        scenario.fixture.assert_complete(0);
    }

    #[test]
    fn delete_latest_and_exact_stop_on_their_distinct_acquisition_errors() {
        for selector in [DeleteSelector::Latest, DeleteSelector::Version(81)] {
            let (fixture, request) = failed_single_acquisition(selector);

            let error = fixture
                .application
                .delete(request)
                .expect_err("scripted acquisition fails");

            assert!(matches!(error, ScratchpadError::NotFound));
            fixture.assert_complete(0);
        }
    }
}

/// Intent: CORE-DIRECT-05.
/// Risk/finding lineage: S5, S10; R1-F02 -> R2-F01/SCOPE-RISK-001/CQ-C2-F01;
/// R3-F02; R3-F02 -> R4-F01/SHORTCUT-RISK-001.
/// Fixture source/application: complete enumeration followed by three strict retire events.
/// Observable: complete-before-retire order, both STATUS outcomes, last exact Store time wins.
mod core_direct_05 {
    use super::*;

    fn mixed_status_delete() -> (DirectFixture, DeleteRequest, DeleteReceipt) {
        let invocation = fixed_uuid(15);
        let address = address(invocation, "mixed.md");
        let versions = [3, 4, 9];
        let metas = versions
            .iter()
            .map(|version| {
                private_meta(
                    address.clone(),
                    *version,
                    if *version == 4 {
                        None
                    } else {
                        Some(invocation)
                    },
                    fixed_time("2026-06-07T00:00:00Z"),
                )
            })
            .collect();
        let first_time = fixed_time("2026-12-31T23:59:59Z");
        let persisted_time = fixed_time("2025-01-01T00:00:00Z");
        let last_processed_time = fixed_time("2026-02-03T04:05:06.000000009Z");
        let events = vec![
            ScriptedEvent::EnumeratePrivateVersions {
                expected_invocation: invocation,
                expected_name: Some(name("mixed.md")),
                expected_visibility: PrivateVisibility::ActiveOnly,
                result: Ok(metas),
            },
            ScriptedEvent::RetirePrivateVersion {
                expected_target: target(address.clone(), 3),
                expected_actor: "agent-scratchpad".to_string(),
                expected_reason: "scratchpad delete".to_string(),
                result: Ok(retirement(RetirementStatus::Retired, first_time)),
            },
            ScriptedEvent::RetirePrivateVersion {
                expected_target: target(address.clone(), 4),
                expected_actor: "agent-scratchpad".to_string(),
                expected_reason: "scratchpad delete".to_string(),
                result: Ok(retirement(RetirementStatus::AlreadyRetired, persisted_time)),
            },
            ScriptedEvent::RetirePrivateVersion {
                expected_target: target(address.clone(), 9),
                expected_actor: "agent-scratchpad".to_string(),
                expected_reason: "scratchpad delete".to_string(),
                result: Ok(retirement(RetirementStatus::Retired, last_processed_time)),
            },
        ];
        (
            direct_fixture(events),
            delete_request(
                invocation,
                "mixed.md",
                DeleteSelector::AllVersions,
                None,
                None,
            ),
            DeleteReceipt {
                address,
                selector: DeleteSelector::AllVersions,
                tombstoned_versions: vec![3, 9],
                already_tombstoned_versions: vec![4],
                actor: "agent-scratchpad".to_string(),
                reason: "scratchpad delete".to_string(),
                tombstoned_at: Some(last_processed_time),
            },
        )
    }

    #[test]
    fn delete_retires_complete_targets_in_order_and_uses_each_status_and_time() {
        let (fixture, request, expected) = mixed_status_delete();

        let actual = fixture
            .application
            .delete(request)
            .expect("mixed-status delete succeeds");

        assert_eq!(actual, expected);
        fixture.assert_complete(0);
    }
}

/// Intent: CORE-DIRECT-06.
/// Risk/finding lineage: S5 and partial effects; R1-F02 -> R2-F01; R3-F02;
/// R5-F02/PR-001 evidence-class match.
/// Fixture source/application: strict three-target acquisition and every retirement index.
/// Observable: no early mutation, no receipt/later event after failure, empty-all zero retirements.
mod core_direct_06 {
    use super::*;

    struct DeleteFailureCase {
        fixture: DirectFixture,
        request: DeleteRequest,
        failure_message: String,
    }

    fn three_target_metas(invocation: Uuid, artifact_name: &str) -> Vec<PrivateVersionMeta> {
        let address = address(invocation, artifact_name);
        (1..=3)
            .map(|version| {
                private_meta(
                    address.clone(),
                    version,
                    if version == 2 { None } else { Some(invocation) },
                    fixed_time("2026-06-08T00:00:00Z"),
                )
            })
            .collect()
    }

    fn acquisition_failure() -> DeleteFailureCase {
        let invocation = fixed_uuid(16);
        let message = "all-target acquisition failed".to_string();
        DeleteFailureCase {
            fixture: direct_fixture(vec![ScriptedEvent::EnumeratePrivateVersions {
                expected_invocation: invocation,
                expected_name: Some(name("failure.md")),
                expected_visibility: PrivateVisibility::ActiveOnly,
                result: Err(ScratchpadError::MetadataDecode(message.clone())),
            }]),
            request: delete_request(
                invocation,
                "failure.md",
                DeleteSelector::AllVersions,
                None,
                None,
            ),
            failure_message: message,
        }
    }

    fn retirement_failure_cases() -> Vec<DeleteFailureCase> {
        (0..3)
            .map(|failure_index| {
                let invocation = fixed_uuid(20 + failure_index as u128);
                let artifact_name = "matrix.md";
                let address = address(invocation, artifact_name);
                let message = format!("retirement index {failure_index} failed");
                let mut events = vec![ScriptedEvent::EnumeratePrivateVersions {
                    expected_invocation: invocation,
                    expected_name: Some(name(artifact_name)),
                    expected_visibility: PrivateVisibility::ActiveOnly,
                    result: Ok(three_target_metas(invocation, artifact_name)),
                }];
                for index in 0..=failure_index {
                    let result = if index == failure_index {
                        Err(ScratchpadError::InvalidInput(message.clone()))
                    } else {
                        Ok(retirement(
                            RetirementStatus::Retired,
                            fixed_time("2026-06-09T00:00:00Z")
                                + TimeDelta::nanoseconds(index as i64),
                        ))
                    };
                    events.push(ScriptedEvent::RetirePrivateVersion {
                        expected_target: target(address.clone(), index as u64 + 1),
                        expected_actor: "matrix-actor".to_string(),
                        expected_reason: "matrix-reason".to_string(),
                        result,
                    });
                }
                DeleteFailureCase {
                    fixture: direct_fixture(events),
                    request: delete_request(
                        invocation,
                        artifact_name,
                        DeleteSelector::AllVersions,
                        Some("matrix-actor"),
                        Some("matrix-reason"),
                    ),
                    failure_message: message,
                }
            })
            .collect()
    }

    fn empty_all_delete() -> (DirectFixture, DeleteRequest, DeleteReceipt) {
        let invocation = fixed_uuid(24);
        let address = address(invocation, "empty.md");
        (
            direct_fixture(vec![ScriptedEvent::EnumeratePrivateVersions {
                expected_invocation: invocation,
                expected_name: Some(name("empty.md")),
                expected_visibility: PrivateVisibility::ActiveOnly,
                result: Ok(Vec::new()),
            }]),
            delete_request(
                invocation,
                "empty.md",
                DeleteSelector::AllVersions,
                Some(""),
                Some(""),
            ),
            DeleteReceipt {
                address,
                selector: DeleteSelector::AllVersions,
                tombstoned_versions: Vec::new(),
                already_tombstoned_versions: Vec::new(),
                actor: String::new(),
                reason: String::new(),
                tombstoned_at: None,
            },
        )
    }

    #[test]
    fn delete_acquisition_failure_performs_no_retirement() {
        let case = acquisition_failure();

        let error = case
            .fixture
            .application
            .delete(case.request)
            .expect_err("all-target acquisition fails");

        assert!(matches!(
            error,
            ScratchpadError::MetadataDecode(message) if message == case.failure_message
        ));
        case.fixture.assert_complete(0);
    }

    #[test]
    fn delete_failure_at_every_three_target_index_stops_without_a_receipt_or_later_call() {
        for case in retirement_failure_cases() {
            let error = case
                .fixture
                .application
                .delete(case.request)
                .expect_err("matrix retirement fails");

            assert!(matches!(
                error,
                ScratchpadError::InvalidInput(message) if message == case.failure_message
            ));
            case.fixture.assert_complete(0);
        }
    }

    #[test]
    fn delete_empty_all_with_empty_actor_and_reason_has_no_retirement() {
        let (fixture, request, expected) = empty_all_delete();

        let actual = fixture
            .application
            .delete(request)
            .expect("empty all-delete succeeds");

        assert_eq!(actual, expected);
        fixture.assert_complete(0);
    }
}

/// Intent: CORE-DIRECT-07.
/// Risk/finding lineage: S6; publication regression; R5-F02/PR-001.
/// Fixture source/application: empty strict queue at canonical destination validation.
/// Observable: exact reserved-prefix error before every capability event.
mod core_direct_07 {
    use super::*;

    fn invalid_publication() -> (DirectFixture, PublishRequest) {
        let invocation = fixed_uuid(25);
        (
            direct_fixture(Vec::new()),
            PublishRequest {
                source: address(invocation, "source.md"),
                source_version: None,
                destination: canonical("scratchpad:reserved", "destination.md"),
                format_hint: None,
                verdict_line: None,
                predecessor_version: None,
            },
        )
    }

    #[test]
    fn publish_rejects_reserved_destination_before_any_capability_event() {
        let (fixture, request) = invalid_publication();

        let error = fixture
            .application
            .publish(request)
            .expect_err("reserved destination fails");

        assert!(matches!(
            error,
            ScratchpadError::InvalidInput(message)
                if message
                    == "canonical workflow_run_id must not start with reserved prefix scratchpad:"
        ));
        fixture.assert_complete(0);
    }
}

/// Intent: CORE-DIRECT-08.
/// Risk/finding lineage: S6 and lineage; R1-F01; publication regression; R5-F02/PR-001.
/// Fixture source/application: strict active load then canonical append at publish.
/// Observable: latest/exact, fallback/override, request-only predecessor, source producer, stops.
mod core_direct_08 {
    use super::*;

    struct PublishScenario {
        fixture: DirectFixture,
        request: PublishRequest,
        expected: PublishReceipt,
    }

    fn publish_success(exact: bool, loaded_producer: Option<Uuid>) -> PublishScenario {
        let source_invocation = fixed_uuid(if exact { 27 } else { 26 });
        let foreign_producer = loaded_producer;
        let source = address(source_invocation, "source.md");
        let destination = canonical(
            "canonical-run",
            if exact { "exact.md" } else { "latest.md" },
        );
        let source_version = exact.then_some(12);
        let mut source_meta = private_meta(
            source.clone(),
            if exact { 12 } else { 11 },
            foreign_producer,
            fixed_time("2026-06-10T00:00:00Z"),
        );
        source_meta.format_hint = Some("source/format".to_string());
        source_meta.verdict_line = Some("SOURCE_VERDICT".to_string());
        source_meta.predecessor_version = Some(777);
        let record = private_record(source_meta.clone(), b"published exact bytes\0\n");
        let request = PublishRequest {
            source: source.clone(),
            source_version,
            destination: destination.clone(),
            format_hint: if exact {
                Some("request/format".to_string())
            } else {
                None
            },
            verdict_line: if exact {
                None
            } else {
                Some("REQUEST_VERDICT".to_string())
            },
            predecessor_version: Some(91),
        };
        let expected_format = if exact {
            Some("request/format".to_string())
        } else {
            Some("source/format".to_string())
        };
        let expected_verdict = if exact {
            Some("SOURCE_VERDICT".to_string())
        } else {
            Some("REQUEST_VERDICT".to_string())
        };
        let draft = CanonicalPublicationDraft {
            destination: destination.clone(),
            content: record.content.clone(),
            producer_invocation_uuid: source_invocation,
            format_hint: expected_format.clone(),
            verdict_line: expected_verdict.clone(),
            predecessor_version: Some(91),
        };
        let outcome = PublicationAppendOutcome {
            version: 21,
            sha256: "destination-sha".to_string(),
            content_len: record.content.len() as u64,
            format_hint: expected_format.clone(),
            verdict_line: expected_verdict.clone(),
            predecessor_version: Some(91),
            created_at: fixed_time("2026-06-11T01:02:03.000000004Z"),
        };
        let expected = PublishReceipt {
            source,
            source_version: source_meta.version,
            source_sha256: source_meta.sha256,
            destination,
            destination_version: outcome.version,
            destination_sha256: outcome.sha256.clone(),
            content_len: outcome.content_len,
            producer_invocation_uuid: source_invocation,
            format_hint: expected_format,
            verdict_line: expected_verdict,
            predecessor_version: Some(91),
            created_at: outcome.created_at,
        };
        PublishScenario {
            fixture: direct_fixture(vec![
                ScriptedEvent::LoadActivePrivateRecord {
                    expected_address: request.source.clone(),
                    expected_version: source_version,
                    result: Ok(record),
                },
                ScriptedEvent::AppendCanonicalPublication {
                    expected_draft: draft,
                    result: Ok(outcome),
                },
            ]),
            request,
            expected,
        }
    }

    fn load_failure() -> (DirectFixture, PublishRequest) {
        let invocation = fixed_uuid(28);
        let request = PublishRequest {
            source: address(invocation, "missing.md"),
            source_version: Some(4),
            destination: canonical("canonical-run", "missing.md"),
            format_hint: None,
            verdict_line: None,
            predecessor_version: None,
        };
        (
            direct_fixture(vec![ScriptedEvent::LoadActivePrivateRecord {
                expected_address: request.source.clone(),
                expected_version: Some(4),
                result: Err(ScratchpadError::NotFound),
            }]),
            request,
        )
    }

    fn append_failure() -> (DirectFixture, PublishRequest) {
        let invocation = fixed_uuid(29);
        let source = address(invocation, "append-failure.md");
        let mut meta = private_meta(source.clone(), 5, None, fixed_time("2026-06-12T00:00:00Z"));
        meta.format_hint = None;
        meta.verdict_line = None;
        meta.predecessor_version = Some(500);
        let record = private_record(meta, b"append failure bytes");
        let request = PublishRequest {
            source: source.clone(),
            source_version: Some(5),
            destination: canonical("canonical-run", "append-failure.md"),
            format_hint: None,
            verdict_line: None,
            predecessor_version: None,
        };
        let expected_draft = CanonicalPublicationDraft {
            destination: request.destination.clone(),
            content: record.content.clone(),
            producer_invocation_uuid: invocation,
            format_hint: None,
            verdict_line: None,
            predecessor_version: None,
        };
        (
            direct_fixture(vec![
                ScriptedEvent::LoadActivePrivateRecord {
                    expected_address: source,
                    expected_version: Some(5),
                    result: Ok(record),
                },
                ScriptedEvent::AppendCanonicalPublication {
                    expected_draft,
                    result: Err(ScratchpadError::Collision),
                },
            ]),
            request,
        )
    }

    #[test]
    fn publish_latest_uses_independent_fallback_and_ignores_loaded_none_lineage() {
        let scenario = publish_success(false, None);

        let actual = scenario
            .fixture
            .application
            .publish(scenario.request)
            .expect("latest publication succeeds");

        assert_eq!(actual, scenario.expected);
        scenario.fixture.assert_complete(0);
    }

    #[test]
    fn publish_exact_uses_independent_override_and_ignores_foreign_loaded_lineage() {
        let scenario = publish_success(true, Some(fixed_uuid(999)));

        let actual = scenario
            .fixture
            .application
            .publish(scenario.request)
            .expect("exact publication succeeds");

        assert_eq!(actual, scenario.expected);
        assert_ne!(actual.producer_invocation_uuid, fixed_uuid(999));
        scenario.fixture.assert_complete(0);
    }

    #[test]
    fn publish_load_failure_emits_no_append() {
        let (fixture, request) = load_failure();

        let error = fixture
            .application
            .publish(request)
            .expect_err("source load fails");

        assert!(matches!(error, ScratchpadError::NotFound));
        fixture.assert_complete(0);
    }

    #[test]
    fn publish_append_failure_returns_exact_error_without_a_later_event() {
        let (fixture, request) = append_failure();

        let error = fixture
            .application
            .publish(request)
            .expect_err("canonical append fails");

        assert!(matches!(error, ScratchpadError::Collision));
        fixture.assert_complete(0);
    }
}

/// Intent: CORE-DIRECT-09.
/// Risk/finding lineage: S8; UTC boundary; R5-F02/PR-001.
/// Fixture source/application: shared strict ObserveUtc/capability queue at every GC entry.
/// Observable: UTC is first and exactly once on success and on later failure.
mod core_direct_09 {
    use super::*;

    fn empty_success() -> (DirectFixture, GcRequest, DateTime<Utc>) {
        let invocation = fixed_uuid(30);
        let observed = fixed_time("2030-01-02T03:04:05.000000006Z");
        (
            direct_fixture(vec![
                ScriptedEvent::ObserveUtc { returns: observed },
                ScriptedEvent::EnumeratePrivateVersions {
                    expected_invocation: invocation,
                    expected_name: None,
                    expected_visibility: PrivateVisibility::ActiveOnly,
                    result: Ok(Vec::new()),
                },
            ]),
            gc_request(GcSelector::Invocation(invocation), false, None, None),
            observed,
        )
    }

    fn later_failure() -> (DirectFixture, GcRequest) {
        let invocation = fixed_uuid(31);
        let observed = fixed_time("2031-01-02T03:04:05Z");
        (
            direct_fixture(vec![
                ScriptedEvent::ObserveUtc { returns: observed },
                ScriptedEvent::EnumeratePrivateVersions {
                    expected_invocation: invocation,
                    expected_name: None,
                    expected_visibility: PrivateVisibility::ActiveOnly,
                    result: Err(ScratchpadError::MetadataDecode(
                        "failure after UTC".to_string(),
                    )),
                },
            ]),
            gc_request(GcSelector::Invocation(invocation), false, None, None),
        )
    }

    #[test]
    fn gc_uses_the_single_first_utc_value_only_as_successful_evaluated_at() {
        let (fixture, request, observed) = empty_success();

        let actual = fixture.application.gc(request).expect("empty GC succeeds");

        assert_eq!(actual.evaluated_at, observed);
        fixture.assert_complete(1);
    }

    #[test]
    fn gc_observes_utc_once_before_a_later_acquisition_failure() {
        let (fixture, request) = later_failure();

        let error = fixture
            .application
            .gc(request)
            .expect_err("candidate acquisition fails after UTC");

        assert!(matches!(
            error,
            ScratchpadError::MetadataDecode(message) if message == "failure after UTC"
        ));
        fixture.assert_complete(1);
    }
}

/// Intent: CORE-DIRECT-10.
/// Risk/finding lineage: S7, S10; R1-F01; R1-F02 -> R2-F01; R3-F02;
/// R3-F02 -> R4-F01/SHORTCUT-RISK-001.
/// Fixture source/application: ObserveUtc, invocation enumeration, then dry or live queue.
/// Observable: dry duplicate order/zero retirements; live order, nullable targets, both statuses.
mod core_direct_10 {
    use super::*;

    struct GcScenario {
        fixture: DirectFixture,
        request: GcRequest,
        expected: GcReport,
    }

    fn invocation_dry_run() -> GcScenario {
        let invocation = fixed_uuid(32);
        let first = address(invocation, "a.md");
        let second = address(invocation, "b.md");
        let metas = vec![
            private_meta(first.clone(), 1, None, fixed_time("2026-06-13T00:00:01Z")),
            private_meta(
                second.clone(),
                1,
                Some(invocation),
                fixed_time("2026-06-13T00:00:02Z"),
            ),
            private_meta(first.clone(), 2, None, fixed_time("2026-06-13T00:00:03Z")),
        ];
        let evaluated_at = fixed_time("2032-01-01T00:00:00Z");
        GcScenario {
            fixture: direct_fixture(vec![
                ScriptedEvent::ObserveUtc {
                    returns: evaluated_at,
                },
                ScriptedEvent::EnumeratePrivateVersions {
                    expected_invocation: invocation,
                    expected_name: None,
                    expected_visibility: PrivateVisibility::ActiveOnly,
                    result: Ok(metas),
                },
            ]),
            request: gc_request(GcSelector::Invocation(invocation), true, None, None),
            expected: GcReport {
                selector: GcSelector::Invocation(invocation),
                dry_run: true,
                tombstoned_rows: vec![first.clone(), second, first],
                already_tombstoned_rows: Vec::new(),
                actor: "agent-scratchpad-gc".to_string(),
                reason: "scratchpad gc invocation".to_string(),
                evaluated_at,
            },
        }
    }

    fn invocation_live() -> GcScenario {
        let invocation = fixed_uuid(33);
        let first = address(invocation, "a.md");
        let second = address(invocation, "b.md");
        let metas = vec![
            private_meta(first.clone(), 1, None, fixed_time("2026-06-14T00:00:01Z")),
            private_meta(second.clone(), 1, None, fixed_time("2026-06-14T00:00:02Z")),
            private_meta(first.clone(), 2, None, fixed_time("2026-06-14T00:00:03Z")),
        ];
        let evaluated_at = fixed_time("2033-01-01T00:00:00Z");
        GcScenario {
            fixture: direct_fixture(vec![
                ScriptedEvent::ObserveUtc {
                    returns: evaluated_at,
                },
                ScriptedEvent::EnumeratePrivateVersions {
                    expected_invocation: invocation,
                    expected_name: None,
                    expected_visibility: PrivateVisibility::ActiveOnly,
                    result: Ok(metas),
                },
                ScriptedEvent::RetirePrivateVersion {
                    expected_target: target(first.clone(), 1),
                    expected_actor: "live-actor".to_string(),
                    expected_reason: "live-reason".to_string(),
                    result: Ok(retirement(
                        RetirementStatus::Retired,
                        fixed_time("2026-06-15T00:00:01Z"),
                    )),
                },
                ScriptedEvent::RetirePrivateVersion {
                    expected_target: target(second.clone(), 1),
                    expected_actor: "live-actor".to_string(),
                    expected_reason: "live-reason".to_string(),
                    result: Ok(retirement(
                        RetirementStatus::AlreadyRetired,
                        fixed_time("2025-06-15T00:00:02Z"),
                    )),
                },
                ScriptedEvent::RetirePrivateVersion {
                    expected_target: target(first.clone(), 2),
                    expected_actor: "live-actor".to_string(),
                    expected_reason: "live-reason".to_string(),
                    result: Ok(retirement(
                        RetirementStatus::Retired,
                        fixed_time("2026-06-15T00:00:03Z"),
                    )),
                },
            ]),
            request: gc_request(
                GcSelector::Invocation(invocation),
                false,
                Some("live-actor"),
                Some("live-reason"),
            ),
            expected: GcReport {
                selector: GcSelector::Invocation(invocation),
                dry_run: false,
                tombstoned_rows: vec![first.clone(), first],
                already_tombstoned_rows: vec![second],
                actor: "live-actor".to_string(),
                reason: "live-reason".to_string(),
                evaluated_at,
            },
        }
    }

    #[test]
    fn invocation_gc_dry_run_preserves_duplicate_addresses_and_retires_nothing() {
        let scenario = invocation_dry_run();

        let actual = scenario
            .fixture
            .application
            .gc(scenario.request)
            .expect("dry-run GC succeeds");

        assert_eq!(actual, scenario.expected);
        scenario.fixture.assert_complete(1);
    }

    #[test]
    fn invocation_gc_live_retires_in_order_and_integrates_both_statuses() {
        let scenario = invocation_live();

        let actual = scenario
            .fixture
            .application
            .gc(scenario.request)
            .expect("live GC succeeds");

        assert_eq!(actual, scenario.expected);
        scenario.fixture.assert_complete(1);
    }
}

/// Intent: CORE-DIRECT-11.
/// Risk/finding lineage: S7 and partial effects; R1-F02 -> R2-F01; R3-F02;
/// R5-F02/PR-001 evidence-class match.
/// Fixture source/application: strict three-target GC acquisition/every retirement index.
/// Observable: complete acquisition, no report/later event, empty/dry zero retirements.
mod core_direct_11 {
    use super::*;

    struct GcFailureCase {
        fixture: DirectFixture,
        request: GcRequest,
        failure_message: String,
    }

    fn three_target_metas(invocation: Uuid) -> Vec<PrivateVersionMeta> {
        let first = address(invocation, "a.md");
        let second = address(invocation, "b.md");
        vec![
            private_meta(first.clone(), 1, None, fixed_time("2026-06-16T00:00:01Z")),
            private_meta(second, 1, None, fixed_time("2026-06-16T00:00:02Z")),
            private_meta(first, 2, None, fixed_time("2026-06-16T00:00:03Z")),
        ]
    }

    fn acquisition_failure() -> GcFailureCase {
        let cutoff = fixed_time("2026-06-20T00:00:00Z");
        let message = "cleanup collection failed".to_string();
        GcFailureCase {
            fixture: direct_fixture(vec![
                ScriptedEvent::ObserveUtc {
                    returns: fixed_time("2034-01-01T00:00:00Z"),
                },
                ScriptedEvent::CollectCleanupEligibleVersions {
                    offered_created_at: vec![cutoff - TimeDelta::days(8)],
                    expected_decisions: vec![true],
                    result: Err(ScratchpadError::MetadataDecode(message.clone())),
                },
            ]),
            request: gc_request(GcSelector::ExpiredBefore(cutoff), false, None, None),
            failure_message: message,
        }
    }

    fn retirement_failure_cases() -> Vec<GcFailureCase> {
        (0..3)
            .map(|failure_index| {
                let invocation = fixed_uuid(40 + failure_index as u128);
                let metas = three_target_metas(invocation);
                let targets = metas
                    .iter()
                    .map(|meta| target(meta.address.clone(), meta.version))
                    .collect::<Vec<_>>();
                let message = format!("GC retirement index {failure_index} failed");
                let mut events = vec![
                    ScriptedEvent::ObserveUtc {
                        returns: fixed_time("2035-01-01T00:00:00Z"),
                    },
                    ScriptedEvent::EnumeratePrivateVersions {
                        expected_invocation: invocation,
                        expected_name: None,
                        expected_visibility: PrivateVisibility::ActiveOnly,
                        result: Ok(metas),
                    },
                ];
                for (index, selected) in targets.into_iter().enumerate().take(failure_index + 1) {
                    let result = if index == failure_index {
                        Err(ScratchpadError::InvalidInput(message.clone()))
                    } else {
                        Ok(retirement(
                            RetirementStatus::Retired,
                            fixed_time("2026-06-17T00:00:00Z")
                                + TimeDelta::nanoseconds(index as i64),
                        ))
                    };
                    events.push(ScriptedEvent::RetirePrivateVersion {
                        expected_target: selected,
                        expected_actor: "gc-matrix".to_string(),
                        expected_reason: "gc-matrix-reason".to_string(),
                        result,
                    });
                }
                GcFailureCase {
                    fixture: direct_fixture(events),
                    request: gc_request(
                        GcSelector::Invocation(invocation),
                        false,
                        Some("gc-matrix"),
                        Some("gc-matrix-reason"),
                    ),
                    failure_message: message,
                }
            })
            .collect()
    }

    fn empty_case(dry_run: bool) -> (DirectFixture, GcRequest, GcReport) {
        let invocation = fixed_uuid(if dry_run { 44 } else { 43 });
        let evaluated_at = fixed_time(if dry_run {
            "2044-01-01T00:00:00Z"
        } else {
            "2043-01-01T00:00:00Z"
        });
        (
            direct_fixture(vec![
                ScriptedEvent::ObserveUtc {
                    returns: evaluated_at,
                },
                ScriptedEvent::EnumeratePrivateVersions {
                    expected_invocation: invocation,
                    expected_name: None,
                    expected_visibility: PrivateVisibility::ActiveOnly,
                    result: Ok(Vec::new()),
                },
            ]),
            gc_request(
                GcSelector::Invocation(invocation),
                dry_run,
                Some(""),
                Some(""),
            ),
            GcReport {
                selector: GcSelector::Invocation(invocation),
                dry_run,
                tombstoned_rows: Vec::new(),
                already_tombstoned_rows: Vec::new(),
                actor: String::new(),
                reason: String::new(),
                evaluated_at,
            },
        )
    }

    #[test]
    fn gc_acquisition_failure_performs_no_retirement() {
        let case = acquisition_failure();

        let error = case
            .fixture
            .application
            .gc(case.request)
            .expect_err("cleanup collection fails");

        assert!(matches!(
            error,
            ScratchpadError::MetadataDecode(message) if message == case.failure_message
        ));
        case.fixture.assert_complete(1);
    }

    #[test]
    fn gc_failure_at_every_three_target_index_stops_without_a_report_or_later_call() {
        for case in retirement_failure_cases() {
            let error = case
                .fixture
                .application
                .gc(case.request)
                .expect_err("matrix retirement fails");

            assert!(matches!(
                error,
                ScratchpadError::InvalidInput(message) if message == case.failure_message
            ));
            case.fixture.assert_complete(1);
        }
    }

    #[test]
    fn gc_empty_live_with_empty_actor_and_reason_has_no_retirement() {
        let (fixture, request, expected) = empty_case(false);

        let actual = fixture
            .application
            .gc(request)
            .expect("empty live GC succeeds");

        assert_eq!(actual, expected);
        fixture.assert_complete(1);
    }

    #[test]
    fn gc_empty_dry_run_with_empty_actor_and_reason_has_no_retirement() {
        let (fixture, request, expected) = empty_case(true);

        let actual = fixture
            .application
            .gc(request)
            .expect("empty dry-run GC succeeds");

        assert_eq!(actual, expected);
        fixture.assert_complete(1);
    }
}

/// Intent: CORE-DIRECT-12.
/// Risk/finding lineage: S7 and inclusive TTL; R1-F03; R1-F01; R5-F02/PR-001.
/// Fixture source/application: strict cleanup timestamps/decisions after first fixed UTC.
/// Observable: equality included, one nanosecond newer excluded, caller cutoff/defaults exact.
mod core_direct_12 {
    use super::*;

    fn ttl_boundary_scenario() -> (DirectFixture, GcRequest, GcReport) {
        let cutoff = fixed_time("2026-07-08T00:00:00Z");
        let equality = cutoff - TimeDelta::days(7);
        let one_nanosecond_newer = equality + TimeDelta::nanoseconds(1);
        let older = equality - TimeDelta::nanoseconds(1);
        let current_utc = fixed_time("2099-12-31T23:59:59Z");
        let invocation = fixed_uuid(45);
        let equality_address = address(invocation, "equality.md");
        let older_address = address(invocation, "older.md");
        let returned = vec![
            private_meta(equality_address.clone(), 1, None, equality),
            private_meta(older_address.clone(), 1, None, older),
        ];
        (
            direct_fixture(vec![
                ScriptedEvent::ObserveUtc {
                    returns: current_utc,
                },
                ScriptedEvent::CollectCleanupEligibleVersions {
                    offered_created_at: vec![equality, one_nanosecond_newer, older],
                    expected_decisions: vec![true, false, true],
                    result: Ok(returned),
                },
            ]),
            gc_request(GcSelector::ExpiredBefore(cutoff), true, None, None),
            GcReport {
                selector: GcSelector::ExpiredBefore(cutoff),
                dry_run: true,
                tombstoned_rows: vec![equality_address, older_address],
                already_tombstoned_rows: Vec::new(),
                actor: "agent-scratchpad-gc".to_string(),
                reason: "scratchpad gc expired".to_string(),
                evaluated_at: current_utc,
            },
        )
    }

    #[test]
    fn expired_gc_uses_inclusive_seven_day_cutoff_not_current_utc() {
        let (fixture, request, expected) = ttl_boundary_scenario();

        let actual = fixture
            .application
            .gc(request)
            .expect("TTL boundary dry-run succeeds");

        assert_eq!(actual, expected);
        fixture.assert_complete(1);
    }
}

struct AdapterHarness {
    _dir: TempDir,
    path: PathBuf,
    adapter: StoreScratchpadPersistence,
}

impl AdapterHarness {
    fn observer(&self) -> Store {
        Store::open(&self.path).expect("open Store observer")
    }
}

fn adapter_fixture<T>(seed: impl FnOnce(&Store, &Path) -> T) -> (AdapterHarness, T) {
    let dir = tempfile::tempdir().expect("temporary Store directory");
    let path = dir.path().join("adapter.sqlite");
    let store = Store::init(&path).expect("initialize real temporary Store");
    let seeded = seed(&store, &path);
    (
        AdapterHarness {
            _dir: dir,
            path,
            adapter: StoreScratchpadPersistence::new(store),
        },
        seeded,
    )
}

fn scratchpad_workflow(invocation_uuid: Uuid) -> String {
    format!("scratchpad:{invocation_uuid}")
}

fn store_key(workflow_run_id: &str, artifact_name: &str) -> ArtifactKey {
    ArtifactKey {
        workflow_run_id: workflow_run_id.to_string(),
        artifact_name: artifact_name.to_string(),
    }
}

fn put_store_row(
    store: &Store,
    workflow_run_id: &str,
    artifact_name: &str,
    producer_invocation_uuid: Option<Uuid>,
    content: &[u8],
) -> PutReceipt {
    store
        .put(PutRequest {
            key: store_key(workflow_run_id, artifact_name),
            producer_invocation_uuid,
            format_hint: Some("fixture/format".to_string()),
            verdict_line: Some("FIXTURE_VERDICT".to_string()),
            predecessor_version: None,
            content: content.to_vec(),
        })
        .expect("seed Store row")
}

fn set_store_created_at(path: &Path, key: &ArtifactKey, version: u64, created_at: DateTime<Utc>) {
    let connection = rusqlite::Connection::open(path).expect("open timestamp fixture connection");
    connection
        .execute(
            "UPDATE artifact_versions SET created_at = ?1 \
             WHERE workflow_run_id = ?2 AND artifact_name = ?3 AND version = ?4",
            params![
                created_at.to_rfc3339(),
                key.workflow_run_id,
                key.artifact_name,
                version as i64,
            ],
        )
        .expect("set controlled Store timestamp");
}

/// Intent: CORE-CAP-01.
/// Risk/finding lineage: S9, S10; R1-F01; R1-F05; R3-F03;
/// R3-F02 -> R4-F01/SHORTCUT-RISK-001; R5-F02/PR-001.
/// Fixture source/application: real temporary Store mirrored from tests/common at adapter methods.
/// Observable: all eight operations, exact translation/errors, visibility/order/nullability/status.
mod core_cap_01 {
    use super::*;

    struct AppendScenario {
        harness: AdapterHarness,
        private_draft: PrivateVersionDraft,
        canonical_draft: CanonicalPublicationDraft,
    }

    fn append_scenario() -> AppendScenario {
        let (harness, ()) = adapter_fixture(|_, _| ());
        let invocation = fixed_uuid(50);
        AppendScenario {
            harness,
            private_draft: PrivateVersionDraft {
                address: address(invocation, "private-append.md"),
                content: b"private adapter bytes\0\n".to_vec(),
                producer_invocation_uuid: invocation,
                format_hint: Some("private/format".to_string()),
                verdict_line: Some("PRIVATE_OK".to_string()),
                predecessor_version: Some(7),
            },
            canonical_draft: CanonicalPublicationDraft {
                destination: canonical("canonical-run", "published.md"),
                content: b"canonical adapter bytes\0\n".to_vec(),
                producer_invocation_uuid: invocation,
                format_hint: Some("canonical/format".to_string()),
                verdict_line: Some("CANONICAL_OK".to_string()),
                predecessor_version: Some(8),
            },
        }
    }

    struct ActiveScenario {
        harness: AdapterHarness,
        address: ScratchpadAddress,
        first: PutReceipt,
        second: PutReceipt,
        first_content: Vec<u8>,
    }

    fn active_scenario() -> ActiveScenario {
        let invocation = fixed_uuid(51);
        let first_content = b"first active bytes".to_vec();
        let second_content = b"second tombstoned bytes".to_vec();
        let ((harness, (first, second)), address) = {
            let address = address(invocation, "history.md");
            let result = adapter_fixture(|store, _| {
                let workflow = scratchpad_workflow(invocation);
                let first = put_store_row(store, &workflow, "history.md", None, &first_content);
                let second = put_store_row(
                    store,
                    &workflow,
                    "history.md",
                    Some(fixed_uuid(510)),
                    &second_content,
                );
                store
                    .tombstone(&second.key, second.version, "seed-actor", "seed-reason")
                    .expect("tombstone newest row");
                (first, second)
            });
            (result, address)
        };
        ActiveScenario {
            harness,
            address,
            first,
            second,
            first_content,
        }
    }

    struct EnumerationScenario {
        harness: AdapterHarness,
        invocation: Uuid,
        a_first: PutReceipt,
        a_second: PutReceipt,
        b_first: PutReceipt,
    }

    fn enumeration_scenario() -> EnumerationScenario {
        let invocation = fixed_uuid(52);
        let (harness, (a_first, a_second, b_first)) = adapter_fixture(|store, _| {
            let workflow = scratchpad_workflow(invocation);
            let b_first = put_store_row(store, &workflow, "b.md", Some(invocation), b"b-one");
            let a_first = put_store_row(store, &workflow, "a.md", None, b"a-one");
            let a_second = put_store_row(store, &workflow, "a.md", Some(fixed_uuid(520)), b"a-two");
            store
                .tombstone(
                    &a_second.key,
                    a_second.version,
                    "enumeration-actor",
                    "enumeration-reason",
                )
                .expect("seed tombstoned enumerated row");
            (a_first, a_second, b_first)
        });
        EnumerationScenario {
            harness,
            invocation,
            a_first,
            a_second,
            b_first,
        }
    }

    struct RetirementScenario {
        harness: AdapterHarness,
        target: PrivateVersionTarget,
    }

    fn retirement_scenario() -> RetirementScenario {
        let invocation = fixed_uuid(53);
        let address = address(invocation, "retire.md");
        let (harness, receipt) = adapter_fixture(|store, _| {
            put_store_row(
                store,
                &scratchpad_workflow(invocation),
                "retire.md",
                None,
                b"retire bytes",
            )
        });
        RetirementScenario {
            harness,
            target: target(address, receipt.version),
        }
    }

    struct CollectorScenario {
        harness: AdapterHarness,
        offered_time: DateTime<Utc>,
        expected_address: ScratchpadAddress,
    }

    fn collector_scenario() -> CollectorScenario {
        let invocation = fixed_uuid(54);
        let created_at = fixed_time("2026-01-01T00:00:00Z");
        let (harness, _receipt) = adapter_fixture(|store, path| {
            let receipt = put_store_row(
                store,
                &scratchpad_workflow(invocation),
                "collect.md",
                None,
                b"collect bytes",
            );
            set_store_created_at(path, &receipt.key, receipt.version, created_at);
            receipt
        });
        CollectorScenario {
            harness,
            offered_time: created_at,
            expected_address: address(invocation, "collect.md"),
        }
    }

    #[test]
    fn real_adapter_translates_private_and_canonical_append_operations_exactly() {
        let scenario = append_scenario();
        let expected_private = scenario.private_draft.clone();
        let expected_canonical = scenario.canonical_draft.clone();

        let private = scenario
            .harness
            .adapter
            .append_private_version(scenario.private_draft)
            .expect("private adapter append succeeds");
        let canonical = scenario
            .harness
            .adapter
            .append_canonical_publication(scenario.canonical_draft)
            .expect("canonical adapter append succeeds");
        let observer = scenario.harness.observer();
        let stored_private = observer
            .get(
                &store_key(
                    &scratchpad_workflow(expected_private.address.invocation_uuid),
                    expected_private.address.name.as_str(),
                ),
                Some(private.version),
            )
            .expect("observe private append");
        let stored_canonical = observer
            .get(
                &store_key(
                    &expected_canonical.destination.workflow_run_id,
                    &expected_canonical.destination.artifact_name,
                ),
                Some(canonical.version),
            )
            .expect("observe canonical append");

        assert_eq!(stored_private.content, expected_private.content);
        assert_eq!(
            private.producer_invocation_uuid,
            Some(expected_private.producer_invocation_uuid)
        );
        assert_eq!(private.sha256, stored_private.meta.sha256);
        assert_eq!(private.content_len, stored_private.meta.content_len);
        assert_eq!(private.format_hint, expected_private.format_hint);
        assert_eq!(private.verdict_line, expected_private.verdict_line);
        assert_eq!(
            private.predecessor_version,
            expected_private.predecessor_version
        );
        assert_eq!(private.created_at, stored_private.meta.created_at);
        assert_eq!(stored_canonical.content, expected_canonical.content);
        assert_eq!(
            stored_canonical.meta.producer_invocation_uuid,
            Some(expected_canonical.producer_invocation_uuid)
        );
        assert_eq!(canonical.sha256, stored_canonical.meta.sha256);
        assert_eq!(canonical.content_len, stored_canonical.meta.content_len);
        assert_eq!(canonical.format_hint, expected_canonical.format_hint);
        assert_eq!(canonical.verdict_line, expected_canonical.verdict_line);
        assert_eq!(
            canonical.predecessor_version,
            expected_canonical.predecessor_version
        );
        assert_eq!(canonical.created_at, stored_canonical.meta.created_at);
    }

    #[test]
    fn real_adapter_distinguishes_active_load_newest_target_and_exact_existing_target() {
        let scenario = active_scenario();

        let latest = scenario
            .harness
            .adapter
            .load_active_private_record(&scenario.address, None)
            .expect("newest active record loads");
        let exact_active = scenario
            .harness
            .adapter
            .load_active_private_record(&scenario.address, Some(scenario.first.version))
            .expect("exact active record loads");
        let tombstoned_load_error = scenario
            .harness
            .adapter
            .load_active_private_record(&scenario.address, Some(scenario.second.version))
            .expect_err("active load hides exact tombstone");
        let newest_target = scenario
            .harness
            .adapter
            .acquire_newest_active_target(&scenario.address)
            .expect("newest active target resolves");
        let exact_existing_target = scenario
            .harness
            .adapter
            .acquire_existing_target(&scenario.address, scenario.second.version)
            .expect("exact existing target sees tombstone");

        assert_eq!(latest.content, scenario.first_content);
        assert_eq!(latest.meta.version, scenario.first.version);
        assert_eq!(latest.meta.producer_invocation_uuid, None);
        assert_eq!(exact_active, latest);
        assert!(matches!(tombstoned_load_error, ScratchpadError::NotFound));
        assert_eq!(
            newest_target,
            target(scenario.address.clone(), scenario.first.version)
        );
        assert_eq!(
            exact_existing_target,
            target(scenario.address, scenario.second.version)
        );
    }

    #[test]
    fn real_adapter_enumeration_preserves_store_order_visibility_and_nullable_lineage() {
        let scenario = enumeration_scenario();

        let active = scenario
            .harness
            .adapter
            .enumerate_private_versions(scenario.invocation, None, PrivateVisibility::ActiveOnly)
            .expect("active enumeration succeeds");
        let all = scenario
            .harness
            .adapter
            .enumerate_private_versions(
                scenario.invocation,
                None,
                PrivateVisibility::IncludeTombstoned,
            )
            .expect("include-tombstoned enumeration succeeds");
        let named = scenario
            .harness
            .adapter
            .enumerate_private_versions(
                scenario.invocation,
                Some(&name("a.md")),
                PrivateVisibility::IncludeTombstoned,
            )
            .expect("named enumeration succeeds");

        assert_eq!(
            active
                .iter()
                .map(|meta| (meta.address.name.as_str(), meta.version))
                .collect::<Vec<_>>(),
            vec![
                ("a.md", scenario.a_first.version),
                ("b.md", scenario.b_first.version),
            ]
        );
        assert_eq!(active[0].producer_invocation_uuid, None);
        assert_eq!(
            all.iter()
                .map(|meta| (meta.address.name.as_str(), meta.version))
                .collect::<Vec<_>>(),
            vec![
                ("a.md", scenario.a_first.version),
                ("a.md", scenario.a_second.version),
                ("b.md", scenario.b_first.version),
            ]
        );
        assert!(all[0].tombstone.is_none());
        let translated_tombstone = all[1].tombstone.as_ref().expect("translated tombstone");
        assert_eq!(translated_tombstone.actor, "enumeration-actor");
        assert_eq!(translated_tombstone.reason, "enumeration-reason");
        assert_eq!(named.as_slice(), &all[..2]);
    }

    #[test]
    fn real_adapter_retirement_translates_both_statuses_exact_time_tombstone_and_error() {
        let scenario = retirement_scenario();

        let retired = scenario
            .harness
            .adapter
            .retire_private_version(&scenario.target, "adapter-actor", "adapter-reason")
            .expect("first retirement succeeds");
        let replayed = scenario
            .harness
            .adapter
            .retire_private_version(&scenario.target, "ignored-actor", "ignored-reason")
            .expect("idempotent retirement succeeds");
        let invalid = scenario
            .harness
            .adapter
            .retire_private_version(&scenario.target, "", "reason")
            .expect_err("Store validation error is projected");
        let rows = scenario
            .harness
            .adapter
            .enumerate_private_versions(
                scenario.target.address.invocation_uuid,
                Some(&scenario.target.address.name),
                PrivateVisibility::IncludeTombstoned,
            )
            .expect("observe translated tombstone");

        assert_eq!(retired.status, RetirementStatus::Retired);
        assert_eq!(replayed.status, RetirementStatus::AlreadyRetired);
        assert_eq!(replayed.tombstoned_at, retired.tombstoned_at);
        assert!(matches!(
            invalid,
            ScratchpadError::InvalidInput(message) if message == "actor must not be empty"
        ));
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].tombstone,
            Some(PrivateTombstone {
                tombstoned_at: retired.tombstoned_at,
                actor: "adapter-actor".to_string(),
                reason: "adapter-reason".to_string(),
            })
        );
    }

    #[test]
    fn real_adapter_cleanup_operation_returns_only_callback_accepted_domain_rows() {
        let scenario = collector_scenario();
        let mut offered = Vec::new();

        let rows = scenario
            .harness
            .adapter
            .collect_cleanup_eligible_versions(&mut |created_at| {
                offered.push(created_at);
                true
            })
            .expect("collector succeeds");

        assert_eq!(offered, vec![scenario.offered_time]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].address, scenario.expected_address);
        assert_eq!(rows[0].producer_invocation_uuid, None);
    }

    #[test]
    fn real_adapter_projects_missing_store_rows_to_exact_scratchpad_error() {
        let (harness, ()) = adapter_fixture(|_, _| ());
        let missing = address(fixed_uuid(55), "missing.md");

        let load_error = harness
            .adapter
            .load_active_private_record(&missing, None)
            .expect_err("missing load fails");
        let newest_error = harness
            .adapter
            .acquire_newest_active_target(&missing)
            .expect_err("missing newest target fails");
        let exact_error = harness
            .adapter
            .acquire_existing_target(&missing, 99)
            .expect_err("missing exact target fails");

        assert!(matches!(load_error, ScratchpadError::NotFound));
        assert!(matches!(newest_error, ScratchpadError::NotFound));
        assert!(matches!(exact_error, ScratchpadError::NotFound));
    }
}

/// Intent: CORE-CAP-02.
/// Risk/finding lineage: S7, S9; R1-F03; R1-F01; R1-F05; R3-F03; R5-F02/PR-001.
/// Fixture source/application: real SQLite controlled timestamps/malformed rows plus callback.
/// Observable: active global list -> prefix reject -> callback -> accepted decode -> complete result.
mod core_cap_02 {
    use super::*;

    struct CleanupOrderScenario {
        harness: AdapterHarness,
        old_valid: DateTime<Utc>,
        fresh_malformed: DateTime<Utc>,
        expected_address: ScratchpadAddress,
    }

    fn cleanup_order_scenario() -> CleanupOrderScenario {
        let invocation = fixed_uuid(56);
        let old_valid = fixed_time("2026-01-01T00:00:00Z");
        let fresh_malformed = fixed_time("2026-12-31T00:00:00Z");
        let (harness, ()) = adapter_fixture(|store, path| {
            let canonical =
                put_store_row(store, "canonical-run", "a-canonical.md", None, b"canonical");
            set_store_created_at(path, &canonical.key, canonical.version, old_valid);

            let valid = put_store_row(
                store,
                &scratchpad_workflow(invocation),
                "a-valid.md",
                None,
                b"valid nullable",
            );
            set_store_created_at(path, &valid.key, valid.version, old_valid);

            let tombstoned = put_store_row(
                store,
                &scratchpad_workflow(invocation),
                "b-tombstoned.md",
                Some(invocation),
                b"tombstoned",
            );
            set_store_created_at(path, &tombstoned.key, tombstoned.version, old_valid);
            store
                .tombstone(&tombstoned.key, tombstoned.version, "seed", "seed")
                .expect("seed inactive tombstone");

            let malformed = put_store_row(
                store,
                "scratchpad:not-a-uuid",
                "z-malformed.md",
                None,
                b"fresh malformed",
            );
            set_store_created_at(path, &malformed.key, malformed.version, fresh_malformed);
        });
        CleanupOrderScenario {
            harness,
            old_valid,
            fresh_malformed,
            expected_address: address(invocation, "a-valid.md"),
        }
    }

    struct ExpiredMalformedScenario {
        harness: AdapterHarness,
        valid_time: DateTime<Utc>,
        malformed_time: DateTime<Utc>,
    }

    fn expired_malformed_scenario() -> ExpiredMalformedScenario {
        let invocation = fixed_uuid(57);
        let valid_time = fixed_time("2025-01-01T00:00:00Z");
        let malformed_time = fixed_time("2025-01-02T00:00:00Z");
        let (harness, ()) = adapter_fixture(|store, path| {
            let valid = put_store_row(
                store,
                &scratchpad_workflow(invocation),
                "a-valid.md",
                None,
                b"valid before malformed",
            );
            set_store_created_at(path, &valid.key, valid.version, valid_time);
            let malformed = put_store_row(
                store,
                "scratchpad:not-a-uuid",
                "z-malformed.md",
                None,
                b"expired malformed",
            );
            set_store_created_at(path, &malformed.key, malformed.version, malformed_time);
        });
        ExpiredMalformedScenario {
            harness,
            valid_time,
            malformed_time,
        }
    }

    #[test]
    fn real_collector_rejects_non_private_and_tombstoned_rows_before_callback_and_fresh_malformed_before_decode()
     {
        let scenario = cleanup_order_scenario();
        let mut offered = Vec::new();

        let rows = scenario
            .harness
            .adapter
            .collect_cleanup_eligible_versions(&mut |created_at| {
                offered.push(created_at);
                created_at == scenario.old_valid
            })
            .expect("fresh malformed row is rejected before decode");

        assert_eq!(
            offered,
            vec![scenario.old_valid, scenario.fresh_malformed],
            "only active private rows reach the callback in Store order"
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].address, scenario.expected_address);
        assert_eq!(rows[0].producer_invocation_uuid, None);
    }

    #[test]
    fn real_collector_decodes_all_callback_accepted_rows_and_returns_first_ordered_error_without_partial_output()
     {
        let scenario = expired_malformed_scenario();
        let mut offered = Vec::new();

        let error = scenario
            .harness
            .adapter
            .collect_cleanup_eligible_versions(&mut |created_at| {
                offered.push(created_at);
                true
            })
            .expect_err("accepted malformed private row fails decode");

        assert_eq!(
            offered,
            vec![scenario.valid_time, scenario.malformed_time],
            "callback runs before ordered decode"
        );
        assert!(matches!(
            error,
            ScratchpadError::MetadataDecode(message)
                if message.starts_with(
                    "workflow_run_id \"scratchpad:not-a-uuid\" has invalid scratchpad UUID:"
                )
        ));
    }
}

fn braced_item<'a>(source: &'a str, marker: &str) -> &'a str {
    let start = source
        .find(marker)
        .unwrap_or_else(|| panic!("source marker not found: {marker}"));
    let open = source[start..]
        .find('{')
        .map(|offset| start + offset)
        .unwrap_or_else(|| panic!("opening brace not found after: {marker}"));
    let mut depth = 0usize;
    for (offset, character) in source[open..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[start..=open + offset];
                }
            }
            _ => {}
        }
    }
    panic!("closing brace not found after: {marker}")
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// Intent: CORE-STRUCT-01.
/// Risk/finding lineage: S2, S9; R1-F04 -> R2-F01 -> R3-F01/CQ-C2-F02;
/// R1-F05; R3-F03; R5-F02/PR-001.
/// Fixture source/application: compile-time application source at negative vocabulary review.
/// Observable: domain capability has eight named operations and no Store/DTO/generic CRUD leakage.
mod core_struct_01 {
    use super::*;

    fn application_source() -> &'static str {
        include_str!("../application.rs")
    }

    #[test]
    fn application_and_capability_exclude_store_dto_and_encoded_workflow_vocabulary() {
        let source = application_source();
        let capability = braced_item(source, "pub(super) trait ScratchpadPersistence");

        for forbidden in [
            "Store",
            "ArtifactKey",
            "ArtifactMeta",
            "ArtifactRecord",
            "ListFilter",
            "PutRequest",
            "PutReceipt",
            "TombstoneReceipt",
            "TombstoneStatus",
            "StoreError",
            "oulipoly_agent_store",
            "scratchpad:",
        ] {
            assert!(
                !source.contains(forbidden),
                "application source leaks forbidden vocabulary {forbidden:?}"
            );
        }
        for generic_operation in [
            "fn put(",
            "fn get(",
            "fn get_meta(",
            "fn list(",
            "fn tombstone(",
        ] {
            assert!(
                !capability.contains(generic_operation),
                "capability mirrors generic operation {generic_operation:?}"
            );
        }
        for domain_operation in [
            "fn append_private_version(",
            "fn load_active_private_record(",
            "fn acquire_newest_active_target(",
            "fn acquire_existing_target(",
            "fn enumerate_private_versions(",
            "fn collect_cleanup_eligible_versions(",
            "fn retire_private_version(",
            "fn append_canonical_publication(",
        ] {
            assert_eq!(
                capability.matches(domain_operation).count(),
                1,
                "capability operation {domain_operation:?}"
            );
        }
    }
}

/// Intent: CORE-STRUCT-02.
/// Risk/finding lineage: S1, S2, S9, S14; R1-F04 -> R2-F01 -> R3-F01;
/// R3-F02; R3-F03; R5-F01/AUDIT-RISK-R5-001/SHORTCUT-RISK-002/PR-002/CQ-R5-F01.
/// Fixture source/application: compile-time root/application source and construction review.
/// Observable: one facade -> application -> capability -> adapter -> Store route; no public bypass.
mod core_struct_02 {
    use super::*;

    fn sources() -> (&'static str, &'static str) {
        (include_str!("../application.rs"), include_str!("../lib.rs"))
    }

    #[test]
    fn production_source_has_one_private_application_adapter_and_construction_route() {
        let (application, root) = sources();
        let facade = braced_item(root, "impl Scratchpad {");
        let facade_state = braced_item(root, "pub struct Scratchpad {");
        let application_state = braced_item(application, "pub(super) struct ScratchpadApplication");

        assert_eq!(
            root.matches("impl ScratchpadPersistence for StoreScratchpadPersistence")
                .count(),
            1,
            "one production capability implementation"
        );
        assert_eq!(
            root.matches("StoreScratchpadPersistence::new(").count(),
            1,
            "one adapter construction"
        );
        assert_eq!(
            root.matches("ScratchpadApplication::new(").count(),
            1,
            "one application construction"
        );
        assert_eq!(
            root.matches("Store::open(").count(),
            1,
            "one composition-root Store open"
        );
        assert!(facade_state.contains("application:"));
        assert!(!facade_state.contains("store:"));
        assert_eq!(
            facade.matches("self.application.").count(),
            6,
            "six facade delegations"
        );
        assert!(!facade.contains("self.store"));
        assert!(!facade.contains("Store::put"));
        assert!(!facade.contains("Store::get"));
        assert!(!facade.contains("Store::list"));
        assert!(!facade.contains("Store::tombstone"));
        assert!(application_state.contains("persistence: P"));
        assert!(application_state.contains("observe_current_utc: N"));
        assert!(!application.contains("pub trait ScratchpadPersistence"));
        assert!(!application.contains("pub struct ScratchpadApplication"));
        assert!(!application.contains("#[cfg(feature"));
        assert!(!root.contains("#[cfg(feature"));
        assert!(!application.contains("StrictFakePersistence"));
        assert!(!root.contains("StrictFakePersistence"));
    }
}

/// Intent: CORE-STRUCT-03.
/// Risk/finding lineage: S10, S11, S12, S13, S14; R3-F02 -> R4-F01/
/// SHORTCUT-RISK-001; R5-F01/AUDIT-RISK-R5-001/SHORTCUT-RISK-002/PR-002/CQ-R5-F01;
/// STATUS Phase 6 historical HIGH watch.
/// Fixture source/application: frozen hashes plus root/application/STATUS source attachment review.
/// Observable: byte-identical STATUS, one root mapper and direct adapter/test attachment, no copy.
mod core_struct_03 {
    use super::*;

    fn sources() -> (&'static str, &'static str, &'static str, &'static str) {
        (
            include_str!("../application.rs"),
            include_str!("../lib.rs"),
            include_str!("../retirement_status.rs"),
            include_str!("../retirement_status/tests.rs"),
        )
    }

    #[test]
    fn status_owner_and_tests_remain_byte_identical() {
        let status = include_bytes!("../retirement_status.rs");
        let status_tests = include_bytes!("../retirement_status/tests.rs");

        assert_eq!(
            sha256_hex(status),
            "9610f2f225198dccd3161e1eb0e87f12eb2d36b3f84f8ce81657b1841fc2db59"
        );
        assert_eq!(
            sha256_hex(status_tests),
            "f219c83b7fdcd3fd01732653f7bdcda2099acee041a8c211956279b977a0a738"
        );
    }

    #[test]
    fn status_translation_has_one_root_definition_and_direct_adapter_and_test_attachments() {
        let (application, root, status, status_tests) = sources();

        assert_eq!(
            root.matches("fn map_store_retirement_status(").count(),
            1,
            "one root-private mapper definition"
        );
        assert_eq!(
            root.matches("crate::map_store_retirement_status(").count(),
            1,
            "one direct production adapter call"
        );
        assert!(
            status_tests.contains("use crate::map_store_retirement_status;"),
            "accepted direct test attachment"
        );
        assert!(!application.contains("map_store_retirement_status"));
        assert!(!status.contains("TombstoneStatus"));
        for duplicate in [
            "PrivateRetirementStatus",
            "DeleteStatusAccumulator",
            "GcStatusAccumulator",
            "record_delete_tombstone",
            "record_gc_tombstone_status",
            "record_gc_tombstone(",
        ] {
            assert!(
                !application.contains(duplicate),
                "application contains duplicate STATUS owner {duplicate:?}"
            );
            assert!(
                !root.contains(duplicate),
                "root contains superseded STATUS owner {duplicate:?}"
            );
        }
        assert!(!root.contains("pub use map_store_retirement_status"));
        assert!(!root.contains("pub(crate) use map_store_retirement_status"));
    }
}
