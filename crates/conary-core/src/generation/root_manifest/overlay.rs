// conary-core/src/generation/root_manifest/overlay.rs

//! OverlayFS upper-tree decoding for delta-sized selected-root transactions.

mod config_state;
mod indexed;
mod probe;

pub use config_state::decode_config_state_upper_indexed;
pub use indexed::decode_selected_root_overlay_upper_indexed;
pub use probe::{
    MountedSelectedRootOverlay, OverlayHardlinkCopyUp, OverlayLowerDirectoryRename,
    OverlayMetadataCopyUp, OverlayOpaqueDirectory, OverlayWhiteoutEncoding,
    SELECTED_ROOT_OVERLAY_CAPABILITIES_VERSION, SelectedRootOverlayCapabilities,
    probe_selected_root_overlay_profile,
};

#[cfg(test)]
use super::CapturedSelectedRoot;
#[cfg(test)]
use super::SelectedRootSnapshot;
#[cfg(test)]
use super::scan::scan_selected_root_overlay_upper;
use super::{GenerationRootEntry, SELECTED_ROOT_MANIFEST_DELTA_VERSION, SelectedRootManifestDelta};
#[cfg(test)]
use crate::filesystem::PrivateCasWriter;
use crate::payload::{PayloadNodeKind, ResolvedPayloadNode};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
#[cfg(test)]
use std::path::Path;

pub const SELECTED_ROOT_OVERLAY_PROFILE_VERSION: u32 = 1;

/// Namespace used by one transaction-owned OverlayFS mount.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OverlayXattrNamespace {
    Trusted,
    User,
}

/// Exact OverlayFS behavior admitted by the selected-root delta decoder.
///
/// The runtime must functionally prove this profile on the actual lower and
/// upper filesystems before lifecycle mutation. Kernel versions and module
/// defaults are not capability authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedRootOverlayProfile {
    pub version: u32,
    pub xattr_namespace: OverlayXattrNamespace,
    pub index: bool,
    pub redirect_dir: bool,
    pub metacopy: bool,
    pub xino: bool,
    pub nfs_export: bool,
    pub verity: bool,
}

impl SelectedRootOverlayProfile {
    /// Privileged profile used for selected-root transactions.
    pub fn trusted() -> Self {
        Self {
            version: SELECTED_ROOT_OVERLAY_PROFILE_VERSION,
            xattr_namespace: OverlayXattrNamespace::Trusted,
            index: true,
            redirect_dir: false,
            metacopy: false,
            xino: false,
            nfs_export: false,
            verity: false,
        }
    }

    /// Exact options whose mounted behavior must be checked by preflight.
    pub fn mount_options(&self) -> crate::Result<Vec<&'static str>> {
        self.validate()?;
        let mut options = vec![
            "index=on",
            "redirect_dir=nofollow",
            "metacopy=off",
            "xino=off",
            "nfs_export=off",
            "verity=off",
        ];
        if self.xattr_namespace == OverlayXattrNamespace::User {
            options.push("userxattr");
        }
        Ok(options)
    }

    pub fn validate(&self) -> crate::Result<()> {
        if self.version != SELECTED_ROOT_OVERLAY_PROFILE_VERSION {
            return Err(crate::Error::InvalidPath(format!(
                "selected-root overlay profile has unsupported version {}; expected {}",
                self.version, SELECTED_ROOT_OVERLAY_PROFILE_VERSION
            )));
        }
        if !self.index
            || self.redirect_dir
            || self.metacopy
            || self.xino
            || self.nfs_export
            || self.verity
        {
            return Err(crate::Error::NotImplemented(
                "selected-root overlay profile must use index=on, redirect_dir=nofollow, metacopy=off, xino=off, nfs_export=off, and verity=off"
                    .to_string(),
            ));
        }
        Ok(())
    }

    fn private_prefix(&self) -> &'static str {
        match self.xattr_namespace {
            OverlayXattrNamespace::Trusted => "trusted.overlay.",
            OverlayXattrNamespace::User => "user.overlay.",
        }
    }
}

