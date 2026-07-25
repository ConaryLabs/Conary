// conary-core/src/repository/static_repo/publish_context/key_management.rs

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::ccs::signing::SigningKeyPair;
use crate::repository::static_repo::{PackageKeyEntry, PackageKeyStatus, PackageKeysFile};
use crate::trust::keys::signing_keypair_to_tuf_key;
use crate::trust::metadata::{RootMetadata, Signed};

#[derive(Default)]
pub(crate) struct PendingKeyRecovery {
    pub(crate) root: bool,
    pub(crate) publish: bool,
}

#[derive(Default)]
pub(crate) struct PendingKeyPromotions {
    entries: Vec<PendingKeyPromotion>,
}

struct PendingKeyPromotion {
    role: String,
    pending_role: String,
}

impl PendingKeyPromotions {
    pub(crate) fn stage_or_load(&mut self, key_dir: &Path, role: &str) -> Result<SigningKeyPair> {
        let pending_role = format!("{role}.pending");
        let key = ensure_pending_key_pair(key_dir, role, &pending_role)?;
        self.track(role);
        Ok(key)
    }

    fn track(&mut self, role: &str) {
        if !self.entries.iter().any(|entry| entry.role == role) {
            self.entries.push(PendingKeyPromotion {
                role: role.to_string(),
                pending_role: format!("{role}.pending"),
            });
        }
    }

    pub(crate) fn promote(&self, key_dir: &Path) -> Result<()> {
        for entry in &self.entries {
            promote_pending_key(key_dir, entry)
                .with_context(|| format!("promote pending {} key", entry.role))?;
        }
        Ok(())
    }
}

pub(crate) fn recover_pending_key_promotions(
    root: &Signed<RootMetadata>,
    key_dir: &Path,
    root_key: &mut SigningKeyPair,
    publish_key: &mut SigningKeyPair,
    pending_key_promotions: &mut PendingKeyPromotions,
) -> Result<PendingKeyRecovery> {
    let mut recovered = PendingKeyRecovery::default();

    if !role_contains_key(root, "root", root_key)?
        && let Some(pending_root_key) = load_pending_key_pair(key_dir, "root")?
        && role_contains_key(root, "root", &pending_root_key)?
    {
        *root_key = pending_root_key;
        pending_key_promotions.track("root");
        recovered.root = true;
    }

    if !publish_roles_contain_key(root, publish_key)?
        && let Some(pending_publish_key) = load_pending_key_pair(key_dir, "publish")?
        && publish_roles_contain_key(root, &pending_publish_key)?
    {
        *publish_key = pending_publish_key;
        pending_key_promotions.track("publish");
        recovered.publish = true;
    }

    Ok(recovered)
}

fn role_contains_key(
    root: &Signed<RootMetadata>,
    role_name: &str,
    key: &SigningKeyPair,
) -> Result<bool> {
    let (key_id, _) = signing_keypair_to_tuf_key(key).map_err(anyhow::Error::from)?;
    let role = root
        .signed
        .roles
        .get(role_name)
        .with_context(|| format!("destination root metadata missing {role_name} role"))?;
    Ok(role.keyids.contains(&key_id))
}

