//! Offline custody of v29 payloads. The directory containing the transformed
//! database and its files is the single publication unit.
use super::*;
use rusqlite::types::ValueRef;
use std::ffi::CString;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Component;

type FileIdentity = (u64, u64, u64, i64, i64, i64, i64);
type SourceIdentities = std::collections::BTreeMap<String, FileIdentity>;
const CUSTODY_MANIFEST: &str = "payload-custody.json";

#[cfg(test)]
thread_local! {
    static AFTER_PAYLOAD_OPEN_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = Default::default();
}

#[cfg(test)]
pub(super) fn set_after_payload_open_hook(hook: impl FnOnce() + 'static) {
    AFTER_PAYLOAD_OPEN_HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
}

#[cfg(test)]
fn after_payload_open_hook() {
    AFTER_PAYLOAD_OPEN_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook();
        }
    });
}

#[derive(serde::Serialize, serde::Deserialize)]
struct CustodyManifest {
    source: SourceIdentities,
    staged: SourceIdentities,
}

#[derive(Clone, Debug)]
struct Reference {
    table: &'static str,
    key: String,
    handle: String,
    kind: String,
    json: String,
    old_path: PathBuf,
    sha: String,
    len: u64,
    policy: String,
    compacted: bool,
    submission_token: Option<String>,
    target_kind: Option<String>,
    target_id: Option<String>,
    requires_v2: bool,
}

fn payload_path(root: &Path, sha: &str) -> Result<PathBuf, String> {
    validate_sha256_hex(sha)?;
    if sha.bytes().any(|b| b.is_ascii_uppercase()) {
        return Err("cutover payload digest must be lowercase".into());
    }
    let path = root
        .join(MAILBOX_PAYLOAD_DIRECTORY)
        .join(MAILBOX_PAYLOAD_ADDRESS_VERSION)
        .join(MAILBOX_PAYLOAD_ALGORITHM)
        .join(&sha[..2])
        .join(sha);
    if path.to_str().is_none() {
        return Err("cutover payload address is not UTF-8".into());
    }
    Ok(path)
}

fn references(conn: &Connection) -> Result<Vec<Reference>, String> {
    let mut all = Vec::new();
    let mut mailbox = conn
        .prepare(
            "SELECT seq,kind,handle,payload_json,payload_file_path,
        payload_sha256,payload_byte_len,payload_retention_policy,payload_compacted_at,
        submission_token,target_kind,target_id,completion_provenance FROM mailbox ORDER BY seq",
        )
        .map_err(|e| e.to_string())?;
    let rows = mailbox
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<i64>>(6)?,
                r.get::<_, Option<String>>(7)?,
                r.get::<_, Option<String>>(8)?,
                r.get::<_, Option<String>>(9)?,
                r.get::<_, Option<String>>(10)?,
                r.get::<_, Option<String>>(11)?,
                r.get::<_, String>(12)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    for row in rows {
        let (
            seq,
            kind,
            handle,
            json,
            path,
            sha,
            len,
            policy,
            compacted,
            token,
            target_kind,
            target_id,
            provenance,
        ) = row.map_err(|e| e.to_string())?;
        if let Some((old_path, sha, len, policy)) = complete_reference(path, sha, len, policy)? {
            if kind != AGENT_BASH_COMPLETE_KIND && kind != SUBMITTED_INPUT_KIND {
                return Err("cutover mailbox payload kind is unsupported".into());
            }
            all.push(Reference {
                table: "mailbox",
                key: seq.to_string(),
                handle,
                kind,
                json,
                old_path,
                sha,
                len,
                policy,
                compacted: compacted.is_some(),
                submission_token: token,
                target_kind,
                target_id,
                requires_v2: provenance == "v2",
            });
        } else if compacted.is_some() {
            return Err(format!(
                "cutover mailbox row {seq} has compacted metadata without payload"
            ));
        }
    }
    let mut events = conn
        .prepare(
            "SELECT event_id,kind,payload_json,payload_file_path,
        payload_sha256,payload_byte_len,payload_retention_policy,
        EXISTS(SELECT 1 FROM completion_continuation_source s
            WHERE s.event_id=completion_event.event_id AND s.phase='accepted')
        FROM completion_event ORDER BY event_id",
        )
        .map_err(|e| e.to_string())?;
    let rows = events
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<i64>>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, bool>(7)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    for row in rows {
        let (key, kind, json, path, sha, len, policy, requires_v2) =
            row.map_err(|e| e.to_string())?;
        if let Some((old_path, sha, len, policy)) = complete_reference(path, sha, len, policy)? {
            if kind != AGENT_BASH_COMPLETE_KIND {
                return Err("cutover completion payload kind is unsupported".into());
            }
            all.push(Reference {
                table: "completion_event",
                handle: key.clone(),
                key,
                kind,
                json: json.ok_or("cutover event payload JSON missing")?,
                old_path,
                sha,
                len,
                policy,
                compacted: true,
                submission_token: None,
                target_kind: None,
                target_id: None,
                requires_v2,
            });
        }
    }
    Ok(all)
}

