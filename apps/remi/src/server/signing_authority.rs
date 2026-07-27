// apps/remi/src/server/signing_authority.rs
//! Durable, exact-profile signing authority for Remi conversion and TUF roles.

use crate::server::repository_manifest::RepositoryManifest;
use anyhow::{Context, Result, bail};
use conary_core::ccs::signing::{SigningKeyPair, load_signing_public_key};
use conary_core::repository::supported_profiles::{SupportedProfile, profile_by_public_id};
use nix::errno::Errno;
use nix::fcntl::{RenameFlags, renameat2};
use nix::unistd::{Gid, Uid, chown};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Component, Path};

const AUTHORITY_DIRECTORY_MODE: u32 = 0o700;
const PRIVATE_KEY_MODE: u32 = 0o600;
const PUBLIC_KEY_MODE: u32 = 0o644;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RepositorySigningRole {
    Targets,
    Snapshot,
    Timestamp,
}

impl RepositorySigningRole {
    const ALL: [Self; 3] = [Self::Targets, Self::Snapshot, Self::Timestamp];

    const fn as_str(self) -> &'static str {
        match self {
            Self::Targets => "targets",
            Self::Snapshot => "snapshot",
            Self::Timestamp => "timestamp",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct AuthorityOwner {
    uid: u32,
    gid: u32,
}

pub(crate) fn ensure_repository_authority(
    manifest: &RepositoryManifest,
    keys_root: &Path,
) -> Result<Vec<String>> {
    let profiles = exact_profiles(manifest)?;
    let owner = require_authority_root(keys_root)?;
    reject_route_slug_authority(keys_root, &profiles)?;
    reject_unexpected_root_entries(keys_root, &profiles)?;
    for profile in &profiles {
        ensure_profile_authority(keys_root, profile, owner)?;
    }
    Ok(profiles
        .into_iter()
        .map(|profile| profile.id().to_string())
        .collect())
}

pub(crate) fn inspect_repository_authority(
    manifest: &RepositoryManifest,
    keys_root: &Path,
) -> Result<Vec<String>> {
    let profiles = exact_profiles(manifest)?;
    let owner = require_authority_root(keys_root)?;
    reject_route_slug_authority(keys_root, &profiles)?;
    reject_unexpected_root_entries(keys_root, &profiles)?;
    for profile in &profiles {
        validate_profile_authority(&keys_root.join(profile.id()), profile.id(), owner)?;
    }
    Ok(profiles
        .into_iter()
        .map(|profile| profile.id().to_string())
        .collect())
}

pub(crate) fn load_role_key(
    keys_root: &Path,
    profile_id: &str,
    role: RepositorySigningRole,
) -> Result<SigningKeyPair> {
    let profile = profile_by_public_id(profile_id)
        .with_context(|| format!("unknown exact repository profile '{profile_id}'"))?;
    let owner = require_authority_root(keys_root)?;
    let profile_dir = keys_root.join(profile.id());
    validate_profile_directory(&profile_dir, profile.id(), owner)?;
    validate_role_pair(&profile_dir, profile.id(), role, owner)?;
    SigningKeyPair::load_from_file(&profile_dir.join(format!("{}.private", role.as_str())))
        .map_err(anyhow::Error::from)
        .with_context(|| {
            format!(
                "load Remi {} signing key for exact profile {}",
                role.as_str(),
                profile.id()
            )
        })
}

fn exact_profiles(manifest: &RepositoryManifest) -> Result<Vec<&'static SupportedProfile>> {
    manifest.validate()?;
    let mut profiles = BTreeMap::new();
    for repository in &manifest.repositories {
        let profile = profile_by_public_id(&repository.profile).with_context(|| {
            format!(
                "repository '{}' names unknown exact profile '{}'",
                repository.name, repository.profile
            )
        })?;
        require_safe_profile_component(profile.id())?;
        profiles.insert(profile.id(), profile);
    }
    Ok(profiles.into_values().collect())
}

fn require_safe_profile_component(profile_id: &str) -> Result<()> {
    let mut components = Path::new(profile_id).components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        bail!("repository profile '{profile_id}' is not one safe path component");
    }
    Ok(())
}

fn require_authority_root(keys_root: &Path) -> Result<AuthorityOwner> {
    let metadata = fs::symlink_metadata(keys_root).with_context(|| {
        format!(
            "repository signing authority root {} does not exist",
            keys_root.display()
        )
    })?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        bail!(
            "repository signing authority root {} must be a plain directory",
            keys_root.display()
        );
    }
    require_mode(keys_root, &metadata, AUTHORITY_DIRECTORY_MODE)?;
    Ok(AuthorityOwner {
        uid: metadata.uid(),
        gid: metadata.gid(),
    })
}

