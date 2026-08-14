// apps/conary/src/commands/ccs/local_dev.rs

use anyhow::{Context, Result};
use conary_core::ccs::signing::SigningKeyPair;
use fs2::FileExt;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

pub struct LocalDevKeyPaths {
    pub private: PathBuf,
    pub public: PathBuf,
    lock: PathBuf,
}

pub fn local_dev_key_paths() -> Result<LocalDevKeyPaths> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .context("HOME or XDG_DATA_HOME is required for local-dev CCS keys")?
        .join("conary")
        .join("ccs")
        .join("local-dev");
    Ok(LocalDevKeyPaths {
        private: base.join("local-dev-key.private.toml"),
        public: base.join("local-dev-key.public.toml"),
        lock: base.join("local-dev-key.lock"),
    })
}

pub fn load_or_create_local_dev_key() -> Result<SigningKeyPair> {
    let paths = local_dev_key_paths()?;
    load_or_create_local_dev_key_at(&paths)
}

fn load_or_create_local_dev_key_at(paths: &LocalDevKeyPaths) -> Result<SigningKeyPair> {
    let _lock = acquire_initialization_lock(&paths.lock)?;
    if let Some(key) = load_authoritative_key(paths)? {
        return Ok(key);
    }

    let key = SigningKeyPair::generate().with_key_id("local-dev");
    key.save_to_files(&paths.private, &paths.public)
        .map_err(anyhow::Error::from)
        .with_context(|| format!("write local-dev key {}", paths.private.display()))?;
    Ok(key)
}

fn load_authoritative_key(paths: &LocalDevKeyPaths) -> Result<Option<SigningKeyPair>> {
    let private_exists = paths
        .private
        .try_exists()
        .with_context(|| format!("inspect local-dev key {}", paths.private.display()))?;

    if !private_exists {
        return Ok(None);
    }

    let key = SigningKeyPair::load_from_file(&paths.private)
        .map_err(anyhow::Error::from)
        .with_context(|| format!("load local-dev key {}", paths.private.display()))?;
    key.save_public_key_to_file(&paths.public)
        .map_err(anyhow::Error::from)
        .with_context(|| {
            format!(
                "repair local-dev public key {} from {}",
                paths.public.display(),
                paths.private.display()
            )
        })?;
    Ok(Some(key))
}

fn acquire_initialization_lock(path: &Path) -> Result<File> {
    let parent = path
        .parent()
        .context("local-dev key lock must have a parent directory")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("create local-dev key directory {}", parent.display()))?;

    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .with_context(|| format!("open local-dev key lock {}", path.display()))?;
    if !file
        .metadata()
        .with_context(|| format!("inspect local-dev key lock {}", path.display()))?
        .is_file()
    {
        anyhow::bail!(
            "local-dev key lock is not a regular file: {}",
            path.display()
        );
    }
    file.lock_exclusive()
        .with_context(|| format!("lock local-dev key initialization {}", path.display()))?;
    Ok(file)
}

pub fn write_local_dev_policy(path: &Path, key: &SigningKeyPair) -> Result<()> {
    std::fs::write(
        path,
        format!(
            "trusted_keys = [\"{}\"]\nrequire_timestamp = false\n",
            key.public_key_base64()
        ),
    )
    .with_context(|| format!("write local-dev trust policy {}", path.display()))
}

pub fn local_dev_trust_policy() -> Result<Option<conary_core::ccs::verify::TrustPolicy>> {
    let paths = local_dev_key_paths()?;
    let private_exists = paths
        .private
        .try_exists()
        .with_context(|| format!("inspect local-dev key {}", paths.private.display()))?;
    let lock_exists = paths
        .lock
        .try_exists()
        .with_context(|| format!("inspect local-dev key lock {}", paths.lock.display()))?;
    if !private_exists && !lock_exists {
        return Ok(None);
    }
    local_dev_trust_policy_at(&paths)
}

