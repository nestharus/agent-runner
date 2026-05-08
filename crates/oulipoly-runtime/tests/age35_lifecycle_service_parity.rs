use oulipoly_runtime::services::{
    InvocationLifecycleFinalizeOutput, InvocationLifecycleFinalizeRequest,
    InvocationLifecycleServicePort, InvocationLifecycleStartRequest,
    ProductionInvocationLifecycleService, error::ServiceError,
};
use oulipoly_state::{InvocationStart, StateDb};
use rusqlite::{OptionalExtension, params};
use std::path::Path;

#[derive(Debug, PartialEq, Eq)]
struct InvocationSnapshot {
    invocation_uuid: String,
    model_name: String,
    provider_name: Option<String>,
    provider_index: i64,
    parent_invocation_id: Option<i64>,
    status: String,
    success: Option<i64>,
    exit_code: Option<i64>,
    error_category: Option<String>,
    terminal_reason: Option<String>,
    created_at_present: bool,
    finished_at_present: bool,
}

#[derive(Debug, PartialEq, Eq)]
struct ProviderAggregateSnapshot {
    invocation_count: i64,
    error_count: i64,
    last_error: Option<String>,
    last_error_at_present: bool,
    last_invoked_at_present: bool,
}

fn memory_db() -> StateDb {
    StateDb::open(Path::new(":memory:")).unwrap()
}

fn start_fixture(uuid: &str, provider_index: usize, parent_id: Option<i64>) -> InvocationStart {
    InvocationStart {
        invocation_uuid: uuid.to_string(),
        model_name: "age35-model".to_string(),
        provider_name: "age35-provider".to_string(),
        provider_index,
        parent_invocation_id: parent_id,
    }
}

fn invocation_snapshot(db: &StateDb, uuid: &str) -> InvocationSnapshot {
    db.connection()
        .query_row(
            "SELECT invocation_uuid, model_name, provider_name, provider_index,
                    parent_invocation_id, status, success, exit_code, error_category,
                    terminal_reason, created_at, finished_at
             FROM invocations
             WHERE invocation_uuid = ?1",
            params![uuid],
            |row| {
                let created_at: String = row.get(10)?;
                let finished_at: Option<String> = row.get(11)?;
                Ok(InvocationSnapshot {
                    invocation_uuid: row.get(0)?,
                    model_name: row.get(1)?,
                    provider_name: row.get(2)?,
                    provider_index: row.get(3)?,
                    parent_invocation_id: row.get(4)?,
                    status: row.get(5)?,
                    success: row.get(6)?,
                    exit_code: row.get(7)?,
                    error_category: row.get(8)?,
                    terminal_reason: row.get(9)?,
                    created_at_present: !created_at.is_empty(),
                    finished_at_present: finished_at.is_some(),
                })
            },
        )
        .unwrap()
}

fn provider_aggregate_snapshot(
    db: &StateDb,
    model_name: &str,
    provider_name: &str,
) -> ProviderAggregateSnapshot {
    db.connection()
        .query_row(
            "SELECT invocation_count, error_count, last_error, last_error_at, last_invoked_at
             FROM providers
             WHERE model_name = ?1 AND provider_name = ?2",
            params![model_name, provider_name],
            |row| {
                let last_error_at: Option<String> = row.get(3)?;
                let last_invoked_at: Option<String> = row.get(4)?;
                Ok(ProviderAggregateSnapshot {
                    invocation_count: row.get(0)?,
                    error_count: row.get(1)?,
                    last_error: row.get(2)?,
                    last_error_at_present: last_error_at.is_some(),
                    last_invoked_at_present: last_invoked_at.is_some(),
                })
            },
        )
        .optional()
        .unwrap()
        .unwrap_or(ProviderAggregateSnapshot {
            invocation_count: 0,
            error_count: 0,
            last_error: None,
            last_error_at_present: false,
            last_invoked_at_present: false,
        })
}

