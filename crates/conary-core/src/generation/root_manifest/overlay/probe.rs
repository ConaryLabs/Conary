// conary-core/src/generation/root_manifest/overlay/probe.rs

//! Functional host/filesystem preflight for the selected-root OverlayFS profile.

use super::{OverlayXattrNamespace, SelectedRootOverlayProfile};
use nix::mount::{MntFlags, MsFlags, mount, umount2};
use serde::{Deserialize, Serialize};
use std::fs;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub const SELECTED_ROOT_OVERLAY_CAPABILITIES_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OverlayWhiteoutEncoding {
    CharacterDeviceZeroZero,
    ZeroSizeRegularXattr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OverlayHardlinkCopyUp {
    Preserved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OverlayLowerDirectoryRename {
    CrossDevice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OverlayMetadataCopyUp {
    CompleteData,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OverlayOpaqueDirectory {
    LogicalY,
}

/// Exact behavior observed on one candidate runtime filesystem.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedRootOverlayCapabilities {
    pub version: u32,
    pub profile: SelectedRootOverlayProfile,
    pub whiteout_encoding: OverlayWhiteoutEncoding,
    pub opaque_directory: OverlayOpaqueDirectory,
    pub hardlink_copy_up: OverlayHardlinkCopyUp,
    pub lower_directory_rename: OverlayLowerDirectoryRename,
    pub metadata_copy_up: OverlayMetadataCopyUp,
}

/// Functionally prove the selected-root profile on `workspace`'s filesystem.
///
/// The probe creates one private disposable lower/upper/work/mount tuple,
/// mounts the exact requested options, exercises semantic operations, syncs,
/// unmounts, and lets `TempDir` discard every artifact. A mount/version/module
/// name is never accepted as a substitute for this behavior.
pub fn probe_selected_root_overlay_profile(
    workspace: &Path,
    profile: &SelectedRootOverlayProfile,
) -> crate::Result<SelectedRootOverlayCapabilities> {
    profile.validate()?;
    fs::create_dir_all(workspace).map_err(|error| {
        crate::Error::IoError(format!(
            "failed to create selected-root overlay probe workspace {}: {error}",
            workspace.display()
        ))
    })?;
    let probe = tempfile::Builder::new()
        .prefix(".overlay-capability-")
        .tempdir_in(workspace)
        .map_err(|error| {
            crate::Error::IoError(format!(
                "failed to create selected-root overlay probe in {}: {error}",
                workspace.display()
            ))
        })?;
    let lower = probe.path().join("lower");
    let upper = probe.path().join("upper");
    let work = probe.path().join("work");
    let merged = probe.path().join("merged");
    for directory in [&lower, &upper, &work, &merged] {
        fs::create_dir(directory)?;
    }
    seed_lower(&lower)?;

    let options = mount_data(&lower, &upper, &work, profile)?;
    mount(
        Some("overlay"),
        &merged,
        Some("overlay"),
        MsFlags::empty(),
        Some(options.as_str()),
    )
    .map_err(|error| {
        crate::Error::NotImplemented(format!(
            "selected-root OverlayFS profile mount failed on {}: {error}",
            workspace.display()
        ))
    })?;
    let mounted = MountedOverlay::new(merged.clone());

    prove_hardlink_copy_up(&merged, &upper, profile)?;
    let whiteout_encoding = prove_whiteout(&merged, &upper, profile)?;
    prove_opaque_replacement(&merged, &upper, profile)?;
    prove_redirect_disabled(&merged)?;
    sync_filesystem(&upper)?;
    mounted.unmount()?;

    Ok(SelectedRootOverlayCapabilities {
        version: SELECTED_ROOT_OVERLAY_CAPABILITIES_VERSION,
        profile: profile.clone(),
        whiteout_encoding,
        opaque_directory: OverlayOpaqueDirectory::LogicalY,
        hardlink_copy_up: OverlayHardlinkCopyUp::Preserved,
        lower_directory_rename: OverlayLowerDirectoryRename::CrossDevice,
        metadata_copy_up: OverlayMetadataCopyUp::CompleteData,
    })
}

fn seed_lower(lower: &Path) -> crate::Result<()> {
    fs::write(lower.join("hardlink-a"), b"hardlink-content")?;
    fs::hard_link(lower.join("hardlink-a"), lower.join("hardlink-b"))?;
    fs::write(lower.join("remove-me"), b"remove")?;
    fs::create_dir(lower.join("replace-me"))?;
    fs::write(lower.join("replace-me/lower-child"), b"lower")?;
    fs::create_dir(lower.join("rename-me"))?;
    fs::write(lower.join("rename-me/lower-child"), b"lower")?;
    Ok(())
}

fn mount_data(
    lower: &Path,
    upper: &Path,
    work: &Path,
    profile: &SelectedRootOverlayProfile,
) -> crate::Result<String> {
    let mut components = Vec::new();
    for (name, path) in [("lowerdir", lower), ("upperdir", upper), ("workdir", work)] {
        let value = path.to_str().ok_or_else(|| {
            crate::Error::NotImplemented(format!(
                "selected-root OverlayFS {name} is not UTF-8: {}",
                path.display()
            ))
        })?;
        if value
            .bytes()
            .any(|byte| matches!(byte, b',' | b':' | b'\\' | b'\n' | b'\r'))
        {
            return Err(crate::Error::InvalidPath(format!(
                "selected-root OverlayFS {name} contains a mount-option delimiter: {}",
                path.display()
            )));
        }
        components.push(format!("{name}={value}"));
    }
    components.extend(profile.mount_options()?.into_iter().map(str::to_string));
    Ok(components.join(","))
}

/// One mounted selected-root OverlayFS view.
///
/// A successful `freeze` is a strict synchronization and unmount boundary.
/// `Drop` uses lazy detach only as crash-tail containment; callers may not
/// decode or publish an upper unless strict unmount succeeded.
pub struct MountedSelectedRootOverlay {
    target: PathBuf,
    mounted: bool,
}

impl MountedSelectedRootOverlay {
    pub fn mount(
        lower: &Path,
        upper: &Path,
        work: &Path,
        target: &Path,
        profile: &SelectedRootOverlayProfile,
    ) -> crate::Result<Self> {
        let options = mount_data(lower, upper, work, profile)?;
        mount(
            Some("overlay"),
            target,
            Some("overlay"),
            MsFlags::empty(),
            Some(options.as_str()),
        )
        .map_err(|error| {
            crate::Error::NotImplemented(format!(
                "selected-root OverlayFS mount failed at {}: {error}",
                target.display()
            ))
        })?;
        Ok(Self {
            target: target.to_path_buf(),
            mounted: true,
        })
    }

    pub fn freeze(mut self, upper: &Path) -> crate::Result<()> {
        sync_filesystem(upper)?;
        umount2(&self.target, MntFlags::empty()).map_err(|error| {
            crate::Error::IoError(format!(
                "failed to strictly unmount selected-root OverlayFS {}: {error}",
                self.target.display()
            ))
        })?;
        self.mounted = false;
        Ok(())
    }

    pub fn unmount(mut self) -> crate::Result<()> {
        umount2(&self.target, MntFlags::empty()).map_err(|error| {
            crate::Error::IoError(format!(
                "failed to unmount selected-root OverlayFS {}: {error}",
                self.target.display()
            ))
        })?;
        self.mounted = false;
        Ok(())
    }
}

impl Drop for MountedSelectedRootOverlay {
    fn drop(&mut self) {
        if self.mounted {
            let _ = umount2(&self.target, MntFlags::MNT_DETACH);
        }
    }
}

fn prove_hardlink_copy_up(
    merged: &Path,
    upper: &Path,
    profile: &SelectedRootOverlayProfile,
) -> crate::Result<()> {
    let changed = merged.join("hardlink-a");
    fs::set_permissions(&changed, fs::Permissions::from_mode(0o600))?;
    let changed_metadata = fs::symlink_metadata(&changed)?;
    let alias_metadata = fs::symlink_metadata(merged.join("hardlink-b"))?;
    if changed_metadata.dev() != alias_metadata.dev()
        || changed_metadata.ino() != alias_metadata.ino()
        || changed_metadata.mode() & 0o7777 != 0o600
        || alias_metadata.mode() & 0o7777 != 0o600
    {
        return Err(crate::Error::NotImplemented(
            "selected-root OverlayFS index did not preserve a copied-up hardlink group".to_string(),
        ));
    }
    let upper_path = upper.join("hardlink-a");
    if fs::read(&upper_path)? != b"hardlink-content" {
        return Err(crate::Error::NotImplemented(
            "selected-root OverlayFS metadata operation did not perform complete data copy-up"
                .to_string(),
        ));
    }
    if private_xattr(&upper_path, profile, "metacopy")?.is_some() {
        return Err(crate::Error::NotImplemented(
            "selected-root OverlayFS emitted metacopy despite metacopy=off".to_string(),
        ));
    }
    if private_xattr(&upper_path, profile, "origin")?.is_none() {
        return Err(crate::Error::NotImplemented(
            "selected-root OverlayFS index did not emit copy-up origin authority".to_string(),
        ));
    }
    Ok(())
}

fn prove_whiteout(
    merged: &Path,
    upper: &Path,
    profile: &SelectedRootOverlayProfile,
) -> crate::Result<OverlayWhiteoutEncoding> {
    fs::remove_file(merged.join("remove-me"))?;
    let whiteout = upper.join("remove-me");
    let metadata = fs::symlink_metadata(&whiteout)?;
    if metadata.file_type().is_char_device() && metadata.rdev() == 0 {
        return Ok(OverlayWhiteoutEncoding::CharacterDeviceZeroZero);
    }
    if metadata.file_type().is_file()
        && metadata.len() == 0
        && private_xattr(&whiteout, profile, "whiteout")?.is_some()
    {
        return Ok(OverlayWhiteoutEncoding::ZeroSizeRegularXattr);
    }
    Err(crate::Error::NotImplemented(
        "selected-root OverlayFS emitted an unknown whiteout representation".to_string(),
    ))
}

fn prove_opaque_replacement(
    merged: &Path,
    upper: &Path,
    profile: &SelectedRootOverlayProfile,
) -> crate::Result<()> {
    fs::remove_dir_all(merged.join("replace-me"))?;
    fs::create_dir(merged.join("replace-me"))?;
    let value = private_xattr(&upper.join("replace-me"), profile, "opaque")?;
    if value.as_deref() != Some(b"y".as_slice()) {
        return Err(crate::Error::NotImplemented(
            "selected-root OverlayFS did not encode lower-directory replacement as opaque=y"
                .to_string(),
        ));
    }
    Ok(())
}

fn prove_redirect_disabled(merged: &Path) -> crate::Result<()> {
    let error = match fs::rename(merged.join("rename-me"), merged.join("renamed")) {
        Ok(()) => {
            return Err(crate::Error::NotImplemented(
                "selected-root OverlayFS created a redirect despite redirect_dir=nofollow"
                    .to_string(),
            ));
        }
        Err(error) => error,
    };
    if error.raw_os_error() != Some(libc::EXDEV) {
        return Err(crate::Error::NotImplemented(format!(
            "selected-root OverlayFS lower-directory rename returned {error}; expected EXDEV"
        )));
    }
    Ok(())
}

fn private_xattr(
    path: &Path,
    profile: &SelectedRootOverlayProfile,
    marker: &str,
) -> crate::Result<Option<Vec<u8>>> {
    let namespace = match profile.xattr_namespace {
        OverlayXattrNamespace::Trusted => "trusted.overlay.",
        OverlayXattrNamespace::User => "user.overlay.",
    };
    xattr::get(path, format!("{namespace}{marker}")).map_err(Into::into)
}

fn sync_filesystem(path: &Path) -> crate::Result<()> {
    let directory = fs::File::open(path)?;
    let result = unsafe { libc::syncfs(directory.as_raw_fd()) };
    if result != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

struct MountedOverlay {
    target: PathBuf,
    mounted: bool,
}

impl MountedOverlay {
    fn new(target: PathBuf) -> Self {
        Self {
            target,
            mounted: true,
        }
    }

    fn unmount(mut self) -> crate::Result<()> {
        umount2(&self.target, MntFlags::empty()).map_err(|error| {
            crate::Error::IoError(format!(
                "failed to unmount selected-root OverlayFS probe {}: {error}",
                self.target.display()
            ))
        })?;
        self.mounted = false;
        Ok(())
    }
}

impl Drop for MountedOverlay {
    fn drop(&mut self) {
        if self.mounted {
            let _ = umount2(&self.target, MntFlags::MNT_DETACH);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_data_is_exact_and_rejects_delimiter_paths() {
        let profile = SelectedRootOverlayProfile {
            xattr_namespace: OverlayXattrNamespace::User,
            ..SelectedRootOverlayProfile::trusted()
        };
        assert_eq!(
            mount_data(
                Path::new("/probe/lower"),
                Path::new("/probe/upper"),
                Path::new("/probe/work"),
                &profile,
            )
            .unwrap(),
            "lowerdir=/probe/lower,upperdir=/probe/upper,workdir=/probe/work,index=on,redirect_dir=nofollow,metacopy=off,xino=off,nfs_export=off,verity=off,userxattr"
        );
        assert!(
            mount_data(
                Path::new("/probe/with,comma"),
                Path::new("/probe/upper"),
                Path::new("/probe/work"),
                &profile,
            )
            .is_err()
        );
    }

    #[test]
    #[ignore = "requires CAP_SYS_ADMIN and an OverlayFS-capable kernel"]
    fn functionally_probes_selected_root_overlay_profile() {
        let workspace = tempfile::tempdir().unwrap();
        let capabilities = probe_selected_root_overlay_profile(
            workspace.path(),
            &SelectedRootOverlayProfile::trusted(),
        )
        .unwrap();
        assert_eq!(
            capabilities.hardlink_copy_up,
            OverlayHardlinkCopyUp::Preserved
        );
        assert_eq!(
            capabilities.lower_directory_rename,
            OverlayLowerDirectoryRename::CrossDevice
        );
        assert_eq!(
            capabilities.metadata_copy_up,
            OverlayMetadataCopyUp::CompleteData
        );
    }
}
