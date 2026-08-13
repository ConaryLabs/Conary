// conary-core/src/generation/root_manifest.rs

//! Authoritative selected-root capture for generation and mutable-state publication.

mod composefs;
mod delta;
mod materialize;
mod overlay;
mod scan;

use crate::payload::{PayloadContentAuthority, PayloadNode, PayloadNodeKind, ResolvedPayloadNode};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

pub use composefs::build_erofs_image_from_root_manifest;
pub use delta::{SELECTED_ROOT_MANIFEST_DELTA_VERSION, SelectedRootManifestDelta};
pub use materialize::{
    apply_resolved_payload_metadata, materialize_captured_selected_root,
    materialize_config_state_upper, materialize_generation_root, materialize_state_root,
    overlay_payload_entries,
};
pub use overlay::{
    OverlayXattrNamespace, SELECTED_ROOT_OVERLAY_PROFILE_VERSION, SelectedRootOverlayProfile,
    decode_selected_root_overlay_upper,
};
pub use scan::{
    SelectedRootCaptureExclusions, capture_existing_payload_node, capture_root_node,
    scan_payload_tree, scan_selected_root, scan_selected_root_with_exclusions,
};

pub const GENERATION_ROOT_MANIFEST_FILE: &str = "generation-root.json";
pub const MUTABLE_STATE_MANIFEST_FILE: &str = "mutable-state.json";
pub const GENERATION_ROOT_MANIFEST_VERSION: u32 = 1;

/// Finite path-domain contract for selected-root publication.
///
/// Classification uses only the normalized first path component. It is a
/// persisted compatibility contract, not a discovery heuristic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RootPathDomain {
    Immutable,
    ConfigState,
    MutableState,
    EphemeralMountOrUser,
}

/// One path and its complete POSIX node/content authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationRootEntry {
    pub path: String,
    pub node: ResolvedPayloadNode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<PayloadContentAuthority>,
}

/// Complete immutable root input for one generation image.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationRootManifest {
    pub version: u32,
    pub root: ResolvedPayloadNode,
    pub entries: Vec<GenerationRootEntry>,
}

/// Complete evidence snapshot for `/etc`, `/var`, and `/srv`.
///
/// Publication derives a touched-node transaction between before and after
/// snapshots. It never replaces a live tree wholesale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MutableStateManifest {
    pub version: u32,
    pub entries: Vec<GenerationRootEntry>,
}

/// Selected-root capture split by its exact publication owners.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapturedSelectedRoot {
    pub generation: GenerationRootManifest,
    pub state: MutableStateManifest,
}

/// Exact before/after evidence for state paths touched by a transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MutableStateTransaction {
    pub version: u32,
    pub before: MutableStateManifest,
    pub after: MutableStateManifest,
    pub touched_paths: Vec<String>,
}

impl GenerationRootEntry {
    pub fn validate(&self) -> crate::Result<()> {
        validate_root_path(&self.path)?;
        self.node
            .validate()
            .map_err(|error| crate::Error::InvalidPath(error.to_string()))?;
        self.node
            .source
            .validate_content(self.content.as_ref())
            .map_err(|error| crate::Error::InvalidPath(error.to_string()))?;
        validate_mode_matches_kind(&self.path, &self.node.source)?;
        Ok(())
    }
}

impl GenerationRootManifest {
    pub fn validate(&self) -> crate::Result<()> {
        require_manifest_version(self.version)?;
        validate_payload_tree(&self.root, &self.entries)?;
        validate_entries(&self.entries, |path| {
            matches!(classify_root_path(path), Ok(RootPathDomain::Immutable))
        })
    }

    pub fn regular_contents(&self) -> impl Iterator<Item = &PayloadContentAuthority> {
        self.entries
            .iter()
            .filter_map(|entry| entry.content.as_ref())
    }

    pub fn write_to(&self, generation_dir: &Path) -> crate::Result<()> {
        self.validate()?;
        crate::filesystem::durable::write_json_atomic(
            &generation_dir.join(GENERATION_ROOT_MANIFEST_FILE),
            self,
        )
    }

    pub fn read_from(generation_dir: &Path) -> crate::Result<Self> {
        let path = generation_dir.join(GENERATION_ROOT_MANIFEST_FILE);
        let bytes = std::fs::read(&path).map_err(|error| {
            crate::Error::NotFound(format!(
                "missing generation root manifest {}: {error}",
                path.display()
            ))
        })?;
        let manifest: Self = serde_json::from_slice(&bytes)?;
        manifest.validate()?;
        Ok(manifest)
    }
}

