//! Featureless, no-effect executable recipe for an exact held model choice.
//! A later K must reestablish physical execution custody. In particular a
//! script is executed by pathname so its observed inode is not an exec pin.

use crate::normal_model_selection;
use oulipoly_state::mailbox::{
    FreshNormalExecutablePlan, FreshNormalModelSelection, FreshRecipientIdentity,
    FreshReleasedHandoff, FreshV30Lane, FreshV30Session,
};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileExt, MetadataExt};
use std::path::Path;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentSnapshot {
    inherited: Vec<(String, String)>,
    data_dir: String,
}

const MAX_ENV_BYTES: u64 = 1024 * 1024;
const MAX_IMAGE_BYTES: u64 = 512 * 1024 * 1024;

pub fn inherited_environment(file: &File) -> io::Result<EnvironmentSnapshot> {
    let seals = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GET_SEALS) };
    let needed = libc::F_SEAL_SEAL | libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_WRITE;
    if seals < 0 || seals & needed != needed {
        return Err(io::Error::other(
            "normal plan environment descriptor is not sealed",
        ));
    }
    let size = file.metadata()?.len();
    if size > MAX_ENV_BYTES {
        return Err(io::Error::other("normal plan environment oversized"));
    }
    let mut bytes = vec![0; size as usize];
    let mut offset = 0;
    while offset < bytes.len() {
        let n = file.read_at(&mut bytes[offset..], offset as u64)?;
        if n == 0 {
            return Err(io::Error::other("normal plan environment truncated"));
        }
        offset += n;
    }
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

fn image_evidence(path: &Path) -> io::Result<(u64, u64, u64, String, String, bool)> {
    let mut source = File::open(path)?;
    let before = source.metadata()?;
    if !before.is_file() || before.len() > MAX_IMAGE_BYTES {
        return Err(io::Error::other(
            "normal plan image is not bounded regular source",
        ));
    }
    let fdinfo = fs::read_to_string(format!("/proc/self/fdinfo/{}", source.as_raw_fd()))?;
    let mount_id = fdinfo
        .lines()
        .find_map(|line| line.strip_prefix("mnt_id:\t"))
        .ok_or_else(|| io::Error::other("normal plan image mount absent"))?
        .parse::<u64>()
        .map_err(io::Error::other)?;
    let mut hash = Sha256::new();
    let mut magic = [0u8; 4];
    let mut first = true;
    let mut count = 0u64;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = source.read(&mut buf)?;
        if n == 0 {
            break;
        }
        if first {
            magic[..n.min(4)].copy_from_slice(&buf[..n.min(4)]);
            first = false;
        }
        count += n as u64;
        if count > MAX_IMAGE_BYTES {
            return Err(io::Error::other("normal plan image oversized"));
        }
        hash.update(&buf[..n]);
    }
    let after = source.metadata()?;
    let at_path_file = File::open(path)?;
    let at_path = at_path_file.metadata()?;
    let at_path_fdinfo =
        fs::read_to_string(format!("/proc/self/fdinfo/{}", at_path_file.as_raw_fd()))?;
    let at_path_mount = at_path_fdinfo
        .lines()
        .find_map(|line| line.strip_prefix("mnt_id:\t"))
        .ok_or_else(|| io::Error::other("normal plan image path mount absent"))?
        .parse::<u64>()
        .map_err(io::Error::other)?;
    if count != before.len()
        || before.dev() != after.dev()
        || before.ino() != after.ino()
        || before.len() != after.len()
        || before.ctime() != after.ctime()
        || before.ctime_nsec() != after.ctime_nsec()
        || before.mtime() != after.mtime()
        || before.mtime_nsec() != after.mtime_nsec()
        || at_path.dev() != before.dev()
        || at_path.ino() != before.ino()
        || at_path_mount != mount_id
    {
        return Err(io::Error::other("normal plan image changed while reading"));
    }
    // Scripts, including #! scripts, retain ordinary path execution semantics.
    // The observed inode and digest are source evidence, not a promise about
    // a later interpreter open or an eventual K.
    let metadata_sha = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(
            before.uid(),
            before.gid(),
            before.mode(),
            before.len(),
            before.ctime(),
            before.ctime_nsec(),
            before.mtime(),
            before.mtime_nsec(),
        ))?)
    );
    Ok((
        before.dev(),
        before.ino(),
        mount_id,
        format!("{:x}", hash.finalize()),
        metadata_sha,
        magic != *b"\x7fELF",
    ))
}

