//! Broker-owned, no-effect choice for a held normal model invocation.
//! This boundary deliberately has no provider plan, K or quota probe.

use oulipoly_state::mailbox::{
    FreshHeadlessModelInvocation, FreshNormalModelSelection, FreshNormalModelSource,
    FreshRecipientIdentity, FreshReleasedHandoff, FreshV30Lane, FreshV30Session,
};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::ffi::CString;
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;

const MAX_CONFIG_BYTES: u64 = 2 * 1024 * 1024;

fn file_at(parent: &File, name: &str, directory: bool) -> io::Result<File> {
    let name = CString::new(name).map_err(|_| io::Error::other("config name invalid"))?;
    let flags = libc::O_RDONLY
        | libc::O_CLOEXEC
        | libc::O_NOFOLLOW
        | if directory { libc::O_DIRECTORY } else { 0 };
    let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn bounded_file_at(parent: &File, name: &str) -> io::Result<(u64, u64, Vec<u8>)> {
    let file = file_at(parent, name, false)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() > MAX_CONFIG_BYTES || meta.nlink() != 1 {
        return Err(io::Error::other(
            "config source file is not a bounded regular inode",
        ));
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.take(MAX_CONFIG_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != meta.len() {
        return Err(io::Error::other("config source changed during read"));
    }
    Ok((meta.dev(), meta.ino(), bytes))
}

fn source_identity(directory: &File, model: &str) -> io::Result<FreshNormalModelSource> {
    if model.is_empty()
        || model == "."
        || model == ".."
        || model.starts_with('-')
        || model.contains('/')
        || model.contains('\\')
    {
        return Err(io::Error::other("model name is not a single config stem"));
    }
    let dir_meta = directory.metadata()?;
    if !dir_meta.is_dir() {
        return Err(io::Error::other("config source is not a directory"));
    }
    let (providers_device, providers_inode, providers) =
        bounded_file_at(directory, "providers.toml")?;
    let models = file_at(directory, "models", true)?;
    let (model_device, model_inode, model_bytes) =
        bounded_file_at(&models, &format!("{model}.toml"))?;
    let mut hash = Sha256::new();
    for bytes in [&providers, &model_bytes] {
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    Ok(FreshNormalModelSource {
        directory_device: dir_meta.dev(),
        directory_inode: dir_meta.ino(),
        providers_device,
        providers_inode,
        model_device,
        model_inode,
        config_sha256: format!("{:x}", hash.finalize()),
    })
}

/// Recompute the candidate from the pinned source on every request. An exact
/// source identity is retained by State, and an uncertain reply can only read
/// the same one-use choice. Metered accounts require a later quota boundary.
pub fn candidate(
    invocation: FreshHeadlessModelInvocation,
    directory: &File,
) -> io::Result<FreshNormalModelSelection> {
    let before = source_identity(directory, &invocation.model)?;
    let path = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
    let pool = oulipoly_runtime::executor::cli::fresh_remote::load_fresh_headless_pool(
        &path,
        &invocation.model,
    )
    .map_err(io::Error::other)?;
    let after = source_identity(directory, &invocation.model)?;
    if before != after || pool.config_sha256 != before.config_sha256 {
        return Err(io::Error::other(
            "normal model config changed during selection",
        ));
    }
    let total = pool.model.providers.len();
    let mut identities = HashSet::new();
    for identity in &pool.account_identities {
        let identity = identity
            .as_deref()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| io::Error::other("normal model account lacks physical identity"))?;
        if !identities.insert(identity) {
            return Err(io::Error::other(
                "normal model physical account identity duplicated",
            ));
        }
    }
    let index = if let Some(pin) = invocation.provider_pin.as_deref() {
        pool.model
            .providers
            .iter()
            .position(|provider| provider.name == pin)
            .ok_or_else(|| io::Error::other("normal model provider pin absent"))?
    } else if total == 1 {
        0
    } else {
        return Err(io::Error::other(
            "normal model multi-account policy has no production quota authority",
        ));
    };
    if pool.account_effects[index].0.is_some() {
        return Err(io::Error::other(
            "normal model metered account has no production quota authority",
        ));
    }
    Ok(FreshNormalModelSelection {
        invocation,
        config_sha256: before.config_sha256.clone(),
        source: before,
        account: pool.model.providers[index].name.clone(),
        account_identity: pool.account_identities[index].clone().unwrap(),
        index,
        total,
        state: "selected_no_effect".into(),
    })
}

pub fn select(
    lane: &FreshV30Lane,
    receipt: &FreshReleasedHandoff,
    actor: &FreshRecipientIdentity,
    session: &FreshV30Session,
    directory: &File,
) -> io::Result<FreshNormalModelSelection> {
    let held = lane
        .read_normal_work(receipt, actor, session)
        .map_err(io::Error::other)?
        .ok_or_else(|| io::Error::other("normal model held work absent"))?;
    let invocation = held
        .headless_model_invocation(receipt, session)
        .map_err(io::Error::other)?;
    let selection = candidate(invocation, directory)?;
    lane.select_normal_model(receipt, actor, session, &selection)
        .map_err(io::Error::other)
}

pub fn observe(
    lane: &FreshV30Lane,
    receipt: &FreshReleasedHandoff,
    actor: &FreshRecipientIdentity,
    session: &FreshV30Session,
    directory: &File,
) -> io::Result<Option<FreshNormalModelSelection>> {
    let Some(selection) = lane
        .read_normal_model_selection(receipt, actor, session)
        .map_err(io::Error::other)?
    else {
        return Ok(None);
    };
    if candidate(selection.invocation.clone(), directory)? != selection {
        return Err(io::Error::other("normal model source or account changed"));
    }
    Ok(Some(selection))
}

#[cfg(test)]
mod tests {
    use super::*;
    use oulipoly_state::StateDb;
    use std::fs;

    fn invocation(pin: Option<&str>) -> FreshHeadlessModelInvocation {
        FreshHeadlessModelInvocation {
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
            model: "test-model".into(),
            provider_pin: pin.map(str::to_owned),
            prompt: "held prompt".into(),
        }
    }

    #[test]
    fn featureless_source_choice_binds_physical_files_and_refuses_unknown_quota() {
        let temp = tempfile::tempdir().unwrap();
        // The source and State are disposable; no broker service or provider
        // process is involved in this no-effect choice.
        drop(StateDb::open(&temp.path().join("state.db")).unwrap());
        let config = temp.path().join("config");
        fs::create_dir(&config).unwrap();
        fs::create_dir(config.join("models")).unwrap();
        fs::write(
            config.join("providers.toml"),
            "[first]\ncommand = 'echo'\nquota_account_id = 'physical-first'\n\
             [second]\ncommand = 'echo'\nquota_account_id = 'physical-second'\n",
        )
        .unwrap();
        fs::write(
            config.join("models/test-model.toml"),
            "[[providers]]\nname = 'first'\n[[providers]]\nname = 'second'\n",
        )
        .unwrap();
        let source = File::open(&config).unwrap();
        assert!(candidate(invocation(None), &source).is_err());
        let first = candidate(invocation(Some("first")), &source).unwrap();
        assert_eq!(first.account, "first");
        assert_eq!(first.account_identity, "physical-first");
        assert_eq!(first.index, 0);
        assert_eq!(first.total, 2);
        assert_eq!(first.state, "selected_no_effect");
        assert_eq!(
            candidate(invocation(Some("first")), &source).unwrap(),
            first
        );
        assert!(candidate(invocation(Some("missing")), &source).is_err());

        let other = temp.path().join("other");
        fs::create_dir(&other).unwrap();
        fs::create_dir(other.join("models")).unwrap();
        fs::copy(config.join("providers.toml"), other.join("providers.toml")).unwrap();
        fs::copy(
            config.join("models/test-model.toml"),
            other.join("models/test-model.toml"),
        )
        .unwrap();
        assert_ne!(
            candidate(invocation(Some("first")), &File::open(other).unwrap()).unwrap(),
            first
        );

        fs::write(config.join("providers.toml"),
            "[first]\ncommand = 'echo'\nquota_account_id = 'physical-first'\n\
             quota_script = 'echo quota'\n[second]\ncommand = 'echo'\nquota_account_id = 'physical-second'\n").unwrap();
        assert!(candidate(invocation(Some("first")), &source).is_err());
        assert_ne!(
            candidate(invocation(Some("second")), &source)
                .unwrap()
                .source,
            first.source
        );
        fs::remove_file(config.join("models/test-model.toml")).unwrap();
        std::os::unix::fs::symlink("../providers.toml", config.join("models/test-model.toml"))
            .unwrap();
        assert!(candidate(invocation(Some("second")), &source).is_err());
    }
}