fn reject_route_slug_authority(keys_root: &Path, profiles: &[&SupportedProfile]) -> Result<()> {
    for profile in profiles {
        let route = profile.remi_route_slug();
        if route == profile.id() {
            continue;
        }
        match fs::symlink_metadata(keys_root.join(route)) {
            Ok(_) => {
                bail!(
                    "repository signing authority uses route slug '{}' for exact profile '{}'; remove the ambiguous route authority after preserving any required public trust data",
                    route,
                    profile.id()
                );
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "inspect ambiguous route authority {}",
                        keys_root.join(route).display()
                    )
                });
            }
        }
    }
    Ok(())
}

fn reject_unexpected_root_entries(keys_root: &Path, profiles: &[&SupportedProfile]) -> Result<()> {
    let expected = profiles
        .iter()
        .map(|profile| profile.id())
        .collect::<BTreeSet<_>>();
    for entry in fs::read_dir(keys_root)
        .with_context(|| format!("read repository authority root {}", keys_root.display()))?
    {
        let entry = entry.with_context(|| {
            format!(
                "read repository authority entry under {}",
                keys_root.display()
            )
        })?;
        let name = entry.file_name();
        if !expected
            .iter()
            .any(|profile_id| name == std::ffi::OsStr::new(profile_id))
        {
            bail!(
                "repository signing authority root {} contains unexpected entry {:?}",
                keys_root.display(),
                name
            );
        }
    }
    Ok(())
}

fn ensure_profile_authority(
    keys_root: &Path,
    profile: &SupportedProfile,
    owner: AuthorityOwner,
) -> Result<()> {
    let profile_dir = keys_root.join(profile.id());
    match fs::symlink_metadata(&profile_dir) {
        Ok(_) => validate_profile_authority(&profile_dir, profile.id(), owner),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            create_profile_authority(keys_root, profile, owner)
        }
        Err(error) => Err(error).with_context(|| {
            format!(
                "inspect repository signing authority {}",
                profile_dir.display()
            )
        }),
    }
}

fn create_profile_authority(
    keys_root: &Path,
    profile: &SupportedProfile,
    owner: AuthorityOwner,
) -> Result<()> {
    let staging = tempfile::Builder::new()
        .prefix(".authority-")
        .tempdir_in(keys_root)
        .with_context(|| {
            format!(
                "create staged signing authority under {}",
                keys_root.display()
            )
        })?;
    fs::set_permissions(
        staging.path(),
        fs::Permissions::from_mode(AUTHORITY_DIRECTORY_MODE),
    )?;

    for role in RepositorySigningRole::ALL {
        let private = staging.path().join(format!("{}.private", role.as_str()));
        let public = staging.path().join(format!("{}.public", role.as_str()));
        SigningKeyPair::generate()
            .with_key_id(role.as_str())
            .save_to_files(&private, &public)
            .map_err(anyhow::Error::from)
            .with_context(|| {
                format!(
                    "generate Remi {} authority for exact profile {}",
                    role.as_str(),
                    profile.id()
                )
            })?;
        fs::set_permissions(&private, fs::Permissions::from_mode(PRIVATE_KEY_MODE))?;
        fs::set_permissions(&public, fs::Permissions::from_mode(PUBLIC_KEY_MODE))?;
        align_owner(&private, owner)?;
        align_owner(&public, owner)?;
        File::open(&private)?.sync_all()?;
        File::open(&public)?.sync_all()?;
    }
    align_owner(staging.path(), owner)?;
    validate_profile_authority(staging.path(), profile.id(), owner)?;
    File::open(staging.path())?.sync_all()?;

    let root = File::open(keys_root)?;
    let staged_name = staging
        .path()
        .file_name()
        .context("staged authority directory has no basename")?;
    match renameat2(
        &root,
        Path::new(staged_name),
        &root,
        Path::new(profile.id()),
        RenameFlags::RENAME_NOREPLACE,
    ) {
        Ok(()) => {
            root.sync_all()?;
            validate_profile_authority(&keys_root.join(profile.id()), profile.id(), owner)
        }
        Err(Errno::EEXIST) => {
            validate_profile_authority(&keys_root.join(profile.id()), profile.id(), owner)
        }
        Err(error) => Err(anyhow::Error::from(error)).with_context(|| {
            format!(
                "publish staged signing authority for exact profile {}",
                profile.id()
            )
        }),
    }
}

fn validate_profile_authority(
    profile_dir: &Path,
    profile_id: &str,
    owner: AuthorityOwner,
) -> Result<()> {
    validate_profile_directory(profile_dir, profile_id, owner)?;
    reject_unexpected_profile_entries(profile_dir, profile_id)?;
    for role in RepositorySigningRole::ALL {
        validate_role_pair(profile_dir, profile_id, role, owner)?;
    }
    Ok(())
}

