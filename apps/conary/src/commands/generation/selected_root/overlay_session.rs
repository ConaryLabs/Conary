// apps/conary/src/commands/generation/selected_root/overlay_session.rs

//! Transaction-owned OverlayFS lifetime for a selected-root session.

use anyhow::{Context, Result};
use conary_core::filesystem::CasStore;
use conary_core::generation::root_manifest::{
    CapturedSelectedRoot, MountedSelectedRootOverlay, SelectedRootManifestDelta,
    SelectedRootOverlayCapabilities, SelectedRootOverlayProfile, apply_resolved_payload_metadata,
    decode_selected_root_overlay_upper, encode_selected_root_overlay_upper_node,
    probe_selected_root_overlay_profile,
};
use std::fs;
use std::path::{Path, PathBuf};

/// A mounted merged root whose only writable authority is `upper`.
pub(super) struct SelectedRootOverlaySession {
    selected_root: PathBuf,
    upper: PathBuf,
    mounted: Option<MountedSelectedRootOverlay>,
    capabilities: SelectedRootOverlayCapabilities,
}

impl SelectedRootOverlaySession {
    /// Probe the actual workspace, admit the materialized immutable lower, and
    /// mount the exact profile before lifecycle mutation can begin.
    pub(super) fn begin(session_dir: &Path, prior: &CapturedSelectedRoot) -> Result<Self> {
        fs::create_dir_all(session_dir).with_context(|| {
            format!(
                "failed to create selected-root overlay workspace {}",
                session_dir.display()
            )
        })?;
        let profile = SelectedRootOverlayProfile::trusted();
        let capabilities = probe_selected_root_overlay_profile(session_dir, &profile)
            .context("selected-root OverlayFS capability preflight failed")?;

        let lower = session_dir.join("lower");
        let upper = session_dir.join("upper");
        let work = session_dir.join("work");
        let selected_root = session_dir.join("root");
        if !lower.is_dir() {
            anyhow::bail!(
                "selected-root immutable lower authority is missing at {}",
                lower.display()
            );
        }
        for directory in [&upper, &work, &selected_root] {
            fs::create_dir(directory).with_context(|| {
                format!(
                    "failed to create selected-root overlay directory {}",
                    directory.display()
                )
            })?;
        }
        // The upper directory itself represents the merged root node. Seeding
        // it prevents an unchanged root from becoming a synthetic delta.
        let upper_root = encode_selected_root_overlay_upper_node(&prior.generation.root, &profile)
            .context("failed to encode selected-root overlay root metadata")?;
        apply_resolved_payload_metadata(&upper, &upper_root)
            .context("failed to seed selected-root overlay root metadata")?;
        let mounted =
            MountedSelectedRootOverlay::mount(&lower, &upper, &work, &selected_root, &profile)
                .context("failed to mount selected-root OverlayFS session")?;

        Ok(Self {
            selected_root,
            upper,
            mounted: Some(mounted),
            capabilities,
        })
    }

    pub(super) fn selected_root(&self) -> &Path {
        &self.selected_root
    }

    /// Freeze the upper with a strict unmount, then decode changed paths only.
    pub(super) fn freeze_and_decode(
        &mut self,
        prior: &CapturedSelectedRoot,
        cas: &CasStore,
    ) -> Result<SelectedRootManifestDelta> {
        self.mounted
            .take()
            .context("selected-root OverlayFS session is already frozen")?
            .freeze(&self.upper)
            .context("failed to freeze selected-root OverlayFS session")?;
        decode_selected_root_overlay_upper(&self.upper, prior, cas, &self.capabilities.profile)
            .context("failed to decode selected-root OverlayFS upper")
    }

    /// Strictly unmount without decoding; the caller then discards the
    /// transaction directory and the prior manifest remains authoritative.
    pub(super) fn unmount_for_discard(&mut self) -> Result<()> {
        if let Some(mounted) = self.mounted.take() {
            mounted
                .unmount()
                .context("failed to unmount discarded selected-root OverlayFS session")?;
        }
        Ok(())
    }
}
