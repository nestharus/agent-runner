mod common;

use std::error::Error;
use std::fmt::Debug;
use std::io;
use std::path::Path;
use std::process::ExitCode;

use chrono::{DateTime, Utc};
use oulipoly_agent_scratchpad::{
    self as scratchpad, CanonicalAddress, DeleteReceipt, DeleteRequest, DeleteSelector, GcReport,
    GcRequest, GcSelector, InvocationScope, ListRequest, PublishReceipt, PublishRequest,
    ReadRequest, Scratchpad, ScratchpadAddress, ScratchpadError, ScratchpadMeta, ScratchpadName,
    ScratchpadRecord, WriteReceipt, WriteRequest,
};
use oulipoly_agent_store::{StoreError, TombstoneMeta};
use serde_json::Value;
use uuid::Uuid;

fn fixed_time(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .expect("valid fixed timestamp")
        .with_timezone(&Utc)
}

fn assert_value_contract<T: Clone + Debug + Eq + PartialEq>(value: &T) {
    assert_eq!(&value.clone(), value);
    assert!(!format!("{value:?}").is_empty());
}

fn open_from_path(path: &Path) -> Result<Scratchpad, ScratchpadError> {
    Scratchpad::open(path)
}

#[derive(Clone, Copy)]
enum ErrorKind {
    InvalidInput(&'static str),
    MissingInvocationScope,
    InvalidInvocationScope(&'static str),
    NotFound,
    NotFoundNamed(&'static str),
    Collision,
    Io,
    Database,
    MigrationRequired,
    IncompatibleSchema,
    Serialization,
    MetadataDecode(&'static str),
}

struct ErrorContract {
    error: ScratchpadError,
    kind: ErrorKind,
    display: &'static str,
    source: Option<&'static str>,
}

fn serialization_error() -> serde_json::Error {
    serde_json::from_str::<Value>("{").expect_err("invalid JSON fixture")
}

fn direct_error_contracts() -> Vec<ErrorContract> {
    vec![
        ErrorContract {
            error: ScratchpadError::InvalidInput("bad field".to_string()),
            kind: ErrorKind::InvalidInput("bad field"),
            display: "invalid input: bad field",
            source: None,
        },
        ErrorContract {
            error: ScratchpadError::MissingInvocationScope,
            kind: ErrorKind::MissingInvocationScope,
            display: "missing invocation scope: pass --invocation-uuid or set OULIPOLY_PARENT_INVOCATION",
            source: None,
        },
        ErrorContract {
            error: ScratchpadError::InvalidInvocationScope("bad parent".to_string()),
            kind: ErrorKind::InvalidInvocationScope("bad parent"),
            display: "invalid invocation scope: bad parent",
            source: None,
        },
        ErrorContract {
            error: ScratchpadError::NotFound,
            kind: ErrorKind::NotFound,
            display: "scratchpad artifact not found",
            source: None,
        },
        ErrorContract {
            error: ScratchpadError::NotFoundNamed("notes.md".to_string()),
            kind: ErrorKind::NotFoundNamed("notes.md"),
            display: "scratchpad artifact not found: notes.md",
            source: None,
        },
        ErrorContract {
            error: ScratchpadError::Collision,
            kind: ErrorKind::Collision,
            display: "backing store collision",
            source: None,
        },
        ErrorContract {
            error: ScratchpadError::Io(io::Error::other("disk offline")),
            kind: ErrorKind::Io,
            display: "io error: disk offline",
            source: Some("disk offline"),
        },
        ErrorContract {
            error: ScratchpadError::Database(rusqlite::Error::InvalidParameterName(
                ":missing".to_string(),
            )),
            kind: ErrorKind::Database,
            display: "database error: Invalid parameter name: :missing",
            source: Some("Invalid parameter name: :missing"),
        },
        ErrorContract {
            error: ScratchpadError::MigrationRequired,
            kind: ErrorKind::MigrationRequired,
            display: "database schema migration required",
            source: None,
        },
        ErrorContract {
            error: ScratchpadError::IncompatibleSchema,
            kind: ErrorKind::IncompatibleSchema,
            display: "incompatible database schema",
            source: None,
        },
        ErrorContract {
            error: ScratchpadError::Serialization(serialization_error()),
            kind: ErrorKind::Serialization,
            display: "json serialization error: EOF while parsing an object at line 1 column 1",
            source: Some("EOF while parsing an object at line 1 column 1"),
        },
        ErrorContract {
            error: ScratchpadError::MetadataDecode("bad metadata".to_string()),
            kind: ErrorKind::MetadataDecode("bad metadata"),
            display: "metadata decode error: bad metadata",
            source: None,
        },
    ]
}

fn assert_error_kind(error: &ScratchpadError, expected: ErrorKind) {
    match (error, expected) {
        (ScratchpadError::InvalidInput(actual), ErrorKind::InvalidInput(expected)) => {
            assert_eq!(actual, expected)
        }
        (ScratchpadError::MissingInvocationScope, ErrorKind::MissingInvocationScope)
        | (ScratchpadError::NotFound, ErrorKind::NotFound)
        | (ScratchpadError::Collision, ErrorKind::Collision)
        | (ScratchpadError::Io(_), ErrorKind::Io)
        | (ScratchpadError::Database(_), ErrorKind::Database)
        | (ScratchpadError::MigrationRequired, ErrorKind::MigrationRequired)
        | (ScratchpadError::IncompatibleSchema, ErrorKind::IncompatibleSchema)
        | (ScratchpadError::Serialization(_), ErrorKind::Serialization) => {}
        (
            ScratchpadError::InvalidInvocationScope(actual),
            ErrorKind::InvalidInvocationScope(expected),
        ) => assert_eq!(actual, expected),
        (ScratchpadError::NotFoundNamed(actual), ErrorKind::NotFoundNamed(expected)) => {
            assert_eq!(actual, expected)
        }
        (ScratchpadError::MetadataDecode(actual), ErrorKind::MetadataDecode(expected)) => {
            assert_eq!(actual, expected)
        }
        (actual, _) => panic!("unexpected error variant: {actual:?}"),
    }
}

// C-GAP-06: every public root value remains directly constructible at its
// existing path with the promised Clone/Debug/Eq/PartialEq contracts.
#[test]
fn public_root_exports_retain_fields_types_and_value_traits() {
    let invocation_uuid = Uuid::from_u128(21);
    let created_at = fixed_time("2026-01-02T03:04:05Z");
    let tombstoned_at = fixed_time("2026-01-03T04:05:06Z");
    let name = ScratchpadName::new("notes.md").expect("valid name");
    let scope = InvocationScope { invocation_uuid };
    let address = ScratchpadAddress {
        invocation_uuid,
        name: name.clone(),
    };
    let canonical = CanonicalAddress {
        workflow_run_id: "canonical-run".to_string(),
        artifact_name: "artifact.md".to_string(),
    };
    let write_request = WriteRequest {
        scope: scope.clone(),
        name: name.clone(),
        content: b"content".to_vec(),
        format_hint: None,
        verdict_line: None,
        predecessor_version: None,
    };
    let read_request = ReadRequest {
        scope: scope.clone(),
        name: name.clone(),
        version: None,
    };
    let list_request = ListRequest {
        scope: scope.clone(),
        name: None,
        include_tombstoned: true,
    };
    let delete_request = DeleteRequest {
        scope: scope.clone(),
        name: name.clone(),
        selector: DeleteSelector::AllVersions,
        actor: None,
        reason: None,
    };
    let publish_request = PublishRequest {
        source: address.clone(),
        source_version: None,
        destination: canonical.clone(),
        format_hint: None,
        verdict_line: None,
        predecessor_version: None,
    };
    let gc_request = GcRequest {
        selector: GcSelector::ExpiredBefore(created_at),
        dry_run: true,
        actor: None,
        reason: None,
    };
    let meta = ScratchpadMeta {
        address: address.clone(),
        invocation_uuid,
        name: name.clone(),
        version: 2,
        sha256: "abc123".to_string(),
        content_len: 7,
        producer_invocation_uuid: None,
        format_hint: None,
        verdict_line: None,
        predecessor_version: None,
        created_at,
        tombstone: Some(TombstoneMeta {
            tombstoned_at,
            actor: "actor".to_string(),
            reason: "reason".to_string(),
        }),
    };
    let record = ScratchpadRecord {
        meta: meta.clone(),
        content: b"content".to_vec(),
    };
    let write_receipt = WriteReceipt {
        address: address.clone(),
        version: 2,
        producer_invocation_uuid: None,
        sha256: "abc123".to_string(),
        content_len: 7,
        format_hint: None,
        verdict_line: None,
        predecessor_version: None,
        created_at,
    };
    let delete_receipt = DeleteReceipt {
        address: address.clone(),
        selector: DeleteSelector::Version(2),
        tombstoned_versions: vec![2],
        already_tombstoned_versions: Vec::new(),
        actor: "actor".to_string(),
        reason: "reason".to_string(),
        tombstoned_at: Some(tombstoned_at),
    };
    let publish_receipt = PublishReceipt {
        source: address.clone(),
        source_version: 2,
        source_sha256: "abc123".to_string(),
        destination: canonical.clone(),
        destination_version: 3,
        destination_sha256: "def456".to_string(),
        content_len: 7,
        producer_invocation_uuid: invocation_uuid,
        format_hint: None,
        verdict_line: None,
        predecessor_version: None,
        created_at,
    };
    let gc_report = GcReport {
        selector: GcSelector::Invocation(invocation_uuid),
        dry_run: false,
        tombstoned_rows: vec![address.clone()],
        already_tombstoned_rows: Vec::new(),
        actor: "actor".to_string(),
        reason: "reason".to_string(),
        evaluated_at: created_at,
    };

    assert_eq!(name.as_str(), "notes.md");
    assert_value_contract(&name);
    assert_value_contract(&scope);
    assert_value_contract(&address);
    assert_value_contract(&canonical);
    assert_value_contract(&write_request);
    assert_value_contract(&read_request);
    assert_value_contract(&list_request);
    assert_value_contract(&delete_request);
    assert_value_contract(&DeleteSelector::Latest);
    assert_value_contract(&DeleteSelector::Version(2));
    assert_value_contract(&DeleteSelector::AllVersions);
    assert_value_contract(&publish_request);
    assert_value_contract(&gc_request);
    assert_value_contract(&GcSelector::Invocation(invocation_uuid));
    assert_value_contract(&GcSelector::ExpiredBefore(created_at));
    assert_value_contract(&meta);
    assert_value_contract(&record);
    assert_value_contract(&write_receipt);
    assert_value_contract(&delete_receipt);
    assert_value_contract(&publish_receipt);
    assert_value_contract(&gc_report);

    let _: fn(&Path) -> Result<Scratchpad, ScratchpadError> = open_from_path;
    let _: fn() -> ExitCode = scratchpad::cli::run;
}

// C-GAP-07: all directly constructible error variants preserve exact payload,
// Display output, and source chaining behavior.
#[test]
fn scratchpad_error_display_payload_and_source_contracts_are_exact() {
    for contract in direct_error_contracts() {
        assert_error_kind(&contract.error, contract.kind);
        assert_eq!(contract.error.to_string(), contract.display);
        assert_eq!(
            contract.error.source().map(ToString::to_string),
            contract.source.map(str::to_string)
        );
    }
}

// C-GAP-07: every StoreError conversion retains its current scratchpad error
// class and payload, including incompatible-schema detail erasure.
#[test]
fn store_error_conversions_are_complete() {
    let cases = [
        ErrorContract {
            error: StoreError::InvalidInput("store input".to_string()).into(),
            kind: ErrorKind::InvalidInput("store input"),
            display: "invalid input: store input",
            source: None,
        },
        ErrorContract {
            error: StoreError::NotFound.into(),
            kind: ErrorKind::NotFound,
            display: "scratchpad artifact not found",
            source: None,
        },
        ErrorContract {
            error: StoreError::Collision.into(),
            kind: ErrorKind::Collision,
            display: "backing store collision",
            source: None,
        },
        ErrorContract {
            error: StoreError::Io(io::Error::other("store io")).into(),
            kind: ErrorKind::Io,
            display: "io error: store io",
            source: Some("store io"),
        },
        ErrorContract {
            error: StoreError::Database(rusqlite::Error::InvalidParameterName(
                ":store".to_string(),
            ))
            .into(),
            kind: ErrorKind::Database,
            display: "database error: Invalid parameter name: :store",
            source: Some("Invalid parameter name: :store"),
        },
        ErrorContract {
            error: StoreError::MigrationRequired.into(),
            kind: ErrorKind::MigrationRequired,
            display: "database schema migration required",
            source: None,
        },
        ErrorContract {
            error: StoreError::IncompatibleSchema("future-schema".to_string()).into(),
            kind: ErrorKind::IncompatibleSchema,
            display: "incompatible database schema",
            source: None,
        },
    ];

    for contract in cases {
        assert_error_kind(&contract.error, contract.kind);
        assert_eq!(contract.error.to_string(), contract.display);
        assert_eq!(
            contract.error.source().map(ToString::to_string),
            contract.source.map(str::to_string)
        );
    }
}

#[test]
fn root_use_case_method_signatures_are_exact() {
    let _: fn(&Scratchpad, WriteRequest) -> Result<WriteReceipt, ScratchpadError> =
        Scratchpad::write;
    let _: fn(&Scratchpad, ReadRequest) -> Result<ScratchpadRecord, ScratchpadError> =
        Scratchpad::read;
    let _: fn(&Scratchpad, ListRequest) -> Result<Vec<ScratchpadMeta>, ScratchpadError> =
        Scratchpad::list;
    let _: fn(&Scratchpad, DeleteRequest) -> Result<DeleteReceipt, ScratchpadError> =
        Scratchpad::delete;
    let _: fn(&Scratchpad, PublishRequest) -> Result<PublishReceipt, ScratchpadError> =
        Scratchpad::publish;
    let _: fn(&Scratchpad, GcRequest) -> Result<GcReport, ScratchpadError> = Scratchpad::gc;
}

#[test]
fn io_and_serde_error_from_conversions_are_exact() {
    let io_error: ScratchpadError = io::Error::other("direct io").into();
    assert_error_kind(&io_error, ErrorKind::Io);
    assert_eq!(io_error.to_string(), "io error: direct io");
    assert_eq!(
        io_error.source().map(ToString::to_string).as_deref(),
        Some("direct io")
    );

    let serde_error: ScratchpadError = serialization_error().into();
    assert_error_kind(&serde_error, ErrorKind::Serialization);
    assert_eq!(
        serde_error.to_string(),
        "json serialization error: EOF while parsing an object at line 1 column 1"
    );
    assert_eq!(
        serde_error.source().map(ToString::to_string).as_deref(),
        Some("EOF while parsing an object at line 1 column 1")
    );
}