pub fn candidate(
    selection: FreshNormalModelSelection,
    config_dir: &File,
    cwd_fd: &File,
    environment_fd: &File,
) -> io::Result<FreshNormalExecutablePlan> {
    if normal_model_selection::candidate(selection.invocation.clone(), config_dir)? != selection {
        return Err(io::Error::other("normal plan selection source changed"));
    }
    let cwd_meta = cwd_fd.metadata()?;
    if !cwd_meta.is_dir() {
        return Err(io::Error::other(
            "normal plan cwd descriptor is not a directory",
        ));
    }
    let cwd_path = fs::read_link(format!("/proc/self/fd/{}", cwd_fd.as_raw_fd()))?;
    let cwd_text = cwd_path
        .to_str()
        .ok_or_else(|| io::Error::other("normal plan cwd is not UTF-8"))?
        .to_owned();
    if !cwd_path.is_absolute()
        || !fs::metadata(&cwd_path)
            .is_ok_and(|meta| meta.dev() == cwd_meta.dev() && meta.ino() == cwd_meta.ino())
    {
        return Err(io::Error::other("normal plan cwd path changed"));
    }
    let snapshot = inherited_environment(environment_fd)?;
    let data_dir = Path::new(&snapshot.data_dir);
    if !data_dir.is_absolute() || snapshot.data_dir.contains('\0') {
        return Err(io::Error::other("normal plan data directory invalid"));
    }
    let config_path = Path::new("/proc/self/fd").join(config_dir.as_raw_fd().to_string());
    let pool = oulipoly_runtime::executor::cli::fresh_remote::load_fresh_headless_pool(
        &config_path,
        &selection.invocation.model,
    )
    .map_err(io::Error::other)?;
    if pool.config_sha256 != selection.config_sha256
        || pool
            .model
            .providers
            .get(selection.index)
            .is_none_or(|member| member.name != selection.account)
        || pool
            .account_identities
            .get(selection.index)
            .and_then(|id| id.as_deref())
            != Some(selection.account_identity.as_str())
    {
        return Err(io::Error::other("normal plan account changed"));
    }
    let prepared =
        oulipoly_runtime::executor::cli::fresh_remote::prepare_fresh_headless_with_environment(
            &pool.model,
            selection.index,
            &selection.invocation.prompt,
            &cwd_path,
            &snapshot.inherited,
            data_dir,
        )
        .map_err(io::Error::other)?;
    let plan = prepared.plan;
    let (image_device, image_inode, image_mount, image_sha, image_metadata_sha, path_execution) =
        image_evidence(&plan.executable)?;
    let environment_sha = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&plan.environment)?)
    );
    let stdin_sha = format!("{:x}", Sha256::digest(&plan.stdin));
    let executable = plan
        .executable
        .to_str()
        .ok_or_else(|| io::Error::other("normal plan executable is not UTF-8"))?
        .to_owned();
    let binding = (
        &selection,
        &cwd_text,
        cwd_meta.dev(),
        cwd_meta.ino(),
        &environment_sha,
        &plan.configured_program,
        &executable,
        image_device,
        image_inode,
        image_mount,
        &image_sha,
        &image_metadata_sha,
        path_execution,
        &plan.argv,
        &stdin_sha,
        plan.stdin.len() as u64,
    );
    let digest = format!("{:x}", Sha256::digest(serde_json::to_vec(&binding)?));
    let result = FreshNormalExecutablePlan {
        selection,
        cwd: cwd_text,
        cwd_device: cwd_meta.dev(),
        cwd_inode: cwd_meta.ino(),
        environment_sha256: environment_sha,
        configured_program: plan.configured_program,
        executable,
        executable_device: image_device,
        executable_inode: image_inode,
        executable_mount_id: image_mount,
        executable_sha256: image_sha,
        executable_metadata_sha256: image_metadata_sha,
        path_execution,
        argv: plan.argv,
        stdin_sha256: stdin_sha,
        stdin_len: plan.stdin.len() as u64,
        plan_sha256: digest,
        state: "planned_no_effect".into(),
    };
    if normal_model_selection::candidate(result.selection.invocation.clone(), config_dir)?
        != result.selection
    {
        return Err(io::Error::other(
            "normal plan config changed during construction",
        ));
    }
    Ok(result)
}

pub fn select(
    lane: &FreshV30Lane,
    receipt: &FreshReleasedHandoff,
    actor: &FreshRecipientIdentity,
    session: &FreshV30Session,
    config_dir: &File,
    cwd: &File,
    environment: &File,
) -> io::Result<FreshNormalExecutablePlan> {
    let selection = normal_model_selection::observe(lane, receipt, actor, session, config_dir)?
        .ok_or_else(|| io::Error::other("normal plan selection absent"))?;
    let plan = candidate(selection, config_dir, cwd, environment)?;
    lane.select_normal_executable_plan(receipt, actor, session, &plan)
        .map_err(io::Error::other)
}