fn complete_reference(
    path: Option<String>,
    sha: Option<String>,
    len: Option<i64>,
    policy: Option<String>,
) -> Result<Option<(PathBuf, String, u64, String)>, String> {
    match (path, sha, len, policy) {
        (None, None, None, None) => Ok(None),
        (Some(path), Some(sha), Some(len), Some(policy)) => {
            let len = u64::try_from(len).map_err(|_| "cutover payload length is negative")?;
            validate_sha256_hex(&sha)?;
            if policy != MAILBOX_PAYLOAD_RETENTION_POLICY {
                return Err("cutover payload retention policy is unsupported".into());
            }
            Ok(Some((PathBuf::from(path), sha, len, policy)))
        }
        _ => Err("cutover payload reference is incomplete".into()),
    }
}

fn projected_json(reference: &Reference, new_path: &Path) -> Result<String, String> {
    if !reference.compacted && reference.kind != SUBMITTED_INPUT_KIND {
        return Ok(reference.json.clone());
    }
    let mut value: serde_json::Value = serde_json::from_str(&reference.json).map_err(|_| {
        format!(
            "cutover {} {} has invalid payload metadata JSON",
            reference.table, reference.key
        )
    })?;
    let object = value
        .as_object_mut()
        .ok_or("cutover payload metadata is not an object")?;
    if object.get("schema_version").and_then(|v| v.as_u64()) != Some(1)
        || object.get("kind").and_then(|v| v.as_str()) != Some(reference.kind.as_str())
    {
        return Err("cutover payload metadata protocol or kind differs from row".into());
    }
    if reference.kind == SUBMITTED_INPUT_KIND {
        let target = object.get("target").ok_or("cutover input target missing")?;
        if object.get("submission_token").and_then(|v| v.as_str())
            != reference.submission_token.as_deref()
            || target.get("kind").and_then(|v| v.as_str()) != reference.target_kind.as_deref()
            || target.get("id").and_then(|v| v.as_str()) != reference.target_id.as_deref()
        {
            return Err("cutover input target differs from authoritative row".into());
        }
    }
    let payload = object
        .get_mut("payload")
        .and_then(|v| v.as_object_mut())
        .ok_or("cutover payload metadata reference missing")?;
    if payload.get("address").and_then(|v| v.as_str())
        != Some(payload_address(&reference.sha).as_str())
        || payload.get("file_path").and_then(|v| v.as_str()) != reference.old_path.to_str()
        || payload.get("sha256").and_then(|v| v.as_str()) != Some(reference.sha.as_str())
        || payload.get("byte_len").and_then(|v| v.as_u64()) != Some(reference.len)
        || payload.get("retention_policy").and_then(|v| v.as_str())
            != Some(reference.policy.as_str())
    {
        return Err("cutover payload metadata conflicts with authoritative row".into());
    }
    if reference.kind == SUBMITTED_INPUT_KIND {
        let target_kind = reference
            .target_kind
            .as_deref()
            .ok_or("cutover input target kind missing")?;
        let target_id = reference
            .target_id
            .as_deref()
            .ok_or("cutover input target id missing")?;
        let token = reference
            .submission_token
            .as_deref()
            .ok_or("cutover input token missing")?;
        let target_kind = match target_kind {
            "session" => InboxTargetKind::Session,
            "chain" => InboxTargetKind::Chain,
            _ => return Err("cutover input target kind invalid".into()),
        };
        if submitted_input_handle(
            token,
            InboxTarget {
                kind: target_kind,
                id: target_id,
            },
        )? != reference.handle
        {
            return Err("cutover input handle differs from authoritative row".into());
        }
    }
    payload.insert(
        "file_path".into(),
        serde_json::Value::String(new_path.to_string_lossy().into_owned()),
    );
    serde_json::to_string(&value).map_err(|e| e.to_string())
}

