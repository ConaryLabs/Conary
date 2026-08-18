// conary-core/src/generation/root_manifest/overlay/config_state.rs

//! Generation config-upper decoding into indexed root authority.

use super::{
    SelectedRootOverlayProfile, decode_upper_operations, expand_prior_hardlink_groups, indexed,
};
use crate::filesystem::PrivateCasWriter;
use crate::generation::root_manifest::{
    GenerationRootEntry, PayloadNodeKind, SelectedRootManifestDelta, SelectedRootSnapshot,
};
use std::path::Path;

/// Decode one generation's sparse `/etc` upper into selected-root authority.
///
/// Generation config uppers are seeded from the complete mutable-state
/// manifest before typed config mutations reduce pristine nodes to the
/// generation lower. User deletions are retained as OverlayFS whiteouts. This
/// boundary projects that exact sparse tree beneath `/etc`; absence alone is
/// never interpreted as deletion.
pub fn decode_config_state_upper_indexed(
    upper: &Path,
    hardlink_namespace: &str,
    conn: &rusqlite::Connection,
    prior: SelectedRootSnapshot,
    cas: &dyn PrivateCasWriter,
    profile: &SelectedRootOverlayProfile,
) -> crate::Result<SelectedRootManifestDelta> {
    profile.validate()?;
    let prior_root = prior.root(conn)?;
    let (config_root, entries) =
        crate::generation::root_manifest::scan_payload_tree(upper, cas, hardlink_namespace)?;

    let mut projected = Vec::with_capacity(entries.len() + 1);
    projected.push(GenerationRootEntry {
        path: "/etc".to_string(),
        node: config_root,
        content: None,
    });
    for mut entry in entries {
        entry.path = format!("/etc{}", entry.path);
        if let PayloadNodeKind::Hardlink { target, identity } = &entry.node.source.kind {
            entry.node.source.kind = PayloadNodeKind::Hardlink {
                target: format!("/etc{target}"),
                identity: identity.clone(),
            };
        }
        projected.push(entry);
    }
    projected.sort_by(|left, right| left.path.cmp(&right.path));

    let decoded = decode_upper_operations(prior_root.clone(), projected, profile, |path| {
        Ok(prior
            .entry(conn, path)?
            .is_some_and(|entry| matches!(entry.node.source.kind, PayloadNodeKind::Directory)))
    })?;
    let mut delta = decoded.into_delta(&prior_root);
    let groups = indexed::affected_hardlink_groups(
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
