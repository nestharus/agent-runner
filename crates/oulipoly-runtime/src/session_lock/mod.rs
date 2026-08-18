use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const SENTINEL_LOCK_TIMEOUT: Duration = Duration::from_secs(5);
const SENTINEL_LOCK_POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lease {
    pub session_id: String,
    pub provider_name: String,
    pub token: String,
    pub expires_at: String,
    pub lock_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseReceipt {
    pub session_id: String,
    pub token: String,
    pub released_at: String,
    pub already_released: bool,
}

#[derive(Debug, Clone)]
pub enum LockError {
    Busy {
        expires_at: String,
        token_hash: Option<String>,
    },
    TokenInvalid,
    LockExpired,
    SentinelBusy {
        timeout_ms: u64,
    },
    Operational {
        message: String,
    },
}

#[derive(Debug)]
pub struct ProcessAuthority {
    _file: File,
    session_id: String,
}

impl ProcessAuthority {
    pub fn require_session(&self, session_id: &str) -> Result<(), LockError> {
        if self.session_id == session_id {
            Ok(())
        } else {
            Err(LockError::Operational {
                message: format!(
                    "process authority session mismatch: expected {}, got {session_id}",
                    self.session_id
                ),
            })
        }
    }
}

#[derive(Debug)]
pub struct SessionLock {
    sentinel: File,
    lock_dir: PathBuf,
}

#[derive(Debug, Deserialize)]
struct StoredLease {
    version: u32,
    session_id: String,
    #[serde(default)]
    token_hash: Option<String>,
    expires_at: String,
}

#[derive(Debug, Serialize)]
struct StoredLeaseOut<'a> {
    version: u32,
    session_id: &'a str,
    provider_name: &'a str,
    token_hash: String,
    created_at: String,
    expires_at: String,
    owner_pid: u32,
}

#[derive(Debug, Deserialize)]
struct StoredReleaseMarker {
    version: u32,
    session_id: String,
    token_hash: Option<String>,
    released_at: String,
}

#[derive(Debug, Serialize)]
struct StoredReleaseMarkerOut<'a> {
    version: u32,
    session_id: &'a str,
    token_hash: String,
    released_at: String,
}

impl SessionLock {
    pub fn new(lock_dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(lock_dir)?;
        #[cfg(unix)]
        {
            fs::set_permissions(lock_dir, fs::Permissions::from_mode(0o700))?;
        }
        let lock_dir = lock_dir.canonicalize()?;
        let sentinel_path = lock_dir.join("sentinel.lock");
        let mut options = OpenOptions::new();
        options.create(true).read(true).write(true);
        #[cfg(unix)]
        {
            options.mode(0o600);
        }
        let sentinel = options.open(sentinel_path)?;
        Ok(Self { sentinel, lock_dir })
    }

    pub fn acquire(
        &self,
        session_id: &str,
        provider_name: &str,
        ttl: Duration,
    ) -> Result<Lease, LockError> {
        self.acquire_with_process_authority(session_id, provider_name, ttl)
            .map(|(_, lease)| lease)
    }

    pub fn acquire_with_process_authority(
        &self,
        session_id: &str,
        provider_name: &str,
        ttl: Duration,
    ) -> Result<(ProcessAuthority, Lease), LockError> {
        let process_authority = self.acquire_process_authority(session_id)?;
        let lease = self.acquire_metadata(session_id, provider_name, ttl)?;
        Ok((process_authority, lease))
    }

    fn acquire_metadata(
        &self,
        session_id: &str,
        provider_name: &str,
        ttl: Duration,
    ) -> Result<Lease, LockError> {
        self.with_lock(|| {
            let lock_path = self.lock_path(session_id);
            let now = Utc::now();

            match read_json::<StoredLease>(&lock_path) {
                Ok(Some(existing)) => {
                    validate_version(existing.version)?;
                    let expires_at = parse_time(&existing.expires_at)?;
                    if expires_at > now {
                        return Err(LockError::Busy {
                            expires_at: existing.expires_at,
                            token_hash: existing.token_hash,
                        });
                    }
                }
                Ok(None) => {}
                Err(err) => return Err(err),
            }

            let token = generate_token()?;
            let created_at = now.to_rfc3339();
            let expires_at = (now
                + chrono::Duration::from_std(ttl).map_err(|err| LockError::Operational {
                    message: format!("invalid ttl: {err}"),
                })?)
            .to_rfc3339();
            let stored = StoredLeaseOut {
                version: 1,
                session_id,
                provider_name,
                token_hash: token_hash(&token),
                created_at,
                expires_at: expires_at.clone(),
                owner_pid: std::process::id(),
            };
            atomic_write_json(
                &self.lock_dir,
                &lock_path,
                &format!("session-{session_id}.lock.acquire"),
                &stored,
            )?;
            remove_if_exists(&self.release_marker_path(session_id))?;
            fsync_dir(&self.lock_dir)?;

            Ok(Lease {
                session_id: session_id.to_string(),
                provider_name: provider_name.to_string(),
                token,
                expires_at,
                lock_path,
            })
        })
    }