fn open_no_follow(path: &Path) -> Result<File, String> {
    use std::os::unix::ffi::OsStrExt;
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::CurDir | Component::ParentDir))
    {
        return Err("cutover payload path is not an exact absolute path".into());
    }
    let root = CString::new("/").unwrap();
    let fd = unsafe {
        libc::open(
            root.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let mut dir = unsafe { File::from_raw_fd(fd) };
    let mut parts = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(p) => Some(p),
            _ => None,
        })
        .peekable();
    while let Some(part) = parts.next() {
        let name = CString::new(part.as_bytes()).map_err(|e| e.to_string())?;
        let flags = if parts.peek().is_some() {
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC
        } else {
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK
        };
        let fd = unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(format!(
                "cutover payload path unavailable: {}",
                std::io::Error::last_os_error()
            ));
        }
        dir = unsafe { File::from_raw_fd(fd) };
    }
    Ok(dir)
}

fn identity(
    file: &File,
    owner: u32,
    expected_len: u64,
    readonly: bool,
) -> Result<FileIdentity, String> {
    let m = file.metadata().map_err(|e| e.to_string())?;
    if !m.is_file()
        || m.uid() != owner
        || m.nlink() != 1
        || m.len() != expected_len
        || (readonly && m.mode() & 0o222 != 0)
    {
        return Err("cutover payload is not an exact single-link immutable regular file".into());
    }
    Ok((
        m.dev(),
        m.ino(),
        m.len(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
    ))
}

fn directory(path: &Path, owner: u32) -> Result<(), String> {
    let m = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !m.is_dir() || m.file_type().is_symlink() || m.uid() != owner || m.mode() & 0o077 != 0 {
        return Err("cutover payload directory is not owner-only".into());
    }
    Ok(())
}

fn ensure_stage_parent(stage: &Path, sha: &str, owner: u32) -> Result<PathBuf, String> {
    let mut path = stage.to_path_buf();
    for component in [
        MAILBOX_PAYLOAD_DIRECTORY,
        MAILBOX_PAYLOAD_ADDRESS_VERSION,
        MAILBOX_PAYLOAD_ALGORITHM,
        &sha[..2],
    ] {
        path.push(component);
        match fs::DirBuilder::new().mode(0o700).create(&path) {
            Ok(()) => File::open(path.parent().unwrap())
                .and_then(|f| f.sync_all())
                .map_err(|e| e.to_string())?,
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.to_string()),
        }
        directory(&path, owner)?;
    }
    Ok(path)
}

