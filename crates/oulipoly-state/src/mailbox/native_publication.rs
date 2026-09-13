//! Live native evidence, separate from physically nonmutating detached inspection.
//! Declared roles: accessor, validator.
use super::*;

/// Related sidecar facts observed in one live SQLite transaction. This is not
/// COMMIT-return acknowledgment, actor custody, or a transferable writer fence.
/// Mutable predicates must be revalidated at the State authority boundary.
#[derive(Debug)]
pub struct NativePublication {
    pub runtime: Option<RuntimeGenerationRow>,
    pub(crate) producer_quiescent: Option<bool>,
    pub original_drain: Option<serde_json::Value>,
}

impl MailboxDb {
    /// Open an existing sidecar in an already writer-authorized native lane.
    /// This is live SQLite participation, not physical-readonly inspection.
    /// Mutable observations still require the operation's transaction fence.
    pub fn open_existing_native_authority(path: &Path) -> Result<Self, String> {
        let authority = MailboxAuthorityFence::acquire(path).map_err(|e| e.to_string())?;
        crate::rebuild_recovery::ensure_writable_open_allowed(authority.path())?;
        let mut mailbox = Self::open_existing_for_completion_authority(&authority)?;
        mailbox._namespace_authority = Some(authority);
        Ok(mailbox)
    }

    /// Existing writer-authorized native lanes only. No creation, migration,
    /// copied fallback, or promise of physical source nonmutation. Live SQLite
    /// participation may update shm or perform legitimate recovery.
    pub fn read_native_publication(
        path: &Path,
        generation: &str,
        invocation: &str,
    ) -> Result<NativePublication, String> {
        let mailbox = Self::open_existing_native_authority(path)?;
        let tx = mailbox
            .conn
            .unchecked_transaction()
            .map_err(|e| e.to_string())?;
        native_publication_on(&tx, generation, invocation)
    }
}

impl CompletionAuthorityFence<'_> {
    pub(crate) fn native_publication(
        &self,
        generation: &str,
        invocation: &str,
    ) -> Result<NativePublication, String> {
        native_publication_on(&self.tx, generation, invocation)
    }
}

fn native_publication_on(
    conn: &Connection,
    generation: &str,
    invocation: &str,
) -> Result<NativePublication, String> {
    let id = RuntimeGenerationId::parse(generation).map_err(|e| e.to_string())?;
    let runtime = runtime_generation_by_id_on(conn, &id).map_err(|e| e.to_string())?;
    if runtime
        .as_ref()
        .is_some_and(|row| row.spawn_invocation_uuid != invocation)
    {
        return Err("native_publication_invocation_conflict".into());
    }
    let original_drain =
        completion_continuation::native_original_drain_on(conn, generation, invocation)?;
    let producer_quiescent = runtime
        .as_ref()
        .and_then(|row| custody_proof_observation(conn, row));
    Ok(NativePublication {
        runtime,
        original_drain,
        producer_quiescent,
    })
}