/// Encode payload xattrs on the upper root so OverlayFS does not interpret a
/// payload-owned name in its private namespace as transaction bookkeeping.
pub fn encode_selected_root_overlay_upper_node(
    node: &ResolvedPayloadNode,
    profile: &SelectedRootOverlayProfile,
) -> crate::Result<ResolvedPayloadNode> {
    profile.validate()?;
    let prefix = profile.private_prefix();
    let mut encoded = node.clone();
    let mut xattrs = BTreeMap::new();
    for (name, value) in &node.source.xattrs {
        let encoded_name = if let Some(suffix) = name.strip_prefix(prefix) {
            format!("{prefix}overlay.{suffix}")
        } else {
            name.clone()
        };
        if xattrs.insert(encoded_name.clone(), value.clone()).is_some() {
            return Err(crate::Error::InvalidPath(format!(
                "overlay upper-root xattr escaping produces duplicate {encoded_name}"
            )));
        }
    }
    encoded.source.xattrs = xattrs;
    Ok(encoded)
}

#[derive(Debug, Default)]
struct OverlayMarkers {
    whiteout: bool,
    opaque: Option<OpaqueMarker>,
    origin: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpaqueMarker {
    Logical,
    WhiteoutScan,
}

pub(super) struct DecodedUpper {
    root: ResolvedPayloadNode,
    removals: Vec<String>,
    opaque_directories: Vec<String>,
    upserts: BTreeMap<String, GenerationRootEntry>,
    copied_up_origins: BTreeSet<String>,
}

pub(super) struct PendingDelta {
    pub(super) root: Option<ResolvedPayloadNode>,
    pub(super) removals: Vec<String>,
    pub(super) opaque_directories: Vec<String>,
    pub(super) upserts: BTreeMap<String, GenerationRootEntry>,
    pub(super) copied_up_origins: BTreeSet<String>,
}

impl DecodedUpper {
    pub(super) fn into_delta(self, prior_root: &ResolvedPayloadNode) -> PendingDelta {
        PendingDelta {
            root: (self.root != *prior_root).then_some(self.root),
            removals: self.removals,
            opaque_directories: self.opaque_directories,
            upserts: self.upserts,
            copied_up_origins: self.copied_up_origins,
        }
    }
}

impl PendingDelta {
    pub(super) fn finish(self) -> crate::Result<SelectedRootManifestDelta> {
        let delta = SelectedRootManifestDelta {
            version: SELECTED_ROOT_MANIFEST_DELTA_VERSION,
            root: self.root,
            removals: self.removals,
            opaque_directories: self.opaque_directories,
            upserts: self.upserts.into_values().collect(),
        };
        delta.validate()?;
        Ok(delta)
    }
}

pub(super) fn decode_upper_operations(
    mut root: ResolvedPayloadNode,
    entries: Vec<GenerationRootEntry>,
    profile: &SelectedRootOverlayProfile,
    mut prior_directory_exists: impl FnMut(&str) -> crate::Result<bool>,
) -> crate::Result<DecodedUpper> {
    let root_markers = decode_overlay_xattrs("/", &mut root, profile)?;
    if root_markers.whiteout || root_markers.opaque.is_some() {
        return Err(crate::Error::InvalidPath(
            "selected-root overlay upper root contains transaction markers that are not representable as root metadata"
                .to_string(),
        ));
    }

    let mut removals = Vec::new();
    let mut opaque_directories = Vec::new();
    let mut upserts = BTreeMap::new();
    let mut copied_up_origins = BTreeSet::new();

    for mut entry in entries {
        let markers = decode_overlay_xattrs(&entry.path, &mut entry.node, profile)?;
        if is_whiteout(&entry, markers.whiteout)? {
            if markers.opaque.is_some() || markers.origin {
                return Err(crate::Error::InvalidPath(format!(
                    "overlay whiteout {} carries incompatible private markers",
                    entry.path
                )));
            }
            removals.push(entry.path);
            continue;
        }
        if markers.whiteout {
            return Err(crate::Error::InvalidPath(format!(
                "overlay xattr whiteout {} is not a zero-size regular file",
                entry.path
            )));
        }
        if let Some(opaque) = markers.opaque {
            if !matches!(entry.node.source.kind, PayloadNodeKind::Directory) {
                return Err(crate::Error::InvalidPath(format!(
                    "overlay opaque marker is attached to non-directory {}",
                    entry.path
                )));
            }
            if opaque == OpaqueMarker::Logical && prior_directory_exists(&entry.path)? {
                opaque_directories.push(entry.path.clone());
            }
        }
        if markers.origin {
            copied_up_origins.insert(entry.path.clone());
        }
        upserts.insert(entry.path.clone(), entry);
    }

    normalize_removals(&mut removals);
    Ok(DecodedUpper {
        root,
        removals,
        opaque_directories,
        upserts,
        copied_up_origins,
    })
}

fn decode_overlay_xattrs(
    path: &str,
    node: &mut ResolvedPayloadNode,
    profile: &SelectedRootOverlayProfile,
) -> crate::Result<OverlayMarkers> {
    let prefix = profile.private_prefix();
    let escaped_prefix = format!("{prefix}overlay.");
    let mut decoded = BTreeMap::new();
    let mut markers = OverlayMarkers::default();

    for (name, value) in std::mem::take(&mut node.source.xattrs) {
        if let Some(suffix) = name.strip_prefix(&escaped_prefix) {
            insert_payload_xattr(path, &mut decoded, format!("{prefix}{suffix}"), value)?;
            continue;
        }
        let Some(marker) = name.strip_prefix(prefix) else {
            insert_payload_xattr(path, &mut decoded, name, value)?;
            continue;
        };
        match marker {
            "whiteout" => markers.whiteout = true,
            "opaque" => {
                markers.opaque = Some(match value.as_slice() {
                    b"y" => OpaqueMarker::Logical,
                    b"x" => OpaqueMarker::WhiteoutScan,
                    _ => {
                        return Err(crate::Error::InvalidPath(format!(
                            "overlay opaque marker on {path} has unknown value"
                        )));
                    }
                });
            }
            "origin" => markers.origin = true,
            // These are OverlayFS bookkeeping only and do not describe a
            // payload node. Their semantics are constrained by the profile.
            "impure" | "nlink" | "upper" | "uuid" => {}
            "redirect" => {
                return Err(crate::Error::NotImplemented(format!(
                    "overlay redirect marker on {path} is forbidden by the selected-root profile"
                )));
            }
            "metacopy" => {
                return Err(crate::Error::NotImplemented(format!(
                    "overlay metacopy marker on {path} is forbidden by the selected-root profile"
                )));
            }
            "protattr" => {
                return Err(crate::Error::NotImplemented(format!(
                    "overlay protected inode attributes on {path} are not represented by PayloadNode"
                )));
            }
            unknown => {
                return Err(crate::Error::NotImplemented(format!(
                    "unknown overlay private xattr {prefix}{unknown} on {path}"
                )));
            }
        }
    }
    node.source.xattrs = decoded;
    Ok(markers)
}

fn insert_payload_xattr(
    path: &str,
    decoded: &mut BTreeMap<String, Vec<u8>>,
    name: String,
    value: Vec<u8>,
) -> crate::Result<()> {
    if decoded.insert(name.clone(), value).is_some() {
        return Err(crate::Error::InvalidPath(format!(
            "overlay xattr escaping produces duplicate payload xattr {name} on {path}"
        )));
    }
    Ok(())
}

fn is_whiteout(entry: &GenerationRootEntry, xattr_whiteout: bool) -> crate::Result<bool> {
    if matches!(
        entry.node.source.kind,
        PayloadNodeKind::CharacterDevice { major: 0, minor: 0 }
    ) {
        return Ok(true);
    }
    if !xattr_whiteout {
        return Ok(false);
    }
    Ok(
        matches!(entry.node.source.kind, PayloadNodeKind::Regular { .. })
            && entry
                .content
                .as_ref()
                .is_some_and(|content| content.size == 0),
    )
}

fn normalize_removals(removals: &mut Vec<String>) {
    removals.sort();
    removals.dedup();
    let mut normalized = Vec::<String>::new();
    for path in removals.drain(..) {
        if normalized
            .iter()
            .any(|ancestor| is_path_or_descendant(&path, ancestor))
        {
            continue;
        }
        normalized.push(path);
    }
    *removals = normalized;
}

pub(super) fn expand_prior_hardlink_groups(
    groups: Vec<Vec<GenerationRootEntry>>,
    removals: &[String],
    opaque_directories: &[String],
    copied_up_origins: &BTreeSet<String>,
    upserts: &mut BTreeMap<String, GenerationRootEntry>,
) -> crate::Result<()> {
    for mut members in groups {
        let identity = members
            .first()
            .and_then(entry_hardlink_identity)
            .ok_or_else(|| {
                crate::Error::InvalidPath(
                    "indexed selected-root hardlink group has no typed identity".to_string(),
                )
            })?
            .to_string();
        members.sort_by(|left, right| left.path.cmp(&right.path));
        let copied_prior_paths = members
            .iter()
            .filter(|member| copied_up_origins.contains(&member.path))
            .map(|member| member.path.as_str())
            .collect::<Vec<_>>();
        let upper_identities = copied_prior_paths
            .iter()
            .filter_map(|path| upserts.get(*path).and_then(entry_hardlink_identity))
            .collect::<BTreeSet<_>>();
        if upper_identities.len() > 1
            || (copied_prior_paths.len() > 1 && upper_identities.is_empty())
        {
            return Err(crate::Error::InvalidPath(format!(
                "OverlayFS index did not preserve copied-up prior hardlink group {identity}"
            )));
        }
        let upper_identity = upper_identities.first().copied();
        let upper_group_paths = if let Some(upper_identity) = upper_identity {
            upserts
                .values()
                .filter(|entry| entry_hardlink_identity(entry) == Some(upper_identity))
                .map(|entry| entry.path.clone())
                .collect::<BTreeSet<_>>()
        } else {
            copied_prior_paths
                .iter()
                .map(|path| (*path).to_string())
                .collect::<BTreeSet<_>>()
        };

        let mut membership_changed = false;
        let mut member_paths = BTreeSet::new();
        for member in &members {
            if path_removed(&member.path, removals, opaque_directories) {
                membership_changed = true;
                continue;
            }
            if upserts.contains_key(&member.path) && !upper_group_paths.contains(&member.path) {
                // Replacing one name with a new inode intentionally breaks
                // that name out of the prior group. The remaining names still
                // need a valid primary if the replaced path owned it.
                membership_changed = true;
                continue;
            }
            member_paths.insert(member.path.clone());
        }
        for path in &upper_group_paths {
            if !members.iter().any(|member| member.path == *path) {
                membership_changed = true;
            }
            if !path_removed(path, removals, opaque_directories) {
                member_paths.insert(path.clone());
            }
        }
        let copied_up = !copied_prior_paths.is_empty();
        if !copied_up && !membership_changed {
            continue;
        }
        if member_paths.is_empty() {
            continue;
        }
        let source = if copied_up {
            upper_group_paths
                .iter()
                .filter_map(|path| upserts.get(path))
                .find(|entry| entry.content.is_some())
                .cloned()
                .ok_or_else(|| {
                    crate::Error::InvalidPath(format!(
                        "copied-up hardlink group {identity} lacks complete content authority"
                    ))
                })?
        } else {
            members
                .iter()
                .find(|member| member.content.is_some())
                .cloned()
                .ok_or_else(|| {
                    crate::Error::InvalidPath(format!(
                        "prior hardlink group {identity} has no content primary"
                    ))
                })?
        };
        let content = source.content.clone().ok_or_else(|| {
            crate::Error::InvalidPath(format!(
                "hardlink group {identity} lacks complete content authority"
            ))
        })?;
        let primary_path = member_paths.first().cloned().ok_or_else(|| {
            crate::Error::InvalidPath(format!("hardlink group {identity} has no surviving member"))
        })?;
        let has_aliases = member_paths.len() > 1;
        let mut primary_node = source.node.clone();
        primary_node.source.kind = PayloadNodeKind::Regular {
            hardlink_identity: has_aliases.then(|| identity.clone()),
        };
        upserts.insert(
            primary_path.clone(),
            GenerationRootEntry {
                path: primary_path.clone(),
                node: primary_node.clone(),
                content: Some(content),
            },
        );
        for path in member_paths.into_iter().skip(1) {
            let mut node = primary_node.clone();
            node.source.kind = PayloadNodeKind::Hardlink {
                target: primary_path.clone(),
                identity: identity.clone(),
            };
            upserts.insert(
                path.clone(),
                GenerationRootEntry {
                    path,
                    node,
                    content: None,
                },
            );
        }
    }
    Ok(())
}

pub(super) fn entry_hardlink_identity(entry: &GenerationRootEntry) -> Option<&str> {
    match &entry.node.source.kind {
        PayloadNodeKind::Regular {
            hardlink_identity: Some(identity),
        }
        | PayloadNodeKind::Hardlink { identity, .. } => Some(identity),
        _ => None,
    }
}

fn path_removed(path: &str, removals: &[String], opaque_directories: &[String]) -> bool {
    removals
        .iter()
        .any(|removed| is_path_or_descendant(path, removed))
        || opaque_directories
            .iter()
            .any(|opaque| path != opaque && is_path_or_descendant(path, opaque))
}

fn is_path_or_descendant(candidate: &str, ancestor: &str) -> bool {
    candidate == ancestor
        || candidate
            .strip_prefix(ancestor)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

#[cfg(test)]
mod tests;
