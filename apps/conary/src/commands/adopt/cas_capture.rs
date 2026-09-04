// apps/conary/src/commands/adopt/cas_capture.rs

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use conary_core::db::models::FileEntry;
use conary_core::filesystem::PrivateCasWriter;
use conary_core::payload::{PayloadContentAuthority, PayloadNodeKind, ResolvedPayloadNode};

use super::FileInfoTuple;

/// Exact live payload captured before an adoption database transaction.
///
/// Package-manager tuples retain discovery evidence only. Installed
/// correctness comes from the typed live node and the exact bytes captured
/// here, never from package-manager digests, mode guesses, or placeholders.
#[derive(Debug, Clone)]
pub(crate) struct CapturedAdoptionFile {
    pub(crate) source: FileInfoTuple,
    pub(crate) node: ResolvedPayloadNode,
    pub(crate) content: Option<PayloadContentAuthority>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct HardlinkKey {
    device: u64,
    inode: u64,
}

struct CapturedFile {
    file: CapturedAdoptionFile,
    hardlink_key: Option<HardlinkKey>,
}

/// Path-indexed authority from one complete selected-root capture.
pub(crate) struct SelectedRootPayloadIndex {
    entries: BTreeMap<String, (ResolvedPayloadNode, Option<PayloadContentAuthority>)>,
}

impl SelectedRootPayloadIndex {
    pub(crate) fn new(
        captured: &conary_core::generation::root_manifest::CapturedSelectedRoot,
    ) -> Self {
        let entries = captured
            .generation
            .entries
            .iter()
            .chain(&captured.state.entries)
            .map(|entry| {
                (
                    entry.path.clone(),
                    (entry.node.clone(), entry.content.clone()),
                )
            })
            .collect();
        Self { entries }
    }
}

impl CapturedAdoptionFile {
    pub(crate) fn file_entry(&self, trove_id: i64) -> FileEntry {
        FileEntry::new(
            self.source.0.clone(),
            self.node.clone(),
            self.content.clone(),
            trove_id,
        )
    }
}

pub(crate) fn capture_package_files(
    package_name: &str,
    files: &[FileInfoTuple],
    cas: Option<&dyn PrivateCasWriter>,
) -> Result<Vec<CapturedAdoptionFile>> {
    let mut captured = files
        .iter()
        .map(|file| {
            capture_file(file, cas).map_err(|error| {
                anyhow!(
                    "package {package_name} has unresolved live payload path {}: {error}",
                    file.0
                )
            })
        })
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    bind_package_hardlinks(&mut captured)?;
    Ok(captured.into_iter().map(|captured| captured.file).collect())
}

/// Bind package declarations to a previously captured complete root.
///
/// Only paths intentionally absent from that capture (runtime exclusions and
/// ephemeral domains) are read separately. This keeps the selected-root scan
/// as the sole byte read for ordinary package-owned paths.
pub(crate) fn capture_package_files_from_selected_root(
    package_name: &str,
    files: &[FileInfoTuple],
    selected_root: &SelectedRootPayloadIndex,
    cas: &dyn PrivateCasWriter,
) -> Result<Vec<CapturedAdoptionFile>> {
    let mut captured = files
        .iter()
        .map(|file| {
            let path = Path::new(&file.0);
            if is_selected_root_anchor(path) || permitted_native_absence(file, path) {
                return Ok(None);
            }
            if let Some((node, content)) = selected_root.entries.get(&file.0) {
                return Ok(Some(CapturedFile {
                    file: CapturedAdoptionFile {
                        source: file.clone(),
                        node: node.clone(),
                        content: content.clone(),
                    },
                    hardlink_key: None,
                }));
            }
            capture_file(file, Some(cas))
        })
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    bind_package_hardlinks(&mut captured).map_err(|error| {
        anyhow!("package {package_name} has invalid excluded-path hardlink authority: {error}")
    })?;
    Ok(captured.into_iter().map(|captured| captured.file).collect())
}

pub(crate) fn validate_package_files(package_name: &str, files: &[FileInfoTuple]) -> Result<()> {
    for file in files {
        validate_file(file).map_err(|error| {
            anyhow!(
                "package {package_name} has unresolved live payload path {}: {error}",
                file.0
            )
        })?;
    }
    Ok(())
}

fn capture_file(
    file: &FileInfoTuple,
    cas: Option<&dyn PrivateCasWriter>,
) -> Result<Option<CapturedFile>> {
    let path = Path::new(&file.0);
    if is_selected_root_anchor(path) {
        return Ok(None);
    }
    if permitted_native_absence(file, path) {
        return Ok(None);
    }
    let node = conary_core::generation::root_manifest::capture_existing_payload_node(path)
        .map_err(anyhow::Error::from)
        .with_context(|| format!("failed to capture exact node {}", file.0))?;
    let (content, hardlink_key) = if matches!(node.source.kind, PayloadNodeKind::Regular { .. }) {
        let (bytes, hardlink_key) = read_regular_without_following(path)?;
        let sha256 = if let Some(cas) = cas {
            cas.store_private_copy(&bytes)
                .with_context(|| format!("failed to store {} in private CAS", file.0))?
        } else {
            conary_core::hash::sha256(&bytes)
        };
        (
            Some(PayloadContentAuthority {
                sha256,
                size: bytes.len() as u64,
            }),
            hardlink_key,
        )
    } else {
        (None, None)
    };
    node.source.validate_content(content.as_ref())?;
    Ok(Some(CapturedFile {
        file: CapturedAdoptionFile {
            source: file.clone(),
            node,
            content,
        },
        hardlink_key,
    }))
}

fn bind_package_hardlinks(captured: &mut [CapturedFile]) -> Result<()> {
    let mut groups = BTreeMap::<HardlinkKey, Vec<usize>>::new();
    for (index, captured) in captured.iter().enumerate() {
        if let Some(key) = captured.hardlink_key {
            groups.entry(key).or_default().push(index);
        }
    }
    for indices in groups.values_mut().filter(|indices| indices.len() > 1) {
        indices.sort_by(|left, right| {
            captured[*left]
                .file
                .source
                .0
                .cmp(&captured[*right].file.source.0)
        });
        let target = captured[indices[0]].file.source.0.clone();
        let identity_input = indices
            .iter()
            .map(|index| captured[*index].file.source.0.as_str())
            .collect::<Vec<_>>()
            .join("\0");
        let identity = format!(
            "adopted-hardlink:sha256:{}",
            conary_core::hash::sha256(identity_input.as_bytes())
        );
        captured[indices[0]].file.node.source.kind = PayloadNodeKind::Regular {
            hardlink_identity: Some(identity.clone()),
        };
        let primary = &captured[indices[0]].file;
        primary
            .node
            .source
            .validate_content(primary.content.as_ref())?;
        for index in indices.iter().skip(1) {
            let file = &mut captured[*index].file;
            file.node.source.kind = PayloadNodeKind::Hardlink {
                target: target.clone(),
                identity: identity.clone(),
            };
            file.content = None;
            file.node.source.validate_content(None)?;
        }
    }
    Ok(())
}

fn validate_file(file: &FileInfoTuple) -> Result<()> {
    let path = Path::new(&file.0);
    if is_selected_root_anchor(path) {
        return Ok(());
    }
    if permitted_native_absence(file, path) {
        return Ok(());
    }
    let node = conary_core::generation::root_manifest::capture_existing_payload_node(path)
        .map_err(anyhow::Error::from)
        .with_context(|| format!("failed to capture exact node {}", file.0))?;
    if matches!(node.source.kind, PayloadNodeKind::Regular { .. }) {
        read_regular_without_following(path)?;
    }
    Ok(())
}

/// The selected root owns its root node independently of package payload.
///
/// Native databases may record `/` as package ownership, but Conary's files
/// and directory-claim graphs intentionally contain paths below root only.
/// Persisting this record would duplicate the generation manifest's root
/// authority and make package removal target the root anchor itself.
fn is_selected_root_anchor(path: &Path) -> bool {
    path == Path::new("/")
}

fn permitted_native_absence(file: &FileInfoTuple, path: &Path) -> bool {
    if !file.7.permits_absence() {
        return false;
    }
    matches!(
        std::fs::symlink_metadata(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound
    )
}

fn read_regular_without_following(path: &Path) -> Result<(Vec<u8>, Option<HardlinkKey>)> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("regular file {} is not safely readable", path.display()))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("failed to inspect opened regular file {}", path.display()))?;
    if !metadata.file_type().is_file() {
        return Err(anyhow!(
            "{} changed node type while its payload was captured",
            path.display()
        ));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .with_context(|| format!("failed to read regular file {}", path.display()))?;
    #[cfg(unix)]
    let hardlink_key = {
        use std::os::unix::fs::MetadataExt;
        (metadata.nlink() > 1).then_some(HardlinkKey {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    };
    #[cfg(not(unix))]
    let hardlink_key = None;
    Ok((bytes, hardlink_key))
}

#[cfg(test)]
#[path = "cas_capture/tests.rs"]
mod tests;