    pub fn acquire_process_authority(
        &self,
        session_id: &str,
    ) -> Result<ProcessAuthority, LockError> {
        let authority_path = self
            .lock_dir
            .join(format!("session-{session_id}.process-authority.lock"));
        let mut options = OpenOptions::new();
        options.create(true).read(true).write(true);
        #[cfg(unix)]
        {
            options.mode(0o600);
        }
        let file = options
            .open(&authority_path)
            .map_err(|error| io_operational("failed to open process authority", error))?;
        match fs4::FileExt::try_lock(&file) {
            Ok(()) => Ok(ProcessAuthority {
                _file: file,
                session_id: session_id.to_string(),
            }),
            Err(fs4::TryLockError::WouldBlock) => Err(LockError::Busy {
                expires_at: "process-lifetime".to_string(),
                token_hash: None,
            }),
            Err(fs4::TryLockError::Error(error)) => {
                Err(io_operational("failed to acquire process authority", error))
            }
        }
    }

    pub fn release(&self, session_id: &str, token: &str) -> Result<ReleaseReceipt, LockError> {
        if !valid_token(token) {
            return Err(LockError::TokenInvalid);
        }

        self.with_lock(|| {
            let lock_path = self.lock_path(session_id);
            let marker_path = self.release_marker_path(session_id);
            let now = Utc::now().to_rfc3339();

            match read_json::<StoredLease>(&lock_path)? {
                Some(existing) => {
                    validate_version(existing.version)?;
                    if existing.session_id != session_id || !stored_token_matches(&existing, token)
                    {
                        return Err(LockError::TokenInvalid);
                    }
                    let marker = StoredReleaseMarkerOut {
                        version: 1,
                        session_id,
                        token_hash: token_hash(token),
                        released_at: now.clone(),
                    };
                    atomic_write_json(
                        &self.lock_dir,
                        &marker_path,
                        &format!("session-{session_id}.released.release"),
                        &marker,
                    )?;
                    remove_if_exists(&lock_path)?;
                    fsync_dir(&self.lock_dir)?;
                    Ok(ReleaseReceipt {
                        session_id: session_id.to_string(),
                        token: token.to_string(),
                        released_at: now,
                        already_released: false,
                    })
                }
                None => match read_json::<StoredReleaseMarker>(&marker_path)? {
                    Some(marker) => {
                        validate_version(marker.version)?;
                        if marker.session_id == session_id && marker_token_matches(&marker, token) {
                            Ok(ReleaseReceipt {
                                session_id: session_id.to_string(),
                                token: token.to_string(),
                                released_at: marker.released_at,
                                already_released: true,
                            })
                        } else {
                            Err(LockError::TokenInvalid)
                        }
                    }
                    None => Err(LockError::LockExpired),
                },
            }
        })
    }

    fn with_lock<T>(&self, f: impl FnOnce() -> Result<T, LockError>) -> Result<T, LockError> {
        self.with_lock_for(SENTINEL_LOCK_TIMEOUT, SENTINEL_LOCK_POLL_INTERVAL, f)
    }

    fn with_lock_for<T>(
        &self,
        timeout: Duration,
        poll_interval: Duration,
        f: impl FnOnce() -> Result<T, LockError>,
    ) -> Result<T, LockError> {
        let started = Instant::now();
        loop {
            match fs4::FileExt::try_lock(&self.sentinel) {
                Ok(()) => break,
                Err(fs4::TryLockError::WouldBlock) if started.elapsed() < timeout => {
                    std::thread::sleep(
                        poll_interval.min(timeout.saturating_sub(started.elapsed())),
                    );
                }
                Err(fs4::TryLockError::WouldBlock) => {
                    return Err(LockError::SentinelBusy {
                        timeout_ms: timeout.as_millis().try_into().unwrap_or(u64::MAX),
                    });
                }
                Err(fs4::TryLockError::Error(error)) => {
                    return Err(LockError::Operational {
                        message: format!("acquire sentinel lock: {error}"),
                    });
                }
            }
        }
        let result = f();
        let unlock_err =
            fs4::FileExt::unlock(&self.sentinel)
                .err()
                .map(|err| LockError::Operational {
                    message: format!("release sentinel lock: {err}"),
                });
        match (result, unlock_err) {
            (Ok(value), None) => Ok(value),
            (Ok(_), Some(err)) => Err(err),
            (Err(err), _) => Err(err),
        }
    }

    fn lock_path(&self, session_id: &str) -> PathBuf {
        self.lock_dir.join(format!("session-{session_id}.lock"))
    }

    fn release_marker_path(&self, session_id: &str) -> PathBuf {
        self.lock_dir.join(format!("session-{session_id}.released"))
    }
}