fn assert_dependency_error(result: Result<InvocationLifecycleFinalizeOutput, ServiceError>) {
    match result {
        Err(ServiceError::Dependency { message }) => {
            assert!(
                !message.is_empty(),
                "dependency error should preserve a message"
            )
        }
        other => panic!("expected dependency error, got {other:?}"),
    }
}

#[test]
fn age_35_production_lifecycle_service_start_matches_direct_state_db_start() {
    let direct_db = memory_db();
    let service_db = memory_db();
    let service = ProductionInvocationLifecycleService;

    let direct_parent_id = direct_db
        .start_invocation(&start_fixture(
            "11111111-1111-4111-8111-111111111111",
            0,
            None,
        ))
        .unwrap();
    let service_parent_id = service_db
        .start_invocation(&start_fixture(
            "22222222-2222-4222-8222-222222222222",
            0,
            None,
        ))
        .unwrap();
    assert_eq!(direct_parent_id, service_parent_id);

    let direct_start = start_fixture(
        "33333333-3333-4333-8333-333333333333",
        1,
        Some(direct_parent_id),
    );
    let service_start = start_fixture(
        "33333333-3333-4333-8333-333333333333",
        1,
        Some(service_parent_id),
    );

    let direct_row_id = direct_db.start_invocation(&direct_start).unwrap();
    let service_row_id = service
        .start_invocation(InvocationLifecycleStartRequest {
            state: &service_db,
            start: &service_start,
        })
        .unwrap()
        .invocation_row_id;

    assert_eq!(
        service_row_id, direct_row_id,
        "service start must preserve row-id semantics"
    );
    assert_eq!(
        invocation_snapshot(&service_db, &service_start.invocation_uuid),
        invocation_snapshot(&direct_db, &direct_start.invocation_uuid),
        "service start must insert the same running row fields as StateDb::start_invocation"
    );
}

#[test]
fn age_35_production_lifecycle_service_finalize_matches_direct_state_db_finalize() {
    let direct_db = memory_db();
    let service_db = memory_db();
    let service = ProductionInvocationLifecycleService;
    let uuid = "44444444-4444-4444-8444-444444444444";
    let start = start_fixture(uuid, 0, None);

    let direct_row_id = direct_db.start_invocation(&start).unwrap();
    let service_row_id = service_db.start_invocation(&start).unwrap();

    direct_db
        .finalize_invocation(
            direct_row_id,
            false,
            7,
            Some("quota_exhausted"),
            Some("exit_nonzero"),
        )
        .unwrap();
    service
        .finalize_invocation(InvocationLifecycleFinalizeRequest {
            state: &service_db,
            invocation_row_id: service_row_id,
            success: false,
            exit_code: 7,
            error_category: Some("quota_exhausted"),
            terminal_reason: Some("exit_nonzero"),
        })
        .unwrap();

    assert_eq!(
        invocation_snapshot(&service_db, uuid),
        invocation_snapshot(&direct_db, uuid),
        "service finalize must write the same terminal row fields as StateDb::finalize_invocation"
    );
    assert_eq!(
        provider_aggregate_snapshot(&service_db, "age35-model", "age35-provider"),
        provider_aggregate_snapshot(&direct_db, "age35-model", "age35-provider"),
        "service finalize must update provider aggregates identically"
    );

    assert!(
        direct_db
            .finalize_invocation(99_999, true, 0, None, None)
            .is_err()
    );
    assert_dependency_error(
        service.finalize_invocation(InvocationLifecycleFinalizeRequest {
            state: &service_db,
            invocation_row_id: 99_999,
            success: true,
            exit_code: 0,
            error_category: None,
            terminal_reason: None,
        }),
    );

    assert!(
        direct_db
            .finalize_invocation(direct_row_id, true, 0, None, None)
            .is_err(),
        "direct finalize must reject already-finalized rows"
    );
    assert_dependency_error(
        service.finalize_invocation(InvocationLifecycleFinalizeRequest {
            state: &service_db,
            invocation_row_id: service_row_id,
            success: true,
            exit_code: 0,
            error_category: None,
            terminal_reason: None,
        }),
    );
}