fn reject_unexpected_profile_entries(profile_dir: &Path, profile_id: &str) -> Result<()> {
    let expected = RepositorySigningRole::ALL
        .into_iter()
        .flat_map(|role| {
            [
                format!("{}.private", role.as_str()),
                format!("{}.public", role.as_str()),
            ]
        })
        .collect::<BTreeSet<_>>();
    for entry in fs::read_dir(profile_dir)
        .with_context(|| format!("read signing authority for exact profile {profile_id}"))?
    {
        let entry = entry.with_context(|| {
            format!("read signing authority entry for exact profile {profile_id}")
        })?;
        let name = entry.file_name();
        if !expected
            .iter()
            .any(|expected_name| name == std::ffi::OsStr::new(expected_name))
        {
            bail!(
                "repository signing authority for exact profile {profile_id} contains unexpected entry {:?}",
                name
            );
        }
    }
    Ok(())
}

fn validate_profile_directory(
    profile_dir: &Path,
    profile_id: &str,
    owner: AuthorityOwner,
) -> Result<()> {
    let metadata = fs::symlink_metadata(profile_dir).with_context(|| {
        format!("repository signing authority for exact profile {profile_id} is missing")
    })?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        bail!(
            "repository signing authority {} must be a plain directory",
            profile_dir.display()
        );
    }
    require_mode(profile_dir, &metadata, AUTHORITY_DIRECTORY_MODE)?;
    require_owner(profile_dir, &metadata, owner)?;
    Ok(())
}

fn validate_role_pair(
    profile_dir: &Path,
    profile_id: &str,
    role: RepositorySigningRole,
    owner: AuthorityOwner,
) -> Result<()> {
    let private = profile_dir.join(format!("{}.private", role.as_str()));
    let public = profile_dir.join(format!("{}.public", role.as_str()));
    let private_metadata = require_plain_file(&private, "private signing key")?;
    let public_metadata = require_plain_file(&public, "public signing key")?;
    require_mode(&private, &private_metadata, PRIVATE_KEY_MODE)?;
    require_mode(&public, &public_metadata, PUBLIC_KEY_MODE)?;
    require_owner(&private, &private_metadata, owner)?;
    require_owner(&public, &public_metadata, owner)?;

    let private_key = SigningKeyPair::load_from_file(&private)
        .map_err(anyhow::Error::from)
        .with_context(|| {
            format!(
                "load private {} authority for exact profile {profile_id}",
                role.as_str()
            )
        })?;
    if private_key.key_id() != Some(role.as_str()) {
        bail!(
            "private authority {} has key ID {:?}; expected '{}'",
            private.display(),
            private_key.key_id(),
            role.as_str()
        );
    }
    let public_key = load_signing_public_key(&public)
        .map_err(anyhow::Error::from)
        .with_context(|| {
            format!(
                "load public {} authority for exact profile {profile_id}",
                role.as_str()
            )
        })?;
    if public_key.key_id() != Some(role.as_str()) {
        bail!(
            "public authority {} has key ID {:?}; expected '{}'",
            public.display(),
            public_key.key_id(),
            role.as_str()
        );
    }
    if public_key.public_key_base64() != private_key.public_key_base64() {
        bail!(
            "public and private {} authority disagree for exact profile {profile_id}",
            role.as_str()
        );
    }
    Ok(())
}

fn require_plain_file(path: &Path, description: &str) -> Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("{description} {} is missing", path.display()))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        bail!("{description} {} must be a plain file", path.display());
    }
    Ok(metadata)
}

fn require_mode(path: &Path, metadata: &fs::Metadata, expected: u32) -> Result<()> {
    let observed = metadata.permissions().mode() & 0o777;
    if observed != expected {
        bail!(
            "repository signing authority {} has mode {observed:#05o}; expected {expected:#05o}",
            path.display()
        );
    }
    Ok(())
}

fn require_owner(path: &Path, metadata: &fs::Metadata, expected: AuthorityOwner) -> Result<()> {
    if metadata.uid() != expected.uid || metadata.gid() != expected.gid {
        bail!(
            "repository signing authority {} has owner {}:{}; expected {}:{}",
            path.display(),
            metadata.uid(),
            metadata.gid(),
            expected.uid,
            expected.gid
        );
    }
    Ok(())
}

