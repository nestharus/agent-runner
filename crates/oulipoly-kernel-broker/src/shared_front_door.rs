//! AGE-319 managed entry. The v1 installed pair is deliberately outside this tree.
//! No State or handle path is opened until a complete selector has been read.
#[cfg(feature = "age319-private-broker-fixture")]
#[used]
static FIXTURE_ONLY_MARKER: [u8; 46] = *b"OULIPOLY_AGE319_FRONT_DOOR_FIXTURE_ONLY_UNSAFE";
#[cfg(target_os = "linux")]
mod linux {
    use serde::Deserialize;
    use sha2::{Digest, Sha256};
    use std::ffi::{CStr, CString, OsStr};
    use std::fs::{self, File, OpenOptions};
    use std::io::{self, Read, Seek, SeekFrom, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    use std::os::unix::net::UnixStream;
    use std::path::{Path, PathBuf};

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Selector {
        schema: u8,
        epoch: u64,
        phase: String,
        generation: String,
        manifest_sha256: String,
        publication_id: String,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Aliases {
        runner: PathBuf,
        runner_alt: PathBuf,
        gui: Option<PathBuf>,
        bash: PathBuf,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct AliasHashes {
        runner: String,
        runner_alt: String,
        gui: Option<String>,
        bash: String,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct AliasOwners {
        runner: u32,
        runner_alt: u32,
        gui: Option<u32>,
        bash: u32,
    }

    #[allow(dead_code)]
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct AliasLinks {
        runner: PathBuf,
        runner_alt: PathBuf,
        gui: Option<PathBuf>,
        bash: PathBuf,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct AdapterBindings {
        runner: PathBuf,
        bash: PathBuf,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct BrokerRoute {
        lane_id: String,
        source_generation: String,
        domain_id: String,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Generation {
        schema: u8,
        generation: String,
        stage_path: PathBuf,
        stage_manifest_sha256: String,
        launcher_sha256: String,
        aliases: Aliases,
        alias_owner_uids: AliasOwners,
        #[serde(rename = "legacy_alias_links")]
        _legacy_alias_links: AliasLinks,
        adapter_bindings: Option<AdapterBindings>,
        legacy_alias_targets: Aliases,
        legacy_alias_sha256: AliasHashes,
        legacy_alias_owner_uids: AliasOwners,
        legacy_runner_entry: PathBuf,
        legacy_runner_sha256: String,
        legacy_runner_owner_uid: u32,
        legacy_runner_config: PathBuf,
        legacy_runner_config_sha256: String,
        legacy_runner_config_owner_uid: u32,
        legacy_bash_entry: PathBuf,
        legacy_bash_sha256: String,
        legacy_bash_owner_uid: u32,
        legacy_bash_config: PathBuf,
        legacy_bash_config_sha256: String,
        legacy_bash_config_owner_uid: u32,
        legacy_bash_state_root: PathBuf,
        fresh_runner_image: PathBuf,
        fresh_runner_sha256: String,
        fresh_runner_config: PathBuf,
        fresh_runner_config_sha256: String,
        fresh_bash_image: PathBuf,
        fresh_bash_sha256: String,
        fresh_bash_config: PathBuf,
        fresh_bash_config_sha256: String,
        broker_image: PathBuf,
        broker_image_sha256: String,
        broker_socket: PathBuf,
        broker_route: BrokerRoute,
    }

    #[derive(Clone, Copy)]
    enum Role {
        Runner,
        RunnerAlt,
        Gui,
        Bash,
    }

    fn error(message: impl Into<String>) -> io::Error {
        io::Error::other(message.into())
    }

    fn trusted(path: &Path, regular: bool, local_owner: Option<u32>) -> io::Result<()> {
        if !path.is_absolute()
            || path
                .components()
                .any(|p| matches!(p, std::path::Component::ParentDir))
        {
            return Err(error("noncanonical front-door path"));
        }
        let mut part = path;
        loop {
            let meta = fs::symlink_metadata(part)?;
            if meta.file_type().is_symlink()
                || (!cfg!(feature = "age319-private-broker-fixture") && meta.mode() & 0o022 != 0)
                || (!cfg!(feature = "age319-private-broker-fixture")
                    && meta.uid() != 0
                    && Some(meta.uid()) != local_owner)
            {
                return Err(error(format!(
                    "untrusted front-door path: {}",
                    part.display()
                )));
            }
            if part == path && (meta.is_file() != regular || regular && meta.nlink() != 1) {
                return Err(error("wrong front-door file type"));
            }
            if part == Path::new("/") {
                break;
            }
            part = part
                .parent()
                .ok_or_else(|| error("front-door path has no parent"))?;
        }
        Ok(())
    }

    fn read_limited(path: &Path, max: u64, local_owner: Option<u32>) -> io::Result<Vec<u8>> {
        trusted(path, true, local_owner)?;
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        let before = file.metadata()?;
        let mut bytes = Vec::new();
        (&mut file).take(max + 1).read_to_end(&mut bytes)?;
        let after = file.metadata()?;
        if bytes.len() as u64 > max
            || before.dev() != after.dev()
            || before.ino() != after.ino()
            || before.len() != after.len()
            || before.mtime_nsec() != after.mtime_nsec()
            || before.ctime_nsec() != after.ctime_nsec()
        {
            return Err(error("front-door file changed or is oversized"));
        }
        Ok(bytes)
    }

    fn digest(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn checked_image(path: &Path, expected: &str, local_owner: Option<u32>) -> io::Result<File> {
        if expected.len() != 64
            || !expected
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(error("invalid image digest"));
        }
        trusted(path, true, local_owner)?;
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        let before = file.metadata()?;
        let mut hash = Sha256::new();
        let length = io::copy(&mut file, &mut hash)?;
        let opened = file.metadata()?;
        if length != before.len()
            || before.dev() != opened.dev()
            || before.ino() != opened.ino()
            || before.len() != opened.len()
            || before.mtime_nsec() != opened.mtime_nsec()
            || before.ctime_nsec() != opened.ctime_nsec()
            || format!("{:x}", hash.finalize()) != expected
        {
            return Err(error(format!("image mismatch: {}", path.display())));
        }
        let named = fs::metadata(path)?;
        if (named.dev(), named.ino()) != (opened.dev(), opened.ino()) {
            return Err(error("image replaced after verification"));
        }
        file.seek(SeekFrom::Start(0))?;
        Ok(file)
    }

    fn verify_generation_files(g: &Generation) -> io::Result<()> {
        // Staging remains private 0400 and is checked by the root installer.
        // Public launchers validate the root-published executable/config copies.
        if g.stage_manifest_sha256.len() != 64 || !g.stage_path.is_absolute() {
            return Err(error("invalid stage provenance"));
        }
        for (path, hash, owner) in [
            (
                &g.legacy_runner_config,
                &g.legacy_runner_config_sha256,
                Some(g.legacy_runner_config_owner_uid),
            ),
            (
                &g.legacy_bash_config,
                &g.legacy_bash_config_sha256,
                Some(g.legacy_bash_config_owner_uid),
            ),
            (&g.fresh_runner_image, &g.fresh_runner_sha256, None),
            (&g.fresh_runner_config, &g.fresh_runner_config_sha256, None),
            (&g.fresh_bash_image, &g.fresh_bash_sha256, None),
            (&g.fresh_bash_config, &g.fresh_bash_config_sha256, None),
        ] {
            checked_image(path, hash, owner)?;
        }
        for path in [&g.fresh_runner_image, &g.fresh_bash_image] {
            if fs::metadata(path)?.mode() & 0o111 != 0 {
                return Err(error(
                    "fresh image became executable while effect is closed",
                ));
            }
        }
        Ok(())
    }

    fn role(g: &Generation, root: &Path) -> io::Result<Role> {
        if let Some(bindings) = &g.adapter_bindings {
            if bindings.runner != g.aliases.runner || bindings.bash != g.aliases.bash {
                return Err(error("adapter binding differs from managed alias"));
            }
        }
        // AT_EXECFN is the kernel's exec path, unlike caller-controlled argv[0].
        let pointer = unsafe { libc::getauxval(libc::AT_EXECFN) } as *const libc::c_char;
        if pointer.is_null() {
            return Err(error("entry path unavailable"));
        }
        let exec_path = Path::new(OsStr::from_bytes(
            unsafe { CStr::from_ptr(pointer) }.to_bytes(),
        ));
        let invoked = if exec_path.is_absolute() {
            exec_path.to_path_buf()
        } else if exec_path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return Err(error("relative entry contains parent traversal"));
        } else {
            std::env::current_dir()?.join(exec_path)
        };
        let launcher = root.join("launcher/oulipoly-shared-front-door");
        for (path, owner, found) in [
            (
                Some(&g.aliases.runner),
                Some(g.alias_owner_uids.runner),
                Role::Runner,
            ),
            (
                Some(&g.aliases.runner_alt),
                Some(g.alias_owner_uids.runner_alt),
                Role::RunnerAlt,
            ),
            (g.aliases.gui.as_ref(), g.alias_owner_uids.gui, Role::Gui),
            (
                Some(&g.aliases.bash),
                Some(g.alias_owner_uids.bash),
                Role::Bash,
            ),
        ] {
            if path.is_some_and(|path| invoked == *path) {
                let path = path.ok_or_else(|| error("managed alias absent"))?;
                let meta = fs::symlink_metadata(path)?;
                if !meta.file_type().is_symlink()
                    || meta.uid() != owner.ok_or_else(|| error("alias owner absent"))?
                    || fs::read_link(path)? != launcher
                {
                    return Err(error("managed alias changed"));
                }
                if !cfg!(feature = "age319-private-broker-fixture") {
                    trusted_alias_parent(
                        path.parent().ok_or_else(|| error("alias parent absent"))?,
                        meta.uid(),
                    )?;
                }
                return Ok(found);
            }
        }
        Err(error("entry is not a managed alias"))
    }

    fn trusted_alias_parent(path: &Path, owner: u32) -> io::Result<()> {
        let mut part = path;
        if fs::symlink_metadata(part)?.uid() != owner {
            return Err(error("managed alias owner differs from parent"));
        }
        loop {
            let meta = fs::symlink_metadata(part)?;
            if !meta.is_dir()
                || meta.file_type().is_symlink()
                || meta.mode() & 0o022 != 0
                || meta.uid() != 0 && meta.uid() != owner
            {
                return Err(error(format!(
                    "untrusted managed alias parent: {}",
                    part.display()
                )));
            }
            if part == Path::new("/") {
                break;
            }
            part = part.parent().ok_or_else(|| error("alias parent absent"))?;
        }
        Ok(())
    }

    fn exact_old_handle(root: &Path, args: &[std::ffi::OsString], owner: u32) -> io::Result<bool> {
        let words: Vec<_> = args.iter().map(|arg| arg.to_string_lossy()).collect();
        if words.len() < 3
            || !matches!(
                words[1].as_ref(),
                "status" | "snapshot" | "mode" | "accept-output" | "detach" | "cancel"
            )
        {
            return Ok(false);
        }
        let candidates: Vec<_> = words[2..]
            .iter()
            .filter(|word| {
                word.starts_with("ab_")
                    && !word.starts_with("ab30_")
                    && word.len() <= 128
                    && word.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            })
            .collect();
        if candidates.len() != 1 {
            return Ok(false);
        }
        let handle = candidates[0];
        let path = root.join(handle.as_ref()).join("meta.json");
        let bytes = match read_limited(&path, 1024 * 1024, Some(owner)) {
            Ok(bytes) => bytes,
            Err(_) => return Ok(false),
        };
        let value: serde_json::Value = serde_json::from_slice(&bytes)?;
        Ok(value["handle"] == handle.as_ref())
    }

    fn exec_image(file: &File) -> io::Result<()> {
        let arguments: Vec<CString> = std::env::args_os()
            .map(|a| CString::new(a.as_bytes()).map_err(|_| error("NUL in argument")))
            .collect::<io::Result<_>>()?;
        let environment: Vec<CString> = std::env::vars_os()
            .map(|(k, v)| {
                let mut pair = k.as_bytes().to_vec();
                pair.push(b'=');
                pair.extend_from_slice(v.as_bytes());
                CString::new(pair).map_err(|_| error("NUL in environment"))
            })
            .collect::<io::Result<_>>()?;
        let mut argv: Vec<*mut libc::c_char> =
            arguments.iter().map(|a| a.as_ptr().cast_mut()).collect();
        argv.push(std::ptr::null_mut());
        let mut envp: Vec<*mut libc::c_char> =
            environment.iter().map(|e| e.as_ptr().cast_mut()).collect();
        envp.push(std::ptr::null_mut());
        let empty = CString::new("").unwrap();
        let rc = unsafe {
            libc::execveat(
                file.as_raw_fd(),
                empty.as_ptr(),
                argv.as_ptr(),
                envp.as_ptr(),
                libc::AT_EMPTY_PATH,
            )
        };
        debug_assert_eq!(rc, -1);
        Err(io::Error::last_os_error())
    }

    fn lock_shared(root: &Path) -> io::Result<File> {
        let path = root.join("publication.lock");
        trusted(&path, true, None)?;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(file)
    }

    fn observe_broker(path: &Path, image: &Path) -> io::Result<BrokerRoute> {
        let mut stream = UnixStream::connect(path)?;
        stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(std::time::Duration::from_secs(5)))?;
        let mut peer = libc::ucred {
            pid: 0,
            uid: 0,
            gid: 0,
        };
        let mut length = std::mem::size_of_val(&peer) as libc::socklen_t;
        if unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&mut peer as *mut libc::ucred).cast(),
                &mut length,
            )
        } != 0
            || length as usize != std::mem::size_of_val(&peer)
            || peer.uid
                != if cfg!(feature = "age319-private-broker-fixture") {
                    unsafe { libc::geteuid() }
                } else {
                    0
                }
        {
            return Err(error("broker peer is not expected owner"));
        }
        if !cfg!(feature = "age319-private-broker-fixture") {
            let named = fs::metadata(image)?;
            let running = fs::metadata(format!("/proc/{}/exe", peer.pid))?;
            if (named.dev(), named.ino()) != (running.dev(), running.ino()) {
                return Err(error("live broker image differs from generation"));
            }
        }
        let mut challenge = [0u8; 16];
        stream.read_exact(&mut challenge)?;
        let mut request = [b'I'; 17];
        request[1..].copy_from_slice(&challenge);
        stream.write_all(&request)?;
        let mut response = Vec::new();
        (&mut stream).take(257).read_to_end(&mut response)?;
        if response.len() > 256 || !response.ends_with(b"\n") {
            return Err(error("invalid broker route response"));
        }
        let response =
            std::str::from_utf8(&response).map_err(|_| error("invalid broker route text"))?;
        let fields: Vec<_> = response.trim_end_matches('\n').split(' ').collect();
        if fields.len() != 4 || fields[0] != "fresh-v30-route" {
            return Err(error("fresh broker route unavailable"));
        }
        let valid =
            |value: &str| uuid::Uuid::parse_str(value).is_ok_and(|id| id.to_string() == value);
        if !fields[1..].iter().copied().all(valid) {
            return Err(error("noncanonical broker route"));
        }
        Ok(BrokerRoute {
            lane_id: fields[1].into(),
            source_generation: fields[2].into(),
            domain_id: fields[3].into(),
        })
    }

    fn active(root: &Path) -> io::Result<(Selector, Generation)> {
        let selector: Selector =
            serde_json::from_slice(&read_limited(&root.join("active.json"), 16 * 1024, None)?)?;
        if selector.schema != 2
            || selector.publication_id.is_empty()
            || selector.publication_id.len() > 128
            || selector.generation.contains('/')
            || selector.generation.starts_with('.')
            || selector.manifest_sha256.len() != 64
            || !matches!(selector.phase.as_str(), "legacy" | "fresh-closed")
            || selector.phase == "legacy" && selector.epoch != 0
            || selector.phase == "fresh-closed" && selector.epoch == 0
        {
            return Err(error("invalid selector"));
        }
        let path = root
            .join("generations")
            .join(&selector.generation)
            .join("manifest.json");
        let bytes = read_limited(&path, 64 * 1024, None)?;
        if digest(&bytes) != selector.manifest_sha256 {
            return Err(error("generation manifest mismatch"));
        }
        let generation: Generation = serde_json::from_slice(&bytes)?;
        if generation.schema != 2 || generation.generation != selector.generation {
            return Err(error("generation identity mismatch"));
        }
        Ok((selector, generation))
    }

    fn run() -> io::Result<()> {
        let current = fs::read_link("/proc/self/exe")?;
        let root = current
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| error("launcher has no control root"))?;
        let expected = root.join("launcher/oulipoly-shared-front-door");
        if current != expected {
            return Err(error("wrong launcher image path"));
        }
        trusted(root, false, None)?;
        let _lock = lock_shared(root)?;
        let (selector, g) = active(root)?;
        let current_file = checked_image(&expected, &g.launcher_sha256, None)?;
        let running = File::open("/proc/self/exe")?;
        if (
            current_file.metadata()?.dev(),
            current_file.metadata()?.ino(),
        ) != (running.metadata()?.dev(), running.metadata()?.ino())
        {
            return Err(error("running launcher was replaced"));
        }
        let role = role(&g, root)?;
        verify_generation_files(&g)?;
        let old_runner = checked_image(
            &g.legacy_runner_entry,
            &g.legacy_runner_sha256,
            Some(g.legacy_runner_owner_uid),
        )?;
        let old_bash = checked_image(
            &g.legacy_bash_entry,
            &g.legacy_bash_sha256,
            Some(g.legacy_bash_owner_uid),
        )?;
        let broker = checked_image(&g.broker_image, &g.broker_image_sha256, None)?;
        drop(broker);
        if selector.phase == "legacy" {
            let (path, hash, owner) = match role {
                Role::Runner => (
                    &g.legacy_alias_targets.runner,
                    &g.legacy_alias_sha256.runner,
                    g.legacy_alias_owner_uids.runner,
                ),
                Role::RunnerAlt => (
                    &g.legacy_alias_targets.runner_alt,
                    &g.legacy_alias_sha256.runner_alt,
                    g.legacy_alias_owner_uids.runner_alt,
                ),
                Role::Gui => (
                    g.legacy_alias_targets
                        .gui
                        .as_ref()
                        .ok_or_else(|| error("GUI alias absent"))?,
                    g.legacy_alias_sha256
                        .gui
                        .as_ref()
                        .ok_or_else(|| error("GUI digest absent"))?,
                    g.legacy_alias_owner_uids
                        .gui
                        .ok_or_else(|| error("GUI owner absent"))?,
                ),
                Role::Bash => (
                    &g.legacy_alias_targets.bash,
                    &g.legacy_alias_sha256.bash,
                    g.legacy_alias_owner_uids.bash,
                ),
            };
            return exec_image(&checked_image(path, hash, Some(owner))?);
        }
        drop(old_runner);
        if matches!(role, Role::Bash)
            && exact_old_handle(
                &g.legacy_bash_state_root,
                &std::env::args_os().collect::<Vec<_>>(),
                g.legacy_bash_owner_uid,
            )?
        {
            return exec_image(&old_bash);
        }
        // Every fresh invocation re-reads the live broker identity. The protocol
        // authenticates the peer as host root; no cached witness authorizes work.
        let observed = observe_broker(&g.broker_socket, &g.broker_image)?;
        if observed.lane_id != g.broker_route.lane_id
            || observed.source_generation != g.broker_route.source_generation
            || observed.domain_id != g.broker_route.domain_id
        {
            return Err(error("broker generation incompatible with selector"));
        }
        Err(error(
            "fresh effect closed; independent root and child lineage unproved",
        ))
    }

    pub fn main() {
        #[cfg(feature = "age319-private-broker-fixture")]
        std::hint::black_box(&super::FIXTURE_ONLY_MARKER);
        if let Err(e) = run() {
            eprintln!("OULIPOLY_FRONT_DOOR_CLOSED={e}");
            std::process::exit(69);
        }
    }
}

#[cfg(target_os = "linux")]
fn main() {
    linux::main();
}
#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("front door requires Linux");
    std::process::exit(69);
}