pub fn any_active_for_session(lock_dir: &Path, session_id: &str) -> Result<bool, LockError> {
    if !lock_dir.exists() {
        return Ok(false);
    }
    let lock_path = lock_dir.join(format!("session-{session_id}.lock"));
    let Some(existing) = read_json::<StoredLease>(&lock_path)? else {
        return Ok(false);
    };
    validate_version(existing.version)?;
    if existing.session_id != session_id {
        return Err(LockError::Operational {
            message: format!(
                "lock metadata session mismatch: expected {session_id}, got {}",
                existing.session_id
            ),
        });
    }
    let expires_at = parse_time(&existing.expires_at)?;
    Ok(expires_at > Utc::now())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Option<T>, LockError> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(io_operational("failed to open lock metadata", err)),
    };
    let mut body = Vec::new();
    file.read_to_end(&mut body)
        .map_err(|err| io_operational("failed to read lock metadata", err))?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|err| LockError::Operational {
            message: format!("failed to parse lock metadata: {err}"),
        })
}

fn atomic_write_json<T: Serialize>(
    lock_dir: &Path,
    final_path: &Path,
    prefix: &str,
    value: &T,
) -> Result<(), LockError> {
    let temp_path = lock_dir.join(format!(
        "{prefix}-{}-{}.tmp",
        std::process::id(),
        random_hex_128()?
    ));
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        options.mode(0o600);
    }
    let mut file = options
        .open(&temp_path)
        .map_err(|err| io_operational("failed to create lock temp file", err))?;
    let body = serde_json::to_vec(value).map_err(|err| LockError::Operational {
        message: format!("failed to encode lock metadata: {err}"),
    })?;
    file.write_all(&body)
        .map_err(|err| io_operational("failed to write lock temp file", err))?;
    file.sync_all()
        .map_err(|err| io_operational("failed to fsync lock temp file", err))?;
    drop(file);
    fs::rename(&temp_path, final_path)
        .map_err(|err| io_operational("failed to rename lock temp file", err))?;
    Ok(())
}

fn remove_if_exists(path: &Path) -> Result<(), LockError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(io_operational("failed to remove lock file", err)),
    }
}

fn fsync_dir(path: &Path) -> Result<(), LockError> {
    File::open(path)
        .and_then(|dir| dir.sync_all())
        .map_err(|err| io_operational("failed to fsync lock directory", err))
}

fn validate_version(version: u32) -> Result<(), LockError> {
    if version == 1 {
        Ok(())
    } else {
        Err(LockError::Operational {
            message: format!("unsupported lock metadata version: {version}"),
        })
    }
}

fn parse_time(value: &str) -> Result<DateTime<Utc>, LockError> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|err| LockError::Operational {
            message: format!("invalid lock expiry timestamp: {err}"),
        })
}

fn stored_token_matches(lease: &StoredLease, token: &str) -> bool {
    lease.token_hash.as_deref() == Some(token_hash(token).as_str())
}

fn marker_token_matches(marker: &StoredReleaseMarker, token: &str) -> bool {
    marker.token_hash.as_deref() == Some(token_hash(token).as_str())
}

fn valid_token(token: &str) -> bool {
    let Some(hex) = token.strip_prefix("pause_") else {
        return false;
    };
    hex.len() == 32
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn generate_token() -> Result<String, LockError> {
    Ok(format!("pause_{}", random_hex_128()?))
}

fn random_hex_128() -> Result<String, LockError> {
    let mut bytes = [0_u8; 16];
    getrandom::getrandom(&mut bytes).map_err(|err| LockError::Operational {
        message: format!("failed to generate random token: {err}"),
    })?;
    let mut out = String::with_capacity(32);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut out, "{byte:02x}").expect("writing to string cannot fail");
    }
    Ok(out)
}

fn token_hash(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

fn io_operational(context: &str, err: io::Error) -> LockError {
    LockError::Operational {
        message: format!("{context}: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn lock_fixture() -> (tempfile::TempDir, SessionLock) {
        let dir = tempfile::tempdir().unwrap();
        let lock = SessionLock::new(dir.path()).unwrap();
        (dir, lock)
    }

    #[test]
    fn sentinel_try_lock_succeeds_without_contention() {
        let (_dir, lock) = lock_fixture();
        lock.with_lock_for(Duration::from_millis(20), Duration::from_millis(1), || {
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn sentinel_try_lock_observes_release_before_deadline() {
        let (dir, lock) = lock_fixture();
        let holder = Arc::new(SessionLock::new(dir.path()).unwrap());
        fs4::FileExt::lock(&holder.sentinel).unwrap();
        let releasing = Arc::clone(&holder);
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(40));
            fs4::FileExt::unlock(&releasing.sentinel).unwrap();
        });

        lock.with_lock_for(Duration::from_millis(500), Duration::from_millis(2), || {
            Ok(())
        })
        .unwrap();
        release.join().unwrap();
    }

    #[test]
    fn sentinel_try_lock_exhaustion_is_bounded_and_typed() {
        let (dir, lock) = lock_fixture();
        let holder = SessionLock::new(dir.path()).unwrap();
        fs4::FileExt::lock(&holder.sentinel).unwrap();
        let started = Instant::now();

        let error = lock
            .with_lock_for(Duration::from_millis(40), Duration::from_millis(2), || {
                Ok(())
            })
            .unwrap_err();

        assert!(matches!(error, LockError::SentinelBusy { timeout_ms: 40 }));
        assert!(started.elapsed() < Duration::from_millis(500));
        fs4::FileExt::unlock(&holder.sentinel).unwrap();
    }
}
