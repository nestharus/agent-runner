//! Producer-partitioned diagnostic event storage.
//!
//! This module stores evidence only. Nothing in this API grants or changes
//! coordination authority. In particular, callers must not make State or
//! PID-mailbox outcomes depend on an event append succeeding.

mod envelope;
pub mod generation;
mod importer;
mod maintenance;
mod reader;
mod schema;
mod union_reader;
mod writer;

pub use envelope::normalize_payload_v1;
pub use envelope::{
    CorrelationId, Digest32, EnvelopeError, EventCorrelations, EventEnvelopeV1, EventFamily,
    EventId, EventKind, GenerationId, LegacyProvenance, LegacySyntheticField,
    MAX_EVENT_PAYLOAD_BYTES, NativeProcessIdentity, NewEventV1, PayloadCodec,
    PayloadNormalizationPolicy, ProcessInstanceId, ProducerIdentity, SessionCorrelationDigest,
    SpanId, SupervisorAuthorityId, TraceId, WriterInstanceId, session_correlation_digest,
};
pub use generation::{
    DATABASE_FILE_NAME, DurableFileIdentity, EVENT_STORE_FORMAT_VERSION, GenerationError,
    GenerationLease, GenerationMaintenanceLease, GenerationReaderLease, GenerationSource,
    HeadCoverageIssue, HeadRecord, HeadSlot, PREPARED_MANIFEST_FILE_NAME, PreparedManifest,
    PublishedGeneration, RetirementReceipt, SEALED_FILE_DIGEST_BUFFER_BYTES,
    SEALED_MANIFEST_FILE_NAME, SealedManifest, SelectedHead, WriterLayout, WriterOwnershipGuard,
    acquire_generation_maintenance_lease_at, acquire_generation_reader_lease_at, digest_hex,
    id_hex, parse_id_hex, publish_retirement_receipt, read_prepared_manifest,
    seal_closed_generation,
};
pub use importer::{
    LegacyCoverageIssue, LegacyImportCheckpoint, LegacyImportCoverage, LegacyImportDisposition,
    LegacyImportError, LegacyImportLimits, LegacyImportReceipt, LegacyImportResult,
    LegacyImportSink, LegacyImportSinkError, import_legacy_source,
};
pub use maintenance::{
    CATALOG_BLOOM_BYTES, CATALOG_FORMAT_VERSION, CatalogCurrentIssue, CatalogCurrentRecord,
    CatalogCurrentSelection, CatalogCurrentSlot, CatalogInventoryResult, CatalogManifestWatermark,
    CatalogPartitionEntry, GenerationEligibilityFacts, GenerationMaintenanceTarget,
    IndexRebuildAction, IndexRebuildPlan, MaintenanceFixture, MaintenanceInspectionError,
    MaintenanceOperation, PartitionLifecycleState, RebuildCursor,
    inspect_closed_generation_for_catalog, plan_local_index_rebuild, select_catalog_current_slots,
};
pub use reader::{
    BoundedRead, CoverageIssue, CoverageIssueKind, DiscoveryCoverage, EventFilter,
    GenerationReadTarget, PartitionWatermark, ReadLimits, ReadRecord, read_discovered_generations,
    read_generation, read_generations, reconcile_exact_generation,
};
pub use schema::{
    AppendDisposition, AppendTicket, EventStoreError, GenerationMetadata, GenerationState,
    ReconciliationOutcome, append_batch, close_generation, initialize_generation_schema,
    mark_generation_writable, read_generation_metadata, verify_generation_schema,
};
pub use union_reader::{
    BoundedUnionRead, ExactLegacyRead, LegacyUnionCoverage, UnionCoverageIssue,
    UnionCoverageIssueKind, UnionReadRecord, UnionRecordOrigin, union_bounded_read,
};
pub use writer::{
    AppendReceipt, DEFAULT_EVENT_WRITER_QUEUE_CAPACITY, DEFAULT_GENERATION_ROTATION_SOFT_BYTES,
    EVENT_WRITER_GROUP_MAX_PAYLOAD_BYTES, EVENT_WRITER_GROUP_MAX_RECORDS,
    EVENT_WRITER_GROUP_MAX_RESIDENCE, EnqueueError, EnqueueErrorKind, EventWriterConfig,
    PendingAppend, ProcessEventWriter, RotationReceipt, WriterError,
};

/// Envelope/schema version implemented by this module.
pub const EVENT_SCHEMA_VERSION: i64 = 1;