fn align_owner(path: &Path, owner: AuthorityOwner) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.uid() == owner.uid && metadata.gid() == owner.gid {
        return Ok(());
    }
    chown(
        path,
        Some(Uid::from_raw(owner.uid)),
        Some(Gid::from_raw(owner.gid)),
    )
    .with_context(|| {
        format!(
            "set repository signing authority owner on {}",
            path.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{DirBuilderExt, symlink};

    fn manifest() -> RepositoryManifest {
        toml::from_str(
            r#"
schema_version = 2

[[repositories]]
name = "fedora-source"
url = "https://packages.example.test/fedora"
profile = "fedora-44"
enabled = true
priority = 100
metadata_expire_seconds = 21600

[repositories.parser]
package_format = "rpm"
architecture = "x86_64"

[repositories.trust]
ecosystem = "rpm"

[repositories.trust.metadata]
kind = "metalink"
url = "https://mirrors.example.test/metalink"

[[repositories.trust.package_keys]]
url = "https://keys.example.test/fedora.gpg"
fingerprint = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
"#,
        )
        .unwrap()
    }

    fn authority_root() -> (tempfile::TempDir, std::path::PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repository-keys");
        fs::DirBuilder::new()
            .mode(AUTHORITY_DIRECTORY_MODE)
            .create(&root)
            .unwrap();
        (temp, root)
    }

    #[test]
    fn first_bootstrap_is_complete_and_repeat_preserves_exact_key_bytes() {
        let (_temp, root) = authority_root();
        let profiles = ensure_repository_authority(&manifest(), &root).unwrap();
        assert_eq!(profiles, vec!["fedora-44"]);

        let profile_dir = root.join("fedora-44");
        let before = RepositorySigningRole::ALL
            .into_iter()
            .flat_map(|role| {
                let profile_dir = profile_dir.clone();
                ["private", "public"].map(move |kind| {
                    let path = profile_dir.join(format!("{}.{}", role.as_str(), kind));
                    (path.clone(), fs::read(path).unwrap())
                })
            })
            .collect::<BTreeMap<_, _>>();

        ensure_repository_authority(&manifest(), &root).unwrap();
        let after = before
            .keys()
            .map(|path| (path.clone(), fs::read(path).unwrap()))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(after, before);
        assert_eq!(
            inspect_repository_authority(&manifest(), &root).unwrap(),
            vec!["fedora-44"]
        );
    }

    #[test]
    fn partial_or_mismatched_authority_fails_without_replacement() {
        let (_temp, root) = authority_root();
        ensure_repository_authority(&manifest(), &root).unwrap();
        let public = root.join("fedora-44/targets.public");
        fs::remove_file(&public).unwrap();

        let error = ensure_repository_authority(&manifest(), &root).unwrap_err();
        assert!(error.to_string().contains("is missing"), "{error:#}");
        assert!(!public.exists());
    }

    #[test]
    fn insecure_private_mode_and_symlinked_profile_fail_closed() {
        let (_temp, root) = authority_root();
        ensure_repository_authority(&manifest(), &root).unwrap();
        let private = root.join("fedora-44/targets.private");
        fs::set_permissions(&private, fs::Permissions::from_mode(0o644)).unwrap();
        let error = inspect_repository_authority(&manifest(), &root).unwrap_err();
        assert!(error.to_string().contains("expected 0o600"), "{error:#}");

        let (_second_temp, second_root) = authority_root();
        symlink(&root, second_root.join("fedora-44")).unwrap();
        let error = ensure_repository_authority(&manifest(), &second_root).unwrap_err();
        assert!(error.to_string().contains("plain directory"), "{error:#}");
    }

    #[test]
    fn route_slug_directory_is_rejected_as_duplicate_authority() {
        let (_temp, root) = authority_root();
        fs::DirBuilder::new()
            .mode(AUTHORITY_DIRECTORY_MODE)
            .create(root.join("fedora"))
            .unwrap();

        let error = ensure_repository_authority(&manifest(), &root).unwrap_err();
        assert!(
            error.to_string().contains("route slug 'fedora'"),
            "{error:#}"
        );
        assert!(!root.join("fedora-44").exists());
    }

    #[test]
    fn unexpected_root_or_profile_authority_fails_without_replacement() {
        let (_temp, root) = authority_root();
        fs::write(root.join("unowned-key-material"), b"unexpected").unwrap();
        let error = ensure_repository_authority(&manifest(), &root).unwrap_err();
        assert!(error.to_string().contains("unexpected entry"), "{error:#}");
        assert!(!root.join("fedora-44").exists());

        fs::remove_file(root.join("unowned-key-material")).unwrap();
        ensure_repository_authority(&manifest(), &root).unwrap();
        let private = root.join("fedora-44/targets.private");
        let before = fs::read(&private).unwrap();
        fs::write(root.join("fedora-44/legacy.private"), b"unexpected").unwrap();
        let error = ensure_repository_authority(&manifest(), &root).unwrap_err();
        assert!(error.to_string().contains("unexpected entry"), "{error:#}");
        assert_eq!(fs::read(private).unwrap(), before);
    }
}