fn local_dev_trust_policy_at(
    paths: &LocalDevKeyPaths,
) -> Result<Option<conary_core::ccs::verify::TrustPolicy>> {
    let _lock = acquire_initialization_lock(&paths.lock)?;
    let Some(key) = load_authoritative_key(paths)? else {
        return Ok(None);
    };
    Ok(Some(conary_core::ccs::verify::TrustPolicy::strict(vec![
        key.public_key_base64(),
    ])))
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::ccs::signing::load_signing_public_key;
    use std::sync::{Arc, Barrier};

    fn key_paths(directory: &Path) -> LocalDevKeyPaths {
        LocalDevKeyPaths {
            private: directory.join("local-dev-key.private.toml"),
            public: directory.join("local-dev-key.public.toml"),
            lock: directory.join("local-dev-key.lock"),
        }
    }

    #[test]
    fn local_dev_policy_trusts_generated_public_key() {
        let temp = tempfile::tempdir().unwrap();
        let key = conary_core::ccs::signing::SigningKeyPair::generate().with_key_id("local-dev");
        let policy_path = temp.path().join("policy.toml");

        write_local_dev_policy(&policy_path, &key).unwrap();
        let policy = conary_core::ccs::verify::TrustPolicy::from_file(&policy_path).unwrap();

        assert_eq!(policy.trusted_keys(), &[key.public_key_base64()]);
    }

    #[test]
    fn old_check_then_write_order_returns_distinct_first_use_authorities() {
        const CALLERS: usize = 8;
        let temp = tempfile::tempdir().unwrap();
        let paths = Arc::new(key_paths(temp.path()));
        let checked_missing = Arc::new(Barrier::new(CALLERS));

        let handles = (0..CALLERS)
            .map(|_| {
                let paths = Arc::clone(&paths);
                let checked_missing = Arc::clone(&checked_missing);
                std::thread::spawn(move || {
                    assert!(!paths.private.exists());
                    checked_missing.wait();
                    let key = SigningKeyPair::generate().with_key_id("local-dev");
                    key.save_to_files(&paths.private, &paths.public).unwrap();
                    key.public_key_base64()
                })
            })
            .collect::<Vec<_>>();
        let authorities = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<std::collections::BTreeSet<_>>();

        assert_eq!(authorities.len(), CALLERS);
    }

    #[test]
    fn concurrent_first_use_returns_one_authoritative_key() {
        const CALLERS: usize = 16;
        let temp = tempfile::tempdir().unwrap();
        let paths = Arc::new(key_paths(temp.path()));
        let start = Arc::new(Barrier::new(CALLERS));

        let handles = (0..CALLERS)
            .map(|_| {
                let paths = Arc::clone(&paths);
                let start = Arc::clone(&start);
                std::thread::spawn(move || {
                    start.wait();
                    load_or_create_local_dev_key_at(&paths)
                        .unwrap()
                        .public_key_base64()
                })
            })
            .collect::<Vec<_>>();
        let authorities = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<std::collections::BTreeSet<_>>();

        assert_eq!(authorities.len(), 1);
        let persisted = load_signing_public_key(&paths.public).unwrap();
        assert_eq!(persisted.public_key_base64(), authorities.first().unwrap());
    }

    #[test]
    fn existing_private_key_repairs_mismatched_public_projection() {
        let temp = tempfile::tempdir().unwrap();
        let paths = key_paths(temp.path());
        let authoritative = SigningKeyPair::generate().with_key_id("local-dev");
        let stale = SigningKeyPair::generate().with_key_id("local-dev");
        authoritative
            .save_to_files(&paths.private, &paths.public)
            .unwrap();
        stale.save_public_key_to_file(&paths.public).unwrap();

        let loaded = load_or_create_local_dev_key_at(&paths).unwrap();
        let projected = load_signing_public_key(&paths.public).unwrap();

        assert_eq!(
            loaded.public_key_base64(),
            authoritative.public_key_base64()
        );
        assert_eq!(
            projected.public_key_base64(),
            authoritative.public_key_base64()
        );
    }

    #[test]
    fn trust_policy_repairs_and_uses_authoritative_private_key() {
        let temp = tempfile::tempdir().unwrap();
        let paths = key_paths(temp.path());
        let authoritative = SigningKeyPair::generate().with_key_id("local-dev");
        let stale = SigningKeyPair::generate().with_key_id("local-dev");
        authoritative
            .save_to_files(&paths.private, &paths.public)
            .unwrap();
        stale.save_public_key_to_file(&paths.public).unwrap();

        let policy = local_dev_trust_policy_at(&paths).unwrap().unwrap();
        let projected = load_signing_public_key(&paths.public).unwrap();

        assert_eq!(policy.trusted_keys(), &[authoritative.public_key_base64()]);
        assert_eq!(
            projected.public_key_base64(),
            authoritative.public_key_base64()
        );
    }
}
