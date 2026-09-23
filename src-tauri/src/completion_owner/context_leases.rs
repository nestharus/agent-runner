//! The durable key is a process incarnation, not a socket. Local socket counts
//! can release only locally known responsibility. A successor has lost socket
//! knowledge: its inherited rows remain until independent identity expiry.
use super::{MailboxDb, SourceProcessIdentity, read_live_process_identity};
use std::io::Read;
use std::os::unix::net::UnixStream;
use std::path::Path;

struct ContextLease {
    identity: SourceProcessIdentity,
    inherited: bool,
    sockets: Vec<UnixStream>,
}

pub(super) struct ContextLeases(Vec<ContextLease>);

impl ContextLeases {
    pub fn inherit(path: &Path) -> Result<Self, String> {
        Ok(Self(
            MailboxDb::open(path)?
                .completion_contexts()?
                .into_iter()
                .map(|identity| ContextLease {
                    identity,
                    inherited: true,
                    sockets: Vec::new(),
                })
                .collect(),
        ))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn identities(&self) -> Vec<SourceProcessIdentity> {
        self.0.iter().map(|lease| lease.identity.clone()).collect()
    }

    pub fn admit(&mut self, path: &Path, context: &SourceProcessIdentity) -> Result<(), String> {
        let db = MailboxDb::open(path)?;
        // A previously ambiguous retain can have left a row without a local
        // socket. Do not turn that uncertainty into locally releasable debt.
        let known = self.0.iter().any(|lease| lease.identity == *context);
        let inherited = !known && db.completion_contexts()?.contains(context);
        db.retain_completion_context(context)?;
        if !known {
            self.0.push(ContextLease {
                identity: context.clone(),
                inherited,
                sockets: Vec::new(),
            });
        }
        Ok(())
    }

    pub fn retain_local(&mut self, context: SourceProcessIdentity, socket: UnixStream) {
        if let Some(lease) = self.0.iter_mut().find(|lease| lease.identity == context) {
            lease.sockets.push(socket);
        } else {
            self.0.push(ContextLease {
                identity: context,
                inherited: false,
                sockets: vec![socket],
            });
        }
    }

    pub fn release_disconnected(&mut self, path: &Path) -> Result<(), String> {
        let mut result = Ok(());
        // Guardian serialization owns both admission and release. A failed
        // DELETE retains this group, including across a same-incarnation join.
        self.0.retain_mut(|lease| {
            lease.sockets.retain_mut(socket_live);
            if !lease.sockets.is_empty() {
                return true;
            }
            if lease.inherited && !incarnation_expired(&lease.identity) {
                return true;
            }
            match MailboxDb::open(path)
                .and_then(|db| db.release_completion_context(&lease.identity))
            {
                Ok(()) => false,
                Err(error) => {
                    result = Err(error);
                    true
                }
            }
        });
        result
    }
}

fn socket_live(socket: &mut UnixStream) -> bool {
    // Never let a failed nonblocking setup serialize the guardian on a lease.
    // Treat inability to inspect as responsibility, not permission to delete.
    if socket.set_nonblocking(true).is_err() {
        return true;
    }
    matches!(socket.read(&mut [0]), Err(error) if matches!(error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted))
}

/// Admission leases alone: an unreadable identity is not evidence of expiry.
/// The shared optional reader conflates absence, denial and malformed data, so
/// only a readable validated incarnation mismatch or kernel ESRCH discharges
/// inherited responsibility. This assumes the native owner's PID/boot domain,
/// not a database imported from another host or PID namespace.
pub(super) fn incarnation_expired(recorded: &SourceProcessIdentity) -> bool {
    let Ok(pid) = libc::pid_t::try_from(recorded.pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    if let Ok(Some(current)) = read_live_process_identity(recorded.pid) {
        return validated_mismatch(recorded, &current);
    }
    // Signal zero never delivers a signal. EPERM and every non-ESRCH error
    // retain responsibility, including a live PID with an unavailable proc view.
    let result = unsafe { libc::kill(pid, 0) };
    result == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

fn validated_mismatch(
    recorded: &SourceProcessIdentity,
    current: &oulipoly_state::pid_identity::ProcessIdentity,
) -> bool {
    let (Ok(old_boot), Ok(new_boot)) = (
        uuid::Uuid::parse_str(&recorded.boot_id),
        uuid::Uuid::parse_str(&current.os_boot_id),
    ) else {
        return false;
    };
    old_boot.get_version_num() == 4
        && new_boot.get_version_num() == 4
        && recorded.starttime_ticks > 0
        && current.os_pid_starttime_ticks > 0
        && (old_boot != new_boot || recorded.starttime_ticks != current.os_pid_starttime_ticks)
}