/// Validate a complete exact payload tree independent of publication domain.
///
/// Derivation outputs use this contract before composition splits their
/// entries into immutable generation and mutable-state manifests.
pub fn validate_payload_tree(
    root: &ResolvedPayloadNode,
    entries: &[GenerationRootEntry],
) -> crate::Result<()> {
    root.validate()
        .map_err(|error| crate::Error::InvalidPath(error.to_string()))?;
    if !matches!(root.source.kind, PayloadNodeKind::Directory) {
        return Err(crate::Error::InvalidPath(
            "payload-tree root metadata must describe a directory".to_string(),
        ));
    }
    validate_mode_matches_kind("/", &root.source)?;
    validate_entries(entries, |_| true)?;
    validate_hardlinks(entries)
}

impl MutableStateManifest {
    pub fn empty() -> Self {
        Self {
            version: GENERATION_ROOT_MANIFEST_VERSION,
            entries: Vec::new(),
        }
    }

    pub fn validate(&self) -> crate::Result<()> {
        require_manifest_version(self.version)?;
        validate_entries(&self.entries, |path| {
            matches!(
                classify_root_path(path),
                Ok(RootPathDomain::ConfigState) | Ok(RootPathDomain::MutableState)
            )
        })?;
        validate_hardlinks(&self.entries)
    }

    pub fn regular_contents(&self) -> impl Iterator<Item = &PayloadContentAuthority> {
        self.entries
            .iter()
            .filter_map(|entry| entry.content.as_ref())
    }

    pub fn write_to(&self, generation_dir: &Path) -> crate::Result<()> {
        self.validate()?;
        crate::filesystem::durable::write_json_atomic(
            &generation_dir.join(MUTABLE_STATE_MANIFEST_FILE),
            self,
        )
    }

    pub fn read_from(generation_dir: &Path) -> crate::Result<Self> {
        let path = generation_dir.join(MUTABLE_STATE_MANIFEST_FILE);
        let bytes = std::fs::read(&path).map_err(|error| {
            crate::Error::NotFound(format!(
                "missing mutable-state manifest {}: {error}",
                path.display()
            ))
        })?;
        let manifest: Self = serde_json::from_slice(&bytes)?;
        manifest.validate()?;
        Ok(manifest)
    }
}

impl MutableStateTransaction {
    pub fn between(
        before: MutableStateManifest,
        after: MutableStateManifest,
    ) -> crate::Result<Self> {
        before.validate()?;
        after.validate()?;
        let before_by_path = entries_by_path(&before.entries);
        let after_by_path = entries_by_path(&after.entries);
        let touched_paths = before_by_path
            .keys()
            .chain(after_by_path.keys())
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter(|path| before_by_path.get(path) != after_by_path.get(path))
            .map(str::to_string)
            .collect();
        Ok(Self {
            version: GENERATION_ROOT_MANIFEST_VERSION,
            before,
            after,
            touched_paths,
        })
    }

    pub fn validate(&self) -> crate::Result<()> {
        require_manifest_version(self.version)?;
        self.before.validate()?;
        self.after.validate()?;
        let expected = Self::between(self.before.clone(), self.after.clone())?;
        if self.touched_paths != expected.touched_paths {
            return Err(crate::Error::InvalidPath(
                "mutable-state transaction touched paths do not match before/after evidence"
                    .to_string(),
            ));
        }
        Ok(())
    }
}

pub fn classify_root_path(path: &str) -> crate::Result<RootPathDomain> {
    validate_root_path(path)?;
    let first = path
        .strip_prefix('/')
        .and_then(|relative| relative.split('/').next())
        .ok_or_else(|| crate::Error::InvalidPath(format!("invalid root path {path:?}")))?;
    Ok(match first {
        "etc" => RootPathDomain::ConfigState,
        "var" | "srv" => RootPathDomain::MutableState,
        "proc" | "sys" | "dev" | "run" | "tmp" | "home" | "root" | "mnt" | "media" => {
            RootPathDomain::EphemeralMountOrUser
        }
        _ => RootPathDomain::Immutable,
    })
}

fn require_manifest_version(version: u32) -> crate::Result<()> {
    if version == GENERATION_ROOT_MANIFEST_VERSION {
        Ok(())
    } else {
        Err(crate::Error::InvalidPath(format!(
            "generation root manifest has unsupported version {version}; expected {GENERATION_ROOT_MANIFEST_VERSION}"
        )))
    }
}

fn validate_entries(
    entries: &[GenerationRootEntry],
    owns_path: impl Fn(&str) -> bool,
) -> crate::Result<()> {
    let mut previous: Option<&str> = None;
    let mut directories = BTreeSet::from(["/"]);
    for entry in entries {
        entry.validate()?;
        if !owns_path(&entry.path) {
            return Err(crate::Error::InvalidPath(format!(
                "root manifest contains path in the wrong publication domain: {}",
                entry.path
            )));
        }
        if previous.is_some_and(|path| path >= entry.path.as_str()) {
            return Err(crate::Error::InvalidPath(
                "root manifest entries must be unique and sorted by path".to_string(),
            ));
        }
        let parent = Path::new(&entry.path)
            .parent()
            .and_then(Path::to_str)
            .unwrap_or("/");
        if !directories.contains(parent) {
            return Err(crate::Error::InvalidPath(format!(
                "root manifest path {} has no explicit parent directory {}",
                entry.path, parent
            )));
        }
        if matches!(entry.node.source.kind, PayloadNodeKind::Directory) {
            directories.insert(entry.path.as_str());
        }
        previous = Some(&entry.path);
    }
    Ok(())
}