fn publish_roles_contain_key(root: &Signed<RootMetadata>, key: &SigningKeyPair) -> Result<bool> {
    for role_name in ["targets", "snapshot", "timestamp"] {
        if !role_contains_key(root, role_name, key)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn load_pending_key_pair(key_dir: &Path, role: &str) -> Result<Option<SigningKeyPair>> {
    let pending_role = format!("{role}.pending");
    let pending_private = key_dir.join(format!("{pending_role}.private"));
    if !pending_private.exists() {
        return Ok(None);
    }

    let key = SigningKeyPair::load_from_file(&pending_private)
        .map_err(anyhow::Error::from)
        .with_context(|| format!("load pending {role} key {}", pending_private.display()))?;
    save_key_pair(&key, key_dir, &pending_role)
        .with_context(|| format!("refresh pending {role} key files"))?;
    Ok(Some(key))
}

fn ensure_pending_key_pair(
    key_dir: &Path,
    role: &str,
    pending_role: &str,
) -> Result<SigningKeyPair> {
    let pending_private = key_dir.join(format!("{pending_role}.private"));
    if pending_private.exists() {
        let key = SigningKeyPair::load_from_file(&pending_private)
            .map_err(anyhow::Error::from)
            .with_context(|| format!("load pending {role} key {}", pending_private.display()))?;
        save_key_pair(&key, key_dir, pending_role)
            .with_context(|| format!("refresh pending {role} key files"))?;
        return Ok(key);
    }

    let key = SigningKeyPair::generate().with_key_id(role);
    save_key_pair(&key, key_dir, pending_role)
        .with_context(|| format!("stage pending {role} key promotion"))?;
    Ok(key)
}

fn promote_pending_key(key_dir: &Path, entry: &PendingKeyPromotion) -> Result<()> {
    let pending_private = key_dir.join(format!("{}.private", entry.pending_role));
    let pending_public = key_dir.join(format!("{}.public", entry.pending_role));
    let active_private = key_dir.join(format!("{}.private", entry.role));
    let active_public = key_dir.join(format!("{}.public", entry.role));

    fs::rename(&pending_private, &active_private).with_context(|| {
        format!(
            "replace active {} private key {} with {}",
            entry.role,
            active_private.display(),
            pending_private.display()
        )
    })?;
    fs::rename(&pending_public, &active_public).with_context(|| {
        format!(
            "replace active {} public key {} with {}",
            entry.role,
            active_public.display(),
            pending_public.display()
        )
    })
}

pub(crate) fn ensure_key_pair(key_dir: &Path, role: &str) -> Result<SigningKeyPair> {
    let private_path = key_dir.join(format!("{role}.private"));
    if private_path.exists() {
        return SigningKeyPair::load_from_file(&private_path)
            .map_err(anyhow::Error::from)
            .with_context(|| format!("load {role} key {}", private_path.display()));
    }

    let key = SigningKeyPair::generate().with_key_id(role);
    save_key_pair(&key, key_dir, role)?;
    Ok(key)
}

pub(crate) fn save_key_pair(key: &SigningKeyPair, key_dir: &Path, role: &str) -> Result<()> {
    key.save_to_files(
        &key_dir.join(format!("{role}.private")),
        &key_dir.join(format!("{role}.public")),
    )
    .map_err(anyhow::Error::from)
    .with_context(|| format!("save {role} key in {}", key_dir.display()))
}

pub(crate) fn build_package_keys_file(
    old_keys: Option<&PackageKeysFile>,
    publish_key: &SigningKeyPair,
    retired_public_key: Option<String>,
) -> Result<PackageKeysFile> {
    let active_public_key = publish_key.public_key_base64();
    let mut entries = Vec::new();
    let mut seen = BTreeSet::new();

    if let Some(old_keys) = old_keys {
        for key in &old_keys.keys {
            let mut key = key.clone();
            if Some(key.public_key.as_str()) == retired_public_key.as_deref() {
                key.status = PackageKeyStatus::Retired;
            }
            if key.public_key == active_public_key {
                continue;
            }
            if seen.insert(key.public_key.clone()) {
                entries.push(key);
            }
        }
    }

    if let Some(public_key) = retired_public_key
        && public_key != active_public_key
        && seen.insert(public_key.clone())
    {
        entries.push(PackageKeyEntry {
            algorithm: "ed25519".to_string(),
            public_key,
            key_id: Some("publish".to_string()),
            status: PackageKeyStatus::Retired,
            comment: Some("retired publishing key".to_string()),
        });
    }

    entries.push(PackageKeyEntry {
        algorithm: "ed25519".to_string(),
        public_key: active_public_key,
        key_id: Some("publish".to_string()),
        status: PackageKeyStatus::Active,
        comment: Some("primary publishing key".to_string()),
    });

    let keys = PackageKeysFile {
        schema: 1,
        keys: entries,
    };
    keys.validate()?;
    Ok(keys)
}

#[cfg(unix)]
pub(crate) fn create_private_dir_all(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
pub(crate) fn create_private_dir_all(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}