fn verify_bytes_protocol(bytes: &[u8], refs: &[&Reference]) -> Result<(), String> {
    for r in refs {
        if r.kind != AGENT_BASH_COMPLETE_KIND {
            continue;
        }
        let value: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|_| "cutover completion payload is not JSON")?;
        if let Some(handle) = value.get("handle").and_then(|v| v.as_str()) {
            if handle != r.handle {
                return Err("cutover completion payload handle differs from row".into());
            }
        }
        if value.get("completion_protocol").is_some_and(|protocol| {
            protocol.as_str() != Some(crate::completion_continuation::PROTOCOL)
        }) {
            return Err("cutover completion payload protocol differs from v2".into());
        }
        if value
            .get("protocol")
            .is_some_and(|protocol| protocol.as_str().is_none_or(str::is_empty))
        {
            return Err("cutover completion payload protocol is invalid".into());
        }
        if r.requires_v2
            && value.get("completion_protocol").and_then(|v| v.as_str())
                != Some(crate::completion_continuation::PROTOCOL)
        {
            return Err("cutover accepted v2 payload lacks its protocol".into());
        }
        if value.get("completion_protocol").is_none()
            && value.get("protocol").and_then(|v| v.as_str()).is_none()
        {
            return Err("cutover completion payload lacks protocol discriminator".into());
        }
    }
    Ok(())
}

fn copy_one(
    source: &Path,
    target: &Path,
    refs: &[&Reference],
    source_owner: u32,
    stage_owner: u32,
    expected: FileIdentity,
) -> Result<FileIdentity, String> {
    let first = refs[0];
    let mut input = open_no_follow(source)?;
    let before = identity(&input, source_owner, first.len, true)?;
    if before != expected {
        return Err("cutover source payload identity changed before copy".into());
    }
    #[cfg(test)]
    after_payload_open_hook();
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(target)
        .map_err(|e| format!("cutover payload stage create failed: {e}"))?;
    let mut digest = Sha256::new();
    let mut bytes = Vec::new();
    let need_protocol = refs.iter().any(|r| r.kind == AGENT_BASH_COMPLETE_KIND);
    if need_protocol && first.len > 32 * 1024 * 1024 {
        return Err("cutover completion payload exceeds protocol bound".into());
    }
    let mut buf = [0u8; PAYLOAD_DIGEST_BUFFER_BYTES];
    loop {
        let n = input.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        digest.update(&buf[..n]);
        output.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        if need_protocol {
            bytes.extend_from_slice(&buf[..n]);
        }
        if output.metadata().map_err(|e| e.to_string())?.len() > first.len {
            return Err("cutover payload grew during copy".into());
        }
    }
    if identity(&input, source_owner, first.len, true)? != before
        || identity(&open_no_follow(source)?, source_owner, first.len, true)? != before
    {
        return Err("cutover payload source changed during copy".into());
    }
    if format_sha256_digest(&digest.finalize().into()) != first.sha {
        return Err("cutover payload digest differs from authoritative row".into());
    }
    verify_bytes_protocol(&bytes, refs)?;
    output
        .set_permissions(fs::Permissions::from_mode(0o400))
        .map_err(|e| e.to_string())?;
    output.sync_all().map_err(|e| e.to_string())?;
    let after = identity(&output, stage_owner, first.len, true)?;
    if identity(&open_no_follow(target)?, stage_owner, first.len, true)? != after {
        return Err("cutover payload stage was replaced".into());
    }
    File::open(target.parent().unwrap())
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())?;
    Ok(after)
}

fn verify_one(path: &Path, refs: &[&Reference], owner: u32) -> Result<FileIdentity, String> {
    let first = refs[0];
    let mut file = open_no_follow(path)?;
    let before = identity(&file, owner, first.len, true)?;
    let mut digest = Sha256::new();
    let mut bytes = Vec::new();
    let need_protocol = refs.iter().any(|r| r.kind == AGENT_BASH_COMPLETE_KIND);
    if need_protocol && first.len > 32 * 1024 * 1024 {
        return Err("cutover completion payload exceeds protocol bound".into());
    }
    let mut buf = [0u8; PAYLOAD_DIGEST_BUFFER_BYTES];
    loop {
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        digest.update(&buf[..n]);
        if need_protocol {
            bytes.extend_from_slice(&buf[..n]);
        }
    }
    if identity(&file, owner, first.len, true)? != before
        || identity(&open_no_follow(path)?, owner, first.len, true)? != before
        || format_sha256_digest(&digest.finalize().into()) != first.sha
    {
        return Err("cutover payload changed or failed digest verification".into());
    }
    verify_bytes_protocol(&bytes, refs)?;
    Ok(before)
}

