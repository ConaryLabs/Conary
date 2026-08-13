// conary-core/src/generation/root_manifest/overlay.rs

//! OverlayFS upper-tree decoding for delta-sized selected-root transactions.

mod probe;

pub use probe::{
    OverlayHardlinkCopyUp, OverlayLowerDirectoryRename, OverlayMetadataCopyUp,
    OverlayOpaqueDirectory, OverlayWhiteoutEncoding, SELECTED_ROOT_OVERLAY_CAPABILITIES_VERSION,
    SelectedRootOverlayCapabilities, probe_selected_root_overlay_profile,
};

use super::{
    CapturedSelectedRoot, GenerationRootEntry, SELECTED_ROOT_MANIFEST_DELTA_VERSION,
    SelectedRootManifestDelta, scan_payload_tree,
};
use crate::filesystem::PrivateCasWriter;
use crate::payload::{PayloadNodeKind, ResolvedPayloadNode};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
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

/// Capture and decode one frozen transaction upper without reading unchanged
/// lower content.
///
/// Callers seed the upper root metadata from `prior` before mounting. A root
/// metadata difference can then be represented without guessing whether it
/// was transaction-owned.
pub fn decode_selected_root_overlay_upper(
    upper: &Path,
    prior: &CapturedSelectedRoot,
    cas: &dyn PrivateCasWriter,
    profile: &SelectedRootOverlayProfile,
) -> crate::Result<SelectedRootManifestDelta> {
    profile.validate()?;
    prior.generation.validate()?;
    prior.state.validate()?;
    let (root, entries) = scan_payload_tree(upper, cas, "selected-root-overlay-v1")?;
    decode_captured_upper(root, entries, prior, profile)
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

fn decode_captured_upper(
    mut root: ResolvedPayloadNode,
    entries: Vec<GenerationRootEntry>,
    prior: &CapturedSelectedRoot,
    profile: &SelectedRootOverlayProfile,
) -> crate::Result<SelectedRootManifestDelta> {
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
            if opaque == OpaqueMarker::Logical {
                opaque_directories.push(entry.path.clone());
            }
        }
        if markers.origin {
            copied_up_origins.insert(entry.path.clone());
        }
        upserts.insert(entry.path.clone(), entry);
    }

    normalize_removals(&mut removals);
    expand_prior_hardlink_authority(
        prior,
        &removals,
        &opaque_directories,
        &copied_up_origins,
        &mut upserts,
    )?;

    let delta = SelectedRootManifestDelta {
        version: SELECTED_ROOT_MANIFEST_DELTA_VERSION,
        root: (root != prior.generation.root).then_some(root),
        removals,
        opaque_directories,
        upserts: upserts.into_values().collect(),
    };
    delta.validate()?;
    // Validate the complete resulting authority now, before a caller can
    // persist this changed-path representation.
    delta.apply(prior)?;
    Ok(delta)
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

