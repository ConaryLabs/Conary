// apps/conary/src/commands/adopt/cas_capture.rs

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use conary_core::db::models::FileEntry;
use conary_core::filesystem::CasStore;
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
    cas: Option<&CasStore>,
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

fn capture_file(file: &FileInfoTuple, cas: Option<&CasStore>) -> Result<Option<CapturedFile>> {
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
mod tests {
    use super::*;
    use conary_core::filesystem::CasStore;
    use conary_core::packages::InstalledFileAbsencePolicy;

    fn tempdir_in_target() -> tempfile::TempDir {
        std::fs::create_dir_all("target").unwrap();
        tempfile::Builder::new()
            .prefix("conary-cas-capture-")
            .tempdir_in("target")
            .unwrap()
    }

    fn file_tuple(
        path: &str,
        size: i64,
        mode: i32,
        digest: Option<&str>,
        link_target: Option<&str>,
    ) -> super::super::FileInfoTuple {
        (
            path.to_string(),
            size,
            mode,
            digest.map(str::to_string),
            Some("root".to_string()),
            Some("root".to_string()),
            link_target.map(str::to_string),
            InstalledFileAbsencePolicy::Required,
        )
    }

    fn file_tuple_with_policy(
        path: &str,
        policy: InstalledFileAbsencePolicy,
    ) -> super::super::FileInfoTuple {
        let mut file = file_tuple(path, 0, 0o100644, None, None);
        file.7 = policy;
        file
    }

    fn assert_error_contains(error: anyhow::Error, snippets: &[&str]) {
        let error = error.to_string();
        for snippet in snippets {
            assert!(
                error.contains(snippet),
                "expected error to contain {snippet:?}; got {error}"
            );
        }
    }

    #[test]
    fn full_adoption_regular_file_requires_cas_storage() {
        let tmp = tempdir_in_target();
        let source = tmp.path().join("hello");
        std::fs::write(&source, b"hello").unwrap();
        let cwd = std::env::current_dir().unwrap();
        let source = source.strip_prefix(cwd).unwrap();
        let cas = CasStore::new(tmp.path().join("objects")).unwrap();

        let captured = capture_package_files(
            "fixture",
            &[file_tuple(
                source.to_str().unwrap(),
                5,
                0o100644,
                Some("package-manager-digest"),
                None,
            )],
            Some(&cas),
        )
        .unwrap();
        let authority = captured[0].content.as_ref().unwrap();

        assert_ne!(authority.sha256, "package-manager-digest");
        assert_eq!(cas.retrieve(&authority.sha256).unwrap(), b"hello");
    }

    #[test]
    fn full_adoption_cas_survives_in_place_source_mutation() {
        let tmp = tempdir_in_target();
        let source = tmp.path().join("mutable-source");
        std::fs::write(&source, b"original bytes").unwrap();
        let cwd = std::env::current_dir().unwrap();
        let source_arg = source.strip_prefix(cwd).unwrap();
        let cas = CasStore::new(tmp.path().join("objects")).unwrap();

        let captured = capture_package_files(
            "fixture",
            &[file_tuple(
                source_arg.to_str().unwrap(),
                14,
                0o100644,
                Some("package-manager-digest"),
                None,
            )],
            Some(&cas),
        )
        .unwrap();
        let hash = &captured[0].content.as_ref().unwrap().sha256;

        std::fs::write(&source, b"mutated bytes").unwrap();

        assert_eq!(cas.retrieve(hash).unwrap(), b"original bytes");
    }

    #[test]
    #[cfg(unix)]
    fn full_adoption_regular_file_uses_private_cas_inode() {
        use std::os::unix::fs::MetadataExt;

        let tmp = tempdir_in_target();
        let source = tmp.path().join("private-inode-source");
        std::fs::write(&source, b"private inode bytes").unwrap();
        let cwd = std::env::current_dir().unwrap();
        let source_arg = source.strip_prefix(cwd).unwrap();
        let cas = CasStore::new(tmp.path().join("objects")).unwrap();

        let captured = capture_package_files(
            "fixture",
            &[file_tuple(
                source_arg.to_str().unwrap(),
                19,
                0o100644,
                Some("package-manager-digest"),
                None,
            )],
            Some(&cas),
        )
        .unwrap();
        let cas_path = cas
            .hash_to_path(&captured[0].content.as_ref().unwrap().sha256)
            .unwrap();

        assert_ne!(
            std::fs::metadata(&source).unwrap().ino(),
            std::fs::metadata(&cas_path).unwrap().ino(),
            "live full adoption must not share an inode with mutable source files"
        );
    }

    #[test]
    #[cfg(unix)]
    fn full_adoption_captures_package_hardlink_topology() {
        let tmp = tempdir_in_target();
        let primary = tmp.path().join("hardlink-a");
        let secondary = tmp.path().join("hardlink-b");
        std::fs::write(&primary, b"shared inode bytes").unwrap();
        std::fs::hard_link(&primary, &secondary).unwrap();
        let cwd = std::env::current_dir().unwrap();
        let primary_arg = primary.strip_prefix(&cwd).unwrap().to_str().unwrap();
        let secondary_arg = secondary.strip_prefix(&cwd).unwrap().to_str().unwrap();

        let captured = capture_package_files(
            "fixture",
            &[
                file_tuple(secondary_arg, 18, 0o100644, None, None),
                file_tuple(primary_arg, 18, 0o100644, None, None),
            ],
            None,
        )
        .unwrap();

        let primary = captured
            .iter()
            .find(|file| file.source.0 == primary_arg)
            .unwrap();
        let secondary = captured
            .iter()
            .find(|file| file.source.0 == secondary_arg)
            .unwrap();
        let PayloadNodeKind::Regular {
            hardlink_identity: Some(primary_identity),
        } = &primary.node.source.kind
        else {
            panic!("lexicographically first package hardlink is not the primary");
        };
        let PayloadNodeKind::Hardlink { target, identity } = &secondary.node.source.kind else {
            panic!("second package hardlink is not typed as a hardlink");
        };
        assert_eq!(target, primary_arg);
        assert_eq!(identity, primary_identity);
        assert!(primary.content.is_some());
        assert!(secondary.content.is_none());
    }

    #[test]
    fn full_adoption_symlink_persists_typed_target_without_fake_content() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cas = CasStore::new(tmp.path().join("objects")).unwrap();
        let link = tmp.path().join("libfoo.so");
        std::os::unix::fs::symlink("libfoo.so.1", &link).unwrap();

        let captured = capture_package_files(
            "fixture",
            &[file_tuple(
                link.to_str().unwrap(),
                11,
                0o120777,
                Some("package-manager-digest"),
                Some("wrong-discovery-target"),
            )],
            Some(&cas),
        )
        .unwrap();

        assert!(captured[0].content.is_none());
        assert_eq!(
            captured[0].node.source.kind,
            PayloadNodeKind::Symlink {
                target: "libfoo.so.1".to_string()
            }
        );
    }

    #[test]
    fn full_adoption_directory_does_not_require_cas_content() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cas = CasStore::new(tmp.path().join("objects")).unwrap();
        let directory = tmp.path().join("doc");
        std::fs::create_dir(&directory).unwrap();

        let captured = capture_package_files(
            "fixture",
            &[file_tuple(
                directory.to_str().unwrap(),
                0,
                0o040755,
                Some("directory-digest"),
                None,
            )],
            Some(&cas),
        )
        .unwrap();

        assert!(captured[0].content.is_none());
        assert!(matches!(
            captured[0].node.source.kind,
            PayloadNodeKind::Directory
        ));
    }

    #[test]
    fn full_adoption_special_nodes_have_typed_authority_without_fake_content() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cas = CasStore::new(tmp.path().join("objects")).unwrap();
        let socket = tmp.path().join("socket");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();

        let captured = capture_package_files(
            "fixture",
            &[file_tuple(
                socket.to_str().unwrap(),
                0,
                0o140777,
                None,
                None,
            )],
            Some(&cas),
        )
        .unwrap();

        assert!(captured[0].content.is_none());
        assert!(matches!(
            captured[0].node.source.kind,
            PayloadNodeKind::Socket
        ));
    }

    #[test]
    fn full_adoption_validation_checks_inputs_without_creating_cas_state() {
        let tmp = tempfile::TempDir::new().unwrap();
        let source = tmp.path().join("preview-source");
        std::fs::write(&source, b"preview bytes").unwrap();
        let objects = tmp.path().join("objects");
        let files = vec![file_tuple(
            source.to_str().unwrap(),
            13,
            0o100644,
            None,
            None,
        )];

        validate_package_files("preview", &files).unwrap();

        assert!(!objects.exists());
    }

    #[test]
    fn full_adoption_missing_regular_file_fails_package_preparation() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cas = CasStore::new(tmp.path().join("objects")).unwrap();
        let files = vec![file_tuple(
            "/usr/bin/missing",
            7,
            0o100755,
            Some("package-manager-digest"),
            None,
        )];

        let error = capture_package_files("broken", &files, Some(&cas)).unwrap_err();

        assert_error_contains(
            error,
            &[
                "package broken",
                "/usr/bin/missing",
                "failed to capture exact node",
            ],
        );
    }

    #[test]
    fn native_optional_absence_is_omitted_from_the_live_payload_snapshot() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cas = CasStore::new(tmp.path().join("objects")).unwrap();
        let ghost = file_tuple_with_policy(
            tmp.path().join("absent-ghost").to_str().unwrap(),
            InstalledFileAbsencePolicy::RpmGhost,
        );
        let missing_ok = file_tuple_with_policy(
            tmp.path().join("absent-missing-ok").to_str().unwrap(),
            InstalledFileAbsencePolicy::RpmMissingOk,
        );

        let captured =
            capture_package_files("native-optional", &[ghost, missing_ok], Some(&cas)).unwrap();

        assert!(captured.is_empty());
        validate_package_files(
            "native-optional",
            &[file_tuple_with_policy(
                tmp.path().join("still-absent").to_str().unwrap(),
                InstalledFileAbsencePolicy::RpmGhostAndMissingOk,
            )],
        )
        .unwrap();
    }

    #[test]
    fn present_rpm_ghost_is_captured_as_live_payload() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("present-ghost");
        std::fs::write(&path, b"runtime state").unwrap();
        let cas = CasStore::new(tmp.path().join("objects")).unwrap();

        let captured = capture_package_files(
            "present-ghost",
            &[file_tuple_with_policy(
                path.to_str().unwrap(),
                InstalledFileAbsencePolicy::RpmGhost,
            )],
            Some(&cas),
        )
        .unwrap();

        assert_eq!(captured.len(), 1);
        assert_eq!(
            cas.retrieve(&captured[0].content.as_ref().unwrap().sha256)
                .unwrap(),
            b"runtime state"
        );
    }

    #[test]
    fn native_selected_root_ownership_is_not_removable_package_payload() {
        let tmp = tempfile::TempDir::new().unwrap();
        let below_root = tmp.path().join("owned-directory");
        std::fs::create_dir(&below_root).unwrap();
        let cas = CasStore::new(tmp.path().join("objects")).unwrap();
        let root = file_tuple("/", 0, 0o040755, None, None);
        let child = file_tuple(below_root.to_str().unwrap(), 0, 0o040755, None, None);

        let captured =
            capture_package_files("filesystem", &[root.clone(), child], Some(&cas)).unwrap();

        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].source.0, below_root.to_string_lossy());
        validate_package_files("filesystem", &[root]).unwrap();
    }
}