pub fn observe(
    lane: &FreshV30Lane,
    receipt: &FreshReleasedHandoff,
    actor: &FreshRecipientIdentity,
    session: &FreshV30Session,
    config_dir: &File,
    cwd: &File,
    environment: &File,
) -> io::Result<Option<FreshNormalExecutablePlan>> {
    let Some(retained) = lane
        .read_normal_executable_plan(receipt, actor, session)
        .map_err(io::Error::other)?
    else {
        return Ok(None);
    };
    if candidate(retained.selection.clone(), config_dir, cwd, environment)? != retained {
        return Err(io::Error::other(
            "normal plan source, cwd, environment or selection changed",
        ));
    }
    Ok(Some(retained))
}

#[cfg(test)]
mod tests {
    use super::*;
    use oulipoly_state::mailbox::{FreshHeadlessModelInvocation, FreshRecipientIdentity};
    use std::os::fd::FromRawFd;
    use std::os::unix::fs::PermissionsExt;

    fn environment(data_dir: &Path, marker: &str) -> File {
        let fd = unsafe {
            libc::memfd_create(
                c"normal-plan-test-env".as_ptr(),
                libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
            )
        };
        assert!(fd >= 0);
        let mut file = unsafe { File::from_raw_fd(fd) };
        serde_json::to_writer(
            &mut file,
            &serde_json::json!({
                "inherited": [["PATH", "/usr/bin:/bin"], ["PLAN_MARKER", marker]],
                "data_dir": data_dir,
            }),
        )
        .unwrap();
        let seals =
            libc::F_SEAL_SEAL | libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_WRITE;
        assert_eq!(unsafe { libc::fcntl(fd, libc::F_ADD_SEALS, seals) }, 0);
        file
    }

    #[test]
    fn no_effect_script_plan_recomputes_exact_sources_and_environment() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("config");
        fs::create_dir(&config).unwrap();
        fs::create_dir(config.join("models")).unwrap();
        let script = temp.path().join("provider.sh");
        fs::write(&script, b"#!/bin/sh\ncat\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(
            config.join("providers.toml"),
            format!(
                "[account]\ncommand = '{}'\nquota_account_id = 'physical-account'\n",
                script.display()
            ),
        )
        .unwrap();
        fs::write(
            config.join("models/example.toml"),
            "[[providers]]\nname = 'account'\n",
        )
        .unwrap();
        let invocation = FreshHeadlessModelInvocation {
            root_id: "root".into(),
            owner_generation: "owner".into(),
            handoff_id: "handoff".into(),
            invocation_uuid: "invocation".into(),
            session_id: "session".into(),
            actor: FreshRecipientIdentity {
                host_pid: 1,
                boot_id: "boot".into(),
                starttime_ticks: 2,
                pidns_dev: 3,
                pidns_ino: 4,
            },
            model: "example".into(),
            provider_pin: None,
            prompt: "hello".into(),
        };
        let config_fd = File::open(&config).unwrap();
        let selection = normal_model_selection::candidate(invocation, &config_fd).unwrap();
        let cwd = File::open(temp.path()).unwrap();
        let original = candidate(
            selection.clone(),
            &config_fd,
            &cwd,
            &environment(temp.path(), "one"),
        )
        .unwrap();
        assert!(original.path_execution);
        assert_eq!(original.selection.account_identity, "physical-account");
        assert_eq!(
            original.stdin_sha256,
            format!("{:x}", Sha256::digest(b"hello"))
        );
        assert_eq!(
            candidate(
                selection.clone(),
                &config_fd,
                &cwd,
                &environment(temp.path(), "one")
            )
            .unwrap(),
            original
        );
        assert_ne!(
            candidate(
                selection.clone(),
                &config_fd,
                &cwd,
                &environment(temp.path(), "two")
            )
            .unwrap(),
            original
        );
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        assert_ne!(
            candidate(
                selection.clone(),
                &config_fd,
                &cwd,
                &environment(temp.path(), "one")
            )
            .unwrap(),
            original
        );
        fs::write(&script, b"#!/bin/sh\nprintf changed\n").unwrap();
        assert_ne!(
            candidate(
                selection.clone(),
                &config_fd,
                &cwd,
                &environment(temp.path(), "one")
            )
            .unwrap(),
            original
        );
        fs::write(
            config.join("models/example.toml"),
            "[[providers]]\nname = 'account'\nargs = ['changed']\n",
        )
        .unwrap();
        assert!(
            candidate(
                selection,
                &config_fd,
                &cwd,
                &environment(temp.path(), "one")
            )
            .is_err()
        );
    }
}