fn groups<'a>(
    refs: &'a [Reference],
    old_root: &Path,
) -> Result<std::collections::BTreeMap<String, Vec<&'a Reference>>, String> {
    let mut groups = std::collections::BTreeMap::<String, Vec<&Reference>>::new();
    for r in refs {
        if r.old_path != payload_path(old_root, &r.sha)? {
            return Err("cutover row points outside the authoritative old payload store".into());
        }
        let group = groups.entry(r.sha.clone()).or_default();
        if group
            .iter()
            .any(|prior| prior.len != r.len || prior.old_path != r.old_path)
        {
            return Err("cutover rows disagree on content-addressed payload".into());
        }
        group.push(r);
    }
    Ok(groups)
}

fn verify_stage_tree(
    stage: &Path,
    grouped: &std::collections::BTreeMap<String, Vec<&Reference>>,
    owner: u32,
) -> Result<(), String> {
    let payload_root = stage.join(MAILBOX_PAYLOAD_DIRECTORY);
    if grouped.is_empty() {
        match fs::symlink_metadata(&payload_root) {
            Ok(_) => return Err("cutover stage has unexpected payload directory".into()),
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
        return Ok(());
    }
    let mut expected = std::collections::HashSet::<PathBuf>::new();
    for sha in grouped.keys() {
        let mut path = payload_root.clone();
        expected.insert(path.clone());
        for part in [
            MAILBOX_PAYLOAD_ADDRESS_VERSION,
            MAILBOX_PAYLOAD_ALGORITHM,
            &sha[..2],
            sha,
        ] {
            path.push(part);
            expected.insert(path.clone());
        }
    }
    let mut pending = vec![payload_root];
    while let Some(path) = pending.pop() {
        if !expected.remove(&path) {
            return Err("cutover stage has unexpected payload artifact".into());
        }
        let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if meta.file_type().is_symlink() || meta.uid() != owner {
            return Err("cutover stage payload artifact is not singly owned".into());
        }
        if meta.is_dir() {
            if meta.mode() & 0o077 != 0 {
                return Err("cutover stage payload directory is exposed".into());
            }
            for item in fs::read_dir(&path).map_err(|e| e.to_string())? {
                pending.push(item.map_err(|e| e.to_string())?.path());
            }
        } else if !meta.is_file() || meta.nlink() != 1 || meta.mode() & 0o777 != 0o400 {
            return Err("cutover stage payload file has invalid type or mode".into());
        }
    }
    if !expected.is_empty() {
        return Err("cutover stage is missing a referenced payload".into());
    }
    Ok(())
}

pub(super) fn capture_source_identities(
    source: &Connection,
    source_path: &Path,
    source_owner: u32,
) -> Result<SourceIdentities, String> {
    let refs = references(source)?;
    let grouped = groups(
        &refs,
        source_path.parent().ok_or("cutover source has no parent")?,
    )?;
    let mut captured = SourceIdentities::new();
    for (sha, group) in &grouped {
        captured.insert(
            sha.clone(),
            verify_one(&group[0].old_path, group, source_owner)?,
        );
    }
    Ok(captured)
}

fn read_manifest(stage: &Path, owner: u32) -> Result<CustodyManifest, String> {
    let path = stage.join(CUSTODY_MANIFEST);
    let file = open_no_follow(&path)?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.uid() != owner || meta.nlink() != 1 || meta.mode() & 0o777 != 0o600 {
        return Err("cutover custody manifest is not root-only".into());
    }
    serde_json::from_reader(std::io::BufReader::new(file))
        .map_err(|e| format!("cutover custody manifest invalid: {e}"))
}

pub(super) fn check_manifest_marker(database: &Path, owner: u32) -> Result<(), String> {
    let root = database.parent().ok_or("broker sidecar has no parent")?;
    let file = open_no_follow(&root.join(CUSTODY_MANIFEST))?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.uid() != owner || meta.nlink() != 1 || meta.mode() & 0o777 != 0o600 {
        return Err("broker sidecar lacks root-only payload custody marker".into());
    }
    Ok(())
}

fn ensure_empty_manifest(root: &Path, owner: u32) -> Result<(), String> {
    match fs::symlink_metadata(root.join(CUSTODY_MANIFEST)) {
        Ok(_) => {
            let manifest = read_manifest(root, owner)?;
            if !manifest.source.is_empty() || !manifest.staged.is_empty() {
                return Err("empty cutover has nonempty payload custody manifest".into());
            }
            return Ok(());
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(e.to_string()),
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(root.join(CUSTODY_MANIFEST))
        .map_err(|e| e.to_string())?;
    serde_json::to_writer(
        &mut file,
        &CustodyManifest {
            source: SourceIdentities::new(),
            staged: SourceIdentities::new(),
        },
    )
    .map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    File::open(root)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())
}

pub(super) fn stage(
    source: &Connection,
    copy: &Path,
    source_path: &Path,
    stage_dir: &Path,
    final_dir: &Path,
    source_owner: u32,
    stage_owner: u32,
    captured: &SourceIdentities,
) -> Result<(), String> {
    let refs = references(source)?;
    let old_root = source_path.parent().ok_or("cutover source has no parent")?;
    let grouped = groups(&refs, old_root)?;
    if grouped.len() != captured.len() {
        return Err("cutover payload references changed after snapshot".into());
    }
    let mut staged = SourceIdentities::new();
    for (sha, group) in &grouped {
        if verify_one(&group[0].old_path, group, source_owner)?
            != *captured
                .get(sha)
                .ok_or("cutover payload identity missing")?
        {
            return Err("cutover source payload identity changed before copy".into());
        }
        let parent = ensure_stage_parent(stage_dir, sha, stage_owner)?;
        staged.insert(
            sha.clone(),
            copy_one(
                &group[0].old_path,
                &parent.join(sha),
                group,
                source_owner,
                stage_owner,
                *captured.get(sha).unwrap(),
            )?,
        );
    }
    let manifest_path = stage_dir.join(CUSTODY_MANIFEST);
    let mut manifest = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&manifest_path)
        .map_err(|e| e.to_string())?;
    serde_json::to_writer(
        &mut manifest,
        &CustodyManifest {
            source: captured.clone(),
            staged,
        },
    )
    .map_err(|e| e.to_string())?;
    manifest.sync_all().map_err(|e| e.to_string())?;
    let mut db = Connection::open(copy).map_err(|e| e.to_string())?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    for r in &refs {
        let new_path = payload_path(final_dir, &r.sha)?;
        let json = projected_json(r, &new_path)?;
        let (sql, key): (&str, &str) = if r.table == "mailbox" {
            (
                "UPDATE mailbox SET payload_file_path=?1,payload_json=?2 WHERE seq=?3 AND payload_file_path=?4",
                &r.key,
            )
        } else {
            (
                "UPDATE completion_event SET payload_file_path=?1,payload_json=?2 WHERE event_id=?3 AND payload_file_path=?4",
                &r.key,
            )
        };
        if tx
            .execute(
                sql,
                params![
                    new_path.to_string_lossy().as_ref(),
                    json,
                    key,
                    r.old_path.to_string_lossy().as_ref()
                ],
            )
            .map_err(|e| e.to_string())?
            != 1
        {
            return Err("cutover payload row changed during transformation".into());
        }
        if r.table == "mailbox" && r.kind == SUBMITTED_INPUT_KIND {
            let old_dir = r
                .old_path
                .parent()
                .ok_or("cutover input has no payload parent")?;
            let new_dir = new_path.parent().unwrap();
            let carriers: (String, String, String, String) = tx
                .query_row(
                    "SELECT state_dir,meta_path,log_path,rc_path FROM mailbox WHERE seq=?1",
                    [&r.key],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .map_err(|e| e.to_string())?;
            let expected = old_dir.to_string_lossy();
            let old_file = r.old_path.to_string_lossy();
            if carriers.0 != expected
                || [&carriers.1, &carriers.2, &carriers.3]
                    .iter()
                    .any(|value| value.as_str() != old_file.as_ref())
            {
                return Err("cutover input compatibility paths differ".into());
            }
            if tx
                .execute(
                    "UPDATE mailbox SET state_dir=?1,meta_path=?2,log_path=?2,rc_path=?2
                WHERE seq=?3",
                    params![
                        new_dir.to_string_lossy().as_ref(),
                        new_path.to_string_lossy().as_ref(),
                        r.key
                    ],
                )
                .map_err(|e| e.to_string())?
                != 1
            {
                return Err("cutover input compatibility paths differ".into());
            }
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .map_err(|e| e.to_string())?;
    drop(db);
    verify(
        source,
        copy,
        source_path,
        stage_dir,
        final_dir,
        source_owner,
        stage_owner,
    )
}

pub(super) fn verify(
    source: &Connection,
    copy: &Path,
    source_path: &Path,
    stage_dir: &Path,
    final_dir: &Path,
    source_owner: u32,
    stage_owner: u32,
) -> Result<(), String> {
    let refs = references(source)?;
    let grouped = groups(
        &refs,
        source_path.parent().ok_or("cutover source has no parent")?,
    )?;
    verify_stage_tree(stage_dir, &grouped, stage_owner)?;
    let captured = read_manifest(stage_dir, stage_owner)?;
    if captured.source.len() != grouped.len() || captured.staged.len() != grouped.len() {
        return Err("cutover custody manifest reference count differs".into());
    }
    let staged = Connection::open_with_flags(copy, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| e.to_string())?;
    for r in &refs {
        let new_path = payload_path(final_dir, &r.sha)?;
        let expected_json = projected_json(r, &new_path)?;
        let sql = if r.table == "mailbox" {
            "SELECT payload_file_path,payload_json FROM mailbox WHERE seq=?1"
        } else {
            "SELECT payload_file_path,payload_json FROM completion_event WHERE event_id=?1"
        };
        let actual: (String, String) = staged
            .query_row(sql, [&r.key], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|e| e.to_string())?;
        if actual != (new_path.to_string_lossy().into_owned(), expected_json) {
            return Err("cutover staged payload row transformation differs".into());
        }
    }
    for (sha, group) in &grouped {
        if verify_one(&group[0].old_path, group, source_owner)?
            != *captured
                .source
                .get(sha)
                .ok_or("cutover custody source identity missing")?
        {
            return Err("cutover source payload identity changed before publication".into());
        }
        if verify_one(&payload_path(stage_dir, sha)?, group, stage_owner)?
            != *captured
                .staged
                .get(sha)
                .ok_or("cutover custody stage identity missing")?
        {
            return Err("cutover stage payload identity changed before publication".into());
        }
    }
    Ok(())
}

pub(super) fn verify_activation(
    conn: &Connection,
    database: &Path,
    owner: u32,
) -> Result<(), String> {
    let refs = references(conn)?;
    if refs.is_empty() {
        return ensure_empty_manifest(
            database
                .parent()
                .ok_or("broker sidecar has no payload root")?,
            owner,
        );
    }
    let root = database
        .parent()
        .ok_or("broker sidecar has no payload root")?;
    let grouped = groups(&refs, root)?;
    verify_stage_tree(root, &grouped, owner)?;
    let manifest = read_manifest(root, owner)?;
    if manifest.source.len() != grouped.len() || manifest.staged.len() != grouped.len() {
        return Err("broker activation lacks exact payload custody manifest".into());
    }
    for (sha, group) in &grouped {
        let path = payload_path(root, sha)?;
        for r in group {
            projected_json(r, &path)?;
        }
        if verify_one(&path, group, owner)?
            != *manifest
                .staged
                .get(sha)
                .ok_or("broker activation lacks payload identity")?
        {
            return Err("broker activation payload identity differs from custody".into());
        }
    }
    Ok(())
}

/// Compare all v29 rows and schema while allowing only the explicit payload
/// path, metadata JSON, and input compatibility-carrier transformations.
pub(super) fn projected_fingerprint(
    conn: &Connection,
    final_dir: Option<&Path>,
) -> Result<[u8; 32], String> {
    let refs = references(conn)?;
    let mut by_key = std::collections::HashMap::new();
    for r in &refs {
        by_key.insert((r.table, r.key.as_str()), r);
    }
    let mut digest = Sha256::new();
    let mut schema = conn
        .prepare("SELECT type,name,tbl_name,COALESCE(sql,'') FROM sqlite_master ORDER BY type,name")
        .map_err(|e| e.to_string())?;
    let rows = schema
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut tables = Vec::new();
    for row in rows {
        let (kind, name, parent, sql) = row.map_err(|e| e.to_string())?;
        for field in [&kind, &name, &parent, &sql] {
            digest.update((field.len() as u64).to_le_bytes());
            digest.update(field.as_bytes());
        }
        if kind == "table" {
            tables.push(name);
        }
    }
    tables.sort();
    for table in tables {
        let quoted = format!("\"{}\"", table.replace('"', "\"\""));
        let stmt = conn
            .prepare(&format!("SELECT * FROM {quoted}"))
            .map_err(|e| e.to_string())?;
        let columns = stmt
            .column_names()
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        let order = (1..=columns.len())
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let mut stmt = conn
            .prepare(&format!("SELECT * FROM {quoted} ORDER BY {order}"))
            .map_err(|e| e.to_string())?;
        digest.update((table.len() as u64).to_le_bytes());
        digest.update(table.as_bytes());
        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            digest.update([0xff]);
            let reference = if let Some(root) = final_dir {
                let key = if table == "mailbox" {
                    row.get::<_, i64>("seq").ok().map(|seq| seq.to_string())
                } else if table == "completion_event" {
                    row.get::<_, String>("event_id").ok()
                } else {
                    None
                };
                key.and_then(|key| by_key.get(&(table.as_str(), key.as_str())).copied())
                    .map(|r| (r, root))
            } else {
                None
            };
            for (i, column) in columns.iter().enumerate() {
                let projected = if let Some((r, root)) = reference {
                    let new_path = payload_path(root, &r.sha)?;
                    match column.as_str() {
                        "payload_file_path" => Some(new_path.to_string_lossy().into_owned()),
                        "payload_json" => Some(projected_json(r, &new_path)?),
                        "state_dir" if r.table == "mailbox" && r.kind == SUBMITTED_INPUT_KIND => {
                            Some(new_path.parent().unwrap().to_string_lossy().into_owned())
                        }
                        "meta_path" | "log_path" | "rc_path"
                            if r.table == "mailbox" && r.kind == SUBMITTED_INPUT_KIND =>
                        {
                            Some(new_path.to_string_lossy().into_owned())
                        }
                        _ => None,
                    }
                } else {
                    None
                };
                let (tag, bytes): (u8, Vec<u8>) = if let Some(value) = projected {
                    (3, value.into_bytes())
                } else {
                    match row.get_ref(i).map_err(|e| e.to_string())? {
                        ValueRef::Null => (0, Vec::new()),
                        ValueRef::Integer(v) => (1, v.to_le_bytes().to_vec()),
                        ValueRef::Real(v) => (2, v.to_bits().to_le_bytes().to_vec()),
                        ValueRef::Text(v) => (3, v.to_vec()),
                        ValueRef::Blob(v) => (4, v.to_vec()),
                    }
                };
                digest.update([tag]);
                digest.update((bytes.len() as u64).to_le_bytes());
                digest.update(bytes);
            }
        }
    }
    Ok(digest.finalize().into())
}
