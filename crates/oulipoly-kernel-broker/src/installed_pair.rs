//! Read-only identity contract for the opt-in Linux paired artifact.
//! This is an ingress prerequisite, not process custody or a State cutover proof.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, Read};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

pub const MANIFEST: &str = "/usr/local/libexec/oulipoly/install-v1.json";
pub const RUNNER: &str = "/usr/local/libexec/oulipoly/oulipoly-agent-runner";
pub const BROKER: &str = "/usr/local/libexec/oulipoly/oulipoly-kernel-broker";
pub const LAUNCHER: &str = "/usr/local/libexec/oulipoly/oulipoly-installed-launcher";
pub const BASH: &str = "/usr/local/libexec/oulipoly/agent-bash";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledPair {
    pub schema: u8,
    pub version: String,
    pub generation: String,
    pub runner_sha256: String,
    pub broker_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launcher_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bash_sha256: Option<String>,
}

impl InstalledPair {
    pub fn load(path: &Path, require_root: bool) -> io::Result<Self> {
        if require_root {
            trusted_path(path)?;
        }
        let file = File::open(path)?;
        trusted_file(&file, require_root)?;
        let mut bytes = Vec::new();
        file.take(4097).read_to_end(&mut bytes)?;
        if bytes.len() > 4096 {
            return Err(io::Error::other("oversized installed pair manifest"));
        }
        let pair: Self = serde_json::from_slice(&bytes)?;
        if !matches!(pair.schema, 1 | 2)
            || (pair.schema == 1 && pair.bash_sha256.is_some())
            || (pair.schema == 2 && pair.bash_sha256.is_none())
            || pair.version != env!("CARGO_PKG_VERSION")
            || uuid::Uuid::parse_str(&pair.generation)
                .ok()
                .is_none_or(|id| id.to_string() != pair.generation)
            || !valid_digest(&pair.runner_sha256)
            || !valid_digest(&pair.broker_sha256)
            || pair
                .launcher_sha256
                .as_deref()
                .is_some_and(|hash| !valid_digest(hash))
            || pair
                .bash_sha256
                .as_deref()
                .is_some_and(|hash| !valid_digest(hash))
        {
            return Err(io::Error::other("incompatible installed pair manifest"));
        }
        Ok(pair)
    }

    pub fn verify_image(
        &self,
        path: &Path,
        expected_sha: &str,
        require_root: bool,
    ) -> io::Result<()> {
        self.verify_image_against(
            path,
            expected_sha,
            require_root,
            &File::open("/proc/self/exe")?,
        )
    }

    pub fn verify_image_against(
        &self,
        path: &Path,
        expected_sha: &str,
        require_root: bool,
        current: &File,
    ) -> io::Result<()> {
        let file = File::open(path)?;
        self.verify_file(path, expected_sha, require_root, &file)?;
        let named = file.metadata()?;
        let running = current.metadata()?;
        if (named.dev(), named.ino()) != (running.dev(), running.ino()) {
            return Err(io::Error::other(
                "running executable is not the installed image",
            ));
        }
        Ok(())
    }

    pub fn verify_file(
        &self,
        path: &Path,
        expected_sha: &str,
        require_root: bool,
        file: &File,
    ) -> io::Result<()> {
        if require_root {
            trusted_path(path)?;
        }
        trusted_file(file, require_root)?;
        let named = fs::metadata(path)?;
        let opened = file.metadata()?;
        if (named.dev(), named.ino()) != (opened.dev(), opened.ino()) {
            return Err(io::Error::other("installed image changed after open"));
        }
        let mut hash = Sha256::new();
        let mut reader = file;
        io::copy(&mut reader, &mut hash)?;
        if format!("{:x}", hash.finalize()) != expected_sha {
            return Err(io::Error::other("installed executable digest mismatch"));
        }
        Ok(())
    }
}

fn trusted_path(path: &Path) -> io::Result<()> {
    if !path.is_absolute() {
        return Err(io::Error::other("relative installed pair path"));
    }
    let mut part = path;
    loop {
        let meta = fs::symlink_metadata(part)?;
        if meta.uid() != 0 || meta.mode() & 0o022 != 0 || meta.file_type().is_symlink() {
            return Err(io::Error::other("untrusted installed pair path"));
        }
        if part == Path::new("/") {
            break;
        }
        part = part
            .parent()
            .ok_or_else(|| io::Error::other("invalid installed pair path"))?;
    }
    Ok(())
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn trusted_file(file: &File, require_root: bool) -> io::Result<()> {
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.nlink() != 1
        || meta.mode() & 0o022 != 0
        || require_root && meta.uid() != 0
    {
        return Err(io::Error::other("untrusted installed pair file"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(manifest: &Path) -> InstalledPair {
        let executable = std::env::current_exe().unwrap();
        let mut hash = Sha256::new();
        io::copy(&mut File::open(&executable).unwrap(), &mut hash).unwrap();
        let pair = InstalledPair {
            schema: 2,
            version: env!("CARGO_PKG_VERSION").into(),
            generation: uuid::Uuid::new_v4().to_string(),
            runner_sha256: format!("{:x}", hash.finalize()),
            broker_sha256: "a".repeat(64),
            launcher_sha256: Some(format!(
                "{:x}",
                Sha256::digest(fs::read(&executable).unwrap())
            )),
            bash_sha256: Some(format!(
                "{:x}",
                Sha256::digest(fs::read(&executable).unwrap())
            )),
        };
        fs::write(manifest, serde_json::to_vec(&pair).unwrap()).unwrap();
        pair
    }

    #[test]
    fn missing_old_and_wrong_installed_images_refuse() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("install-v1.json");
        assert!(InstalledPair::load(&path, false).is_err());
        let pair = fixture(&path);
        let executable = std::env::current_exe().unwrap();
        pair.verify_image(&executable, &pair.runner_sha256, false)
            .unwrap();
        pair.verify_image(&executable, pair.launcher_sha256.as_deref().unwrap(), false)
            .unwrap();
        pair.verify_image(&executable, pair.bash_sha256.as_deref().unwrap(), false)
            .unwrap();
        assert!(
            pair.verify_image(&executable, &pair.broker_sha256, false)
                .is_err()
        );
        let stale = temp.path().join("old-runner");
        fs::copy(&executable, &stale).unwrap();
        assert!(
            pair.verify_image(&stale, &pair.runner_sha256, false)
                .is_err()
        );
        assert!(
            pair.verify_image(&stale, pair.launcher_sha256.as_deref().unwrap(), false)
                .is_err()
        );
        let mut old = pair;
        old.version = "0.0.0".into();
        fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
        assert!(InstalledPair::load(&path, false).is_err());
        old.version = env!("CARGO_PKG_VERSION").into();
        old.bash_sha256 = Some("wrong".into());
        fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
        assert!(InstalledPair::load(&path, false).is_err());
    }

    #[test]
    fn retained_v1_manifest_still_loads_without_bash() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("install-v1.json");
        let mut old = fixture(&path);
        old.schema = 1;
        old.bash_sha256 = None;
        fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
        let loaded = InstalledPair::load(&path, false).unwrap();
        assert_eq!(loaded.schema, 1);
        assert!(loaded.bash_sha256.is_none());
        old.schema = 2;
        fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
        assert!(InstalledPair::load(&path, false).is_err());
    }
}