fn validate_hardlinks(entries: &[GenerationRootEntry]) -> crate::Result<()> {
    let by_path = entries_by_path(entries);
    let mut identities = BTreeMap::<&str, (&GenerationRootEntry, usize)>::new();
    for entry in entries {
        let PayloadNodeKind::Regular {
            hardlink_identity: Some(identity),
        } = &entry.node.source.kind
        else {
            continue;
        };
        if identities.insert(identity, (entry, 1)).is_some() {
            return Err(crate::Error::InvalidPath(format!(
                "hardlink identity {identity:?} has more than one primary node"
            )));
        }
    }
    for entry in entries {
        let PayloadNodeKind::Hardlink { target, identity } = &entry.node.source.kind else {
            continue;
        };
        let target_entry = by_path.get(target.as_str()).ok_or_else(|| {
            crate::Error::InvalidPath(format!(
                "hardlink {} targets missing path {target}",
                entry.path
            ))
        })?;
        let Some((primary, count)) = identities.get_mut(identity.as_str()) else {
            return Err(crate::Error::InvalidPath(format!(
                "hardlink {} uses identity {identity:?} without a primary node",
                entry.path
            )));
        };
        if primary.path != target_entry.path {
            return Err(crate::Error::InvalidPath(format!(
                "hardlink {} identity {identity:?} targets {}, expected {}",
                entry.path, target, primary.path
            )));
        }
        if classify_root_path(&entry.path)? != classify_root_path(&primary.path)? {
            return Err(crate::Error::InvalidPath(format!(
                "hardlink group crosses generation publication domains at {}",
                primary.path
            )));
        }
        if entry.node.source.mode != primary.node.source.mode
            || entry.node.source.user != primary.node.source.user
            || entry.node.source.group != primary.node.source.group
            || entry.node.source.mtime != primary.node.source.mtime
            || entry.node.source.xattrs != primary.node.source.xattrs
            || entry.node.uid != primary.node.uid
            || entry.node.gid != primary.node.gid
        {
            return Err(crate::Error::InvalidPath(format!(
                "hardlink {} metadata differs from primary {}",
                entry.path, primary.path
            )));
        }
        *count += 1;
    }
    if let Some((identity, _)) = identities.iter().find(|(_, (_, count))| *count < 2) {
        return Err(crate::Error::InvalidPath(format!(
            "hardlink primary identity {identity:?} has no linked path"
        )));
    }
    Ok(())
}

fn entries_by_path(entries: &[GenerationRootEntry]) -> BTreeMap<&str, &GenerationRootEntry> {
    entries
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect()
}

fn validate_root_path(path: &str) -> crate::Result<()> {
    let parsed = Path::new(path);
    if !parsed.is_absolute()
        || path == "/"
        || path.ends_with('/')
        || path
            .split('/')
            .skip(1)
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(crate::Error::InvalidPath(format!(
            "root manifest path must be a normalized absolute non-root path: {path:?}"
        )));
    }
    for component in parsed.components() {
        match component {
            Component::RootDir | Component::Normal(_) => {}
            Component::Prefix(_) | Component::CurDir | Component::ParentDir => {
                return Err(crate::Error::InvalidPath(format!(
                    "root manifest path is not normalized: {path:?}"
                )));
            }
        }
    }
    Ok(())
}

fn validate_mode_matches_kind(path: &str, node: &PayloadNode) -> crate::Result<()> {
    let expected = match node.kind {
        PayloadNodeKind::Regular { .. } => libc::S_IFREG,
        PayloadNodeKind::Directory => libc::S_IFDIR,
        PayloadNodeKind::Symlink { .. } => libc::S_IFLNK,
        PayloadNodeKind::Hardlink { .. } => return Ok(()),
        PayloadNodeKind::BlockDevice { .. } => libc::S_IFBLK,
        PayloadNodeKind::CharacterDevice { .. } => libc::S_IFCHR,
        PayloadNodeKind::Fifo => libc::S_IFIFO,
        PayloadNodeKind::Socket => libc::S_IFSOCK,
    };
    if node.mode & libc::S_IFMT != expected {
        return Err(crate::Error::InvalidPath(format!(
            "payload node kind for {path} disagrees with full mode {:o}",
            node.mode
        )));
    }
    Ok(())
}

#[cfg(test)]
#[path = "root_manifest/tests.rs"]
mod tests;