fn expand_prior_hardlink_authority(
    prior: &CapturedSelectedRoot,
    removals: &[String],
    opaque_directories: &[String],
    copied_up_origins: &BTreeSet<String>,
    upserts: &mut BTreeMap<String, GenerationRootEntry>,
) -> crate::Result<()> {
    let prior_entries = prior
        .generation
        .entries
        .iter()
        .chain(&prior.state.entries)
        .map(|entry| (entry.path.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    let mut groups = BTreeMap::<String, Vec<&GenerationRootEntry>>::new();
    for entry in prior_entries.values() {
        let identity = match &entry.node.source.kind {
            PayloadNodeKind::Regular {
                hardlink_identity: Some(identity),
            }
            | PayloadNodeKind::Hardlink { identity, .. } => identity,
            _ => continue,
        };
        groups.entry(identity.clone()).or_default().push(entry);
    }

    for (identity, mut members) in groups {
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
                .map(|entry| (*entry).clone())
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

fn entry_hardlink_identity(entry: &GenerationRootEntry) -> Option<&str> {
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
mod tests {
    use super::*;
    use crate::filesystem::CasStore;
    use crate::payload::{PayloadContentAuthority, PayloadIdentity, PayloadNode};
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn profile_is_explicit_and_rejects_hardlink_breaking_index_off() {
        let profile = SelectedRootOverlayProfile::trusted();
        assert_eq!(
            profile.mount_options().unwrap(),
            [
                "index=on",
                "redirect_dir=nofollow",
                "metacopy=off",
                "xino=off",
                "nfs_export=off",
                "verity=off",
            ]
        );
        let mut invalid = profile;
        invalid.index = false;
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn decodes_documented_whiteouts_and_opaque_markers() {
        let prior = captured(vec![
            directory_entry("/opt"),
            directory_entry("/opt/tree"),
            regular_entry("/opt/tree/lower", content('a', 1)),
            regular_entry("/opt/remove", content('b', 2)),
        ]);
        let profile = user_profile();
        let mut opaque = directory_entry("/opt/tree");
        opaque
            .node
            .source
            .xattrs
            .insert("user.overlay.opaque".into(), b"y".to_vec());
        let mut whiteout = regular_entry("/opt/remove", content('0', 0));
        whiteout
            .node
            .source
            .xattrs
            .insert("user.overlay.whiteout".into(), Vec::new());
        let delta =
            decode_captured_upper(directory_node(), vec![opaque, whiteout], &prior, &profile)
                .unwrap();

        assert_eq!(delta.removals, ["/opt/remove"]);
        assert_eq!(delta.opaque_directories, ["/opt/tree"]);
        let applied = delta.apply(&prior).unwrap();
        assert!(entry(&applied, "/opt/remove").is_none());
        assert!(entry(&applied, "/opt/tree/lower").is_none());
    }

    #[test]
    fn opaque_x_only_enables_whiteout_scanning() {
        let prior = captured(vec![
            directory_entry("/opt"),
            directory_entry("/opt/tree"),
            regular_entry("/opt/tree/lower", content('a', 1)),
        ]);
        let mut directory = directory_entry("/opt/tree");
        directory
            .node
            .source
            .xattrs
            .insert("user.overlay.opaque".into(), b"x".to_vec());
        let delta =
            decode_captured_upper(directory_node(), vec![directory], &prior, &user_profile())
                .unwrap();
        assert!(delta.opaque_directories.is_empty());
        assert!(entry(&delta.apply(&prior).unwrap(), "/opt/tree/lower").is_some());
    }

    #[test]
    fn unescapes_payload_xattrs_and_rejects_unknown_private_markers() {
        let prior = captured(vec![directory_entry("/usr")]);
        let mut escaped = directory_entry("/usr");
        escaped
            .node
            .source
            .xattrs
            .insert("user.overlay.overlay.metacopy".into(), b"payload".to_vec());
        let delta = decode_captured_upper(directory_node(), vec![escaped], &prior, &user_profile())
            .unwrap();
        assert_eq!(
            delta.upserts[0]
                .node
                .source
                .xattrs
                .get("user.overlay.metacopy")
                .map(Vec::as_slice),
            Some(b"payload".as_slice())
        );

        let mut unknown = directory_entry("/usr");
        unknown
            .node
            .source
            .xattrs
            .insert("user.overlay.future".into(), b"1".to_vec());
        assert!(
            decode_captured_upper(directory_node(), vec![unknown], &prior, &user_profile(),)
                .is_err()
        );
    }

    #[test]
    fn strips_index_origin_from_upper_root_metadata() {
        let prior = captured(vec![directory_entry("/usr")]);
        let mut upper_root = directory_node();
        upper_root
            .source
            .xattrs
            .insert("user.overlay.origin".into(), b"root-handle".to_vec());
        let delta = decode_captured_upper(upper_root, Vec::new(), &prior, &user_profile()).unwrap();
        assert!(delta.root.is_none());
    }

    #[test]
    fn rejects_redirect_metacopy_and_protected_inode_markers() {
        let prior = captured(vec![directory_entry("/usr")]);
        for marker in ["redirect", "metacopy", "protattr"] {
            let mut entry = directory_entry("/usr");
            entry
                .node
                .source
                .xattrs
                .insert(format!("user.overlay.{marker}"), b"value".to_vec());
            assert!(
                decode_captured_upper(directory_node(), vec![entry], &prior, &user_profile(),)
                    .is_err()
            );
        }
    }

    #[test]
    fn copied_up_prior_hardlink_expands_complete_group_authority() {
        let identity = "prior:hardlink:1";
        let mut primary = regular_entry("/usr/lib/a", content('a', 4));
        primary.node.source.kind = PayloadNodeKind::Regular {
            hardlink_identity: Some(identity.into()),
        };
        let mut alias = primary.clone();
        alias.path = "/usr/lib/b".into();
        alias.content = None;
        alias.node.source.kind = PayloadNodeKind::Hardlink {
            target: primary.path.clone(),
            identity: identity.into(),
        };
        let prior = captured(vec![
            directory_entry("/usr"),
            directory_entry("/usr/lib"),
            primary,
            alias,
        ]);
        let mut changed = regular_entry("/usr/lib/b", content('c', 7));
        changed
            .node
            .source
            .xattrs
            .insert("user.overlay.origin".into(), b"opaque-handle".to_vec());
        let delta = decode_captured_upper(directory_node(), vec![changed], &prior, &user_profile())
            .unwrap();
        let applied = delta.apply(&prior).unwrap();
        assert_eq!(
            entry(&applied, "/usr/lib/a")
                .and_then(|entry| entry.content.as_ref())
                .map(|content| content.size),
            Some(7)
        );
        assert!(matches!(
            &entry(&applied, "/usr/lib/b").unwrap().node.source.kind,
            PayloadNodeKind::Hardlink { target, identity: found }
                if target == "/usr/lib/a" && found == identity
        ));
    }

    #[test]
    fn removing_prior_hardlink_primary_reanchors_survivors() {
        let identity = "prior:hardlink:1";
        let mut primary = regular_entry("/usr/lib/a", content('a', 4));
        primary.node.source.kind = PayloadNodeKind::Regular {
            hardlink_identity: Some(identity.into()),
        };
        let mut alias = primary.clone();
        alias.path = "/usr/lib/b".into();
        alias.content = None;
        alias.node.source.kind = PayloadNodeKind::Hardlink {
            target: primary.path.clone(),
            identity: identity.into(),
        };
        let prior = captured(vec![
            directory_entry("/usr"),
            directory_entry("/usr/lib"),
            primary,
            alias,
        ]);
        let mut whiteout = regular_entry("/usr/lib/a", content('0', 0));
        whiteout
            .node
            .source
            .xattrs
            .insert("user.overlay.whiteout".into(), Vec::new());
        let applied =
            decode_captured_upper(directory_node(), vec![whiteout], &prior, &user_profile())
                .unwrap()
                .apply(&prior)
                .unwrap();
        assert!(entry(&applied, "/usr/lib/a").is_none());
        assert!(matches!(
            &entry(&applied, "/usr/lib/b").unwrap().node.source.kind,
            PayloadNodeKind::Regular {
                hardlink_identity: None
            }
        ));
    }

    #[test]
    fn removing_only_prior_alias_clears_primary_group_identity() {
        let identity = "prior:hardlink:1";
        let mut primary = regular_entry("/usr/lib/a", content('a', 4));
        primary.node.source.kind = PayloadNodeKind::Regular {
            hardlink_identity: Some(identity.into()),
        };
        let mut alias = primary.clone();
        alias.path = "/usr/lib/b".into();
        alias.content = None;
        alias.node.source.kind = PayloadNodeKind::Hardlink {
            target: primary.path.clone(),
            identity: identity.into(),
        };
        let prior = captured(vec![
            directory_entry("/usr"),
            directory_entry("/usr/lib"),
            primary,
            alias,
        ]);
        let mut whiteout = regular_entry("/usr/lib/b", content('0', 0));
        whiteout
            .node
            .source
            .xattrs
            .insert("user.overlay.whiteout".into(), Vec::new());
        let applied =
            decode_captured_upper(directory_node(), vec![whiteout], &prior, &user_profile())
                .unwrap()
                .apply(&prior)
                .unwrap();
        assert!(entry(&applied, "/usr/lib/b").is_none());
        assert!(matches!(
            &entry(&applied, "/usr/lib/a").unwrap().node.source.kind,
            PayloadNodeKind::Regular {
                hardlink_identity: None
            }
        ));
    }

    #[test]
    fn new_alias_joins_copied_up_prior_hardlink_group() {
        let prior_identity = "prior:hardlink:1";
        let upper_identity = "selected-root-overlay-v1:hardlink:1";
        let mut primary = regular_entry("/usr/lib/a", content('a', 4));
        primary.node.source.kind = PayloadNodeKind::Regular {
            hardlink_identity: Some(prior_identity.into()),
        };
        let mut alias = primary.clone();
        alias.path = "/usr/lib/b".into();
        alias.content = None;
        alias.node.source.kind = PayloadNodeKind::Hardlink {
            target: primary.path.clone(),
            identity: prior_identity.into(),
        };
        let prior = captured(vec![
            directory_entry("/usr"),
            directory_entry("/usr/lib"),
            primary,
            alias,
        ]);
        let mut copied = regular_entry("/usr/lib/a", content('c', 7));
        copied.node.source.kind = PayloadNodeKind::Regular {
            hardlink_identity: Some(upper_identity.into()),
        };
        copied
            .node
            .source
            .xattrs
            .insert("user.overlay.origin".into(), b"opaque-handle".to_vec());
        let mut new_alias = copied.clone();
        new_alias.path = "/usr/lib/c".into();
        new_alias.content = None;
        new_alias.node.source.kind = PayloadNodeKind::Hardlink {
            target: copied.path.clone(),
            identity: upper_identity.into(),
        };
        let applied = decode_captured_upper(
            directory_node(),
            vec![copied, new_alias],
            &prior,
            &user_profile(),
        )
        .unwrap()
        .apply(&prior)
        .unwrap();
        for path in ["/usr/lib/b", "/usr/lib/c"] {
            assert!(matches!(
                &entry(&applied, path).unwrap().node.source.kind,
                PayloadNodeKind::Hardlink { target, identity }
                    if target == "/usr/lib/a" && identity == prior_identity
            ));
        }
    }

    #[test]
    fn independent_primary_replacement_reanchors_old_alias() {
        let identity = "prior:hardlink:1";
        let mut primary = regular_entry("/usr/lib/a", content('a', 4));
        primary.node.source.kind = PayloadNodeKind::Regular {
            hardlink_identity: Some(identity.into()),
        };
        let mut alias = primary.clone();
        alias.path = "/usr/lib/b".into();
        alias.content = None;
        alias.node.source.kind = PayloadNodeKind::Hardlink {
            target: primary.path.clone(),
            identity: identity.into(),
        };
        let prior = captured(vec![
            directory_entry("/usr"),
            directory_entry("/usr/lib"),
            primary,
            alias,
        ]);
        let replacement = regular_entry("/usr/lib/a", content('c', 9));
        let applied =
            decode_captured_upper(directory_node(), vec![replacement], &prior, &user_profile())
                .unwrap()
                .apply(&prior)
                .unwrap();
        assert_eq!(
            entry(&applied, "/usr/lib/a")
                .and_then(|entry| entry.content.as_ref())
                .map(|content| content.size),
            Some(9)
        );
        assert_eq!(
            entry(&applied, "/usr/lib/b")
                .and_then(|entry| entry.content.as_ref())
                .map(|content| content.size),
            Some(4)
        );
        for path in ["/usr/lib/a", "/usr/lib/b"] {
            assert!(matches!(
                &entry(&applied, path).unwrap().node.source.kind,
                PayloadNodeKind::Regular {
                    hardlink_identity: None
                }
            ));
        }
    }

    #[test]
    fn filesystem_decoder_reads_only_upper_content() {
        let temp = tempfile::tempdir().unwrap();
        let upper = temp.path().join("upper");
        std::fs::create_dir_all(upper.join("usr/bin")).unwrap();
        std::fs::set_permissions(&upper, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(upper.join("usr/bin/changed"), b"changed only").unwrap();
        let prior = captured(vec![
            directory_entry("/usr"),
            directory_entry("/usr/bin"),
            regular_entry("/usr/bin/unchanged", content('a', 1_000_000)),
        ]);
        let cas = CasStore::new(temp.path().join("objects")).unwrap();
        let delta =
            decode_selected_root_overlay_upper(&upper, &prior, &cas, &user_profile()).unwrap();
        assert_eq!(delta.upserts.len(), 3);
        assert!(delta.upserts.iter().any(|entry| {
            entry.path == "/usr/bin/changed"
                && entry
                    .content
                    .as_ref()
                    .is_some_and(|content| content.size == 12)
        }));
        assert!(entry(&delta.apply(&prior).unwrap(), "/usr/bin/unchanged").is_some());
    }

    fn user_profile() -> SelectedRootOverlayProfile {
        SelectedRootOverlayProfile {
            xattr_namespace: OverlayXattrNamespace::User,
            ..SelectedRootOverlayProfile::trusted()
        }
    }

    fn captured(entries: Vec<GenerationRootEntry>) -> CapturedSelectedRoot {
        let (generation, state): (Vec<_>, Vec<_>) = entries.into_iter().partition(|entry| {
            super::super::classify_root_path(&entry.path).unwrap()
                == super::super::RootPathDomain::Immutable
        });
        CapturedSelectedRoot {
            generation: super::super::GenerationRootManifest {
                version: super::super::GENERATION_ROOT_MANIFEST_VERSION,
                root: directory_node(),
                entries: sorted(generation),
            },
            state: super::super::MutableStateManifest {
                version: super::super::GENERATION_ROOT_MANIFEST_VERSION,
                entries: sorted(state),
            },
        }
    }

    fn sorted(mut entries: Vec<GenerationRootEntry>) -> Vec<GenerationRootEntry> {
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        entries
    }

    fn entry<'a>(
        captured: &'a CapturedSelectedRoot,
        path: &str,
    ) -> Option<&'a GenerationRootEntry> {
        captured
            .generation
            .entries
            .iter()
            .chain(&captured.state.entries)
            .find(|entry| entry.path == path)
    }

    fn directory_entry(path: &str) -> GenerationRootEntry {
        GenerationRootEntry {
            path: path.into(),
            node: directory_node(),
            content: None,
        }
    }

    fn regular_entry(path: &str, content: PayloadContentAuthority) -> GenerationRootEntry {
        GenerationRootEntry {
            path: path.into(),
            node: ResolvedPayloadNode::from_numeric_source(PayloadNode::regular(0o644)).unwrap(),
            content: Some(content),
        }
    }

    fn directory_node() -> ResolvedPayloadNode {
        let mut node = PayloadNode::regular(0o755);
        node.kind = PayloadNodeKind::Directory;
        node.mode = libc::S_IFDIR | 0o755;
        node.user = PayloadIdentity::Numeric { id: 0 };
        node.group = PayloadIdentity::Numeric { id: 0 };
        ResolvedPayloadNode::from_numeric_source(node).unwrap()
    }

    fn content(byte: char, size: u64) -> PayloadContentAuthority {
        PayloadContentAuthority {
            sha256: byte.to_string().repeat(64),
            size,
        }
    }
}
