// conary-core/src/generation/root_manifest/overlay/indexed.rs

//! SQLite-indexed prior-authority resolution for OverlayFS upper decoding.

use super::{
    SelectedRootOverlayProfile, decode_upper_operations, entry_hardlink_identity,
    expand_prior_hardlink_groups,
};
use crate::filesystem::PrivateCasWriter;
use crate::generation::root_manifest::scan::scan_selected_root_overlay_upper;
use crate::generation::root_manifest::{
    GenerationRootEntry, PayloadNodeKind, SelectedRootManifestDelta, SelectedRootSnapshot,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Decode a frozen upper against indexed snapshot authority.
///
/// Only changed paths, changed subtrees, and affected hardlink groups are
/// resolved from the parent. The returned delta is structurally validated;
/// callers must apply it to `prior` to perform exact incremental validation
/// and persist the child snapshot.
pub fn decode_selected_root_overlay_upper_indexed(
    upper: &Path,
    conn: &rusqlite::Connection,
    prior: SelectedRootSnapshot,
    cas: &dyn PrivateCasWriter,
    profile: &SelectedRootOverlayProfile,
) -> crate::Result<SelectedRootManifestDelta> {
    profile.validate()?;
    let prior_root = prior.root(conn)?;
    let (root, entries) = scan_selected_root_overlay_upper(upper, cas, "selected-root-overlay-v1")?;
    let decoded = decode_upper_operations(root, entries, profile, |path| {
        Ok(prior
            .entry(conn, path)?
            .is_some_and(|entry| matches!(entry.node.source.kind, PayloadNodeKind::Directory)))
    })?;
    let mut delta = decoded.into_delta(&prior_root);
    let groups = affected_hardlink_groups(
        conn,
        prior,
        &delta.removals,
        &delta.opaque_directories,
        &delta.copied_up_origins,
        &delta.upserts,
    )?;
    expand_prior_hardlink_groups(
        groups,
        &delta.removals,
        &delta.opaque_directories,
        &delta.copied_up_origins,
        &mut delta.upserts,
    )?;
    delta.finish()
}

pub(super) fn affected_hardlink_groups(
    conn: &rusqlite::Connection,
    prior: SelectedRootSnapshot,
    removals: &[String],
    opaque_directories: &[String],
    copied_up_origins: &BTreeSet<String>,
    upserts: &BTreeMap<String, GenerationRootEntry>,
) -> crate::Result<Vec<Vec<GenerationRootEntry>>> {
    let mut identities = BTreeSet::new();
    for path in copied_up_origins.iter().chain(upserts.keys()) {
        if let Some(entry) = prior.entry(conn, path)?
            && let Some(identity) = entry_hardlink_identity(&entry)
        {
            identities.insert(identity.to_string());
        }
    }
    for path in removals.iter().chain(opaque_directories) {
        for entry in prior.subtree(conn, path)? {
            if let Some(identity) = entry_hardlink_identity(&entry) {
                identities.insert(identity.to_string());
            }
        }
    }
    identities
        .into_iter()
        .map(|identity| prior.hardlink_group(conn, &identity))
        .collect()
}
