// conary-core/src/generation/builder/runtime_inputs.rs

//! Exact projection of installed payload authority into generation artifacts.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use crate::ccs::manifest::FileCapability;
use crate::db::models::{DirectoryClaim, FileEntry, InstallSource, InstalledFileCapability, Trove};
use crate::generation::root_manifest::{
    GENERATION_ROOT_MANIFEST_VERSION, GenerationRootEntry, GenerationRootManifest,
    MutableStateManifest, RootPathDomain, capture_existing_payload_node, capture_root_node,
    classify_root_path,
};
use crate::payload::PayloadNodeKind;

use super::file_capabilities::{SECURITY_CAPABILITY_XATTR, encode_security_capability_xattr};

mod aliases;

type RuntimeXattrs = BTreeMap<String, Vec<u8>>;
type CapabilityTargetKey = (i64, String);
type RuntimeCapabilityXattrs = HashMap<CapabilityTargetKey, RuntimeXattrs>;

#[derive(Debug)]
pub(super) struct RuntimeGenerationInputs {
    pub generation: GenerationRootManifest,
    pub state: MutableStateManifest,
    pub adopted_track_count: usize,
}

impl RuntimeGenerationInputs {
    pub fn security_capability_xattr_count(&self) -> usize {
        self.generation
            .entries
            .iter()
            .chain(&self.state.entries)
            .filter(|entry| {
                entry
                    .node
                    .source
                    .xattrs
                    .contains_key(SECURITY_CAPABILITY_XATTR)
            })
            .count()
    }
}

pub(super) fn collect_runtime_generation_inputs(
    conn: &rusqlite::Connection,
    troves: &[Trove],
    files: Vec<FileEntry>,
    selected_root: &Path,
) -> crate::Result<RuntimeGenerationInputs> {
    let trove_map: HashMap<i64, (&str, &InstallSource)> = troves
        .iter()
        .filter_map(|trove| {
            trove
                .id
                .map(|id| (id, (trove.name.as_str(), &trove.install_source)))
        })
        .collect();
    let adopted_track_count = troves
        .iter()
        .filter(|trove| trove.install_source == InstallSource::AdoptedTrack)
        .count();
    let mut capability_xattrs = collect_runtime_capability_xattrs(conn, &trove_map, &files)?;
    let mut runtime_entries = Vec::new();
    let mut immutable_entries = Vec::new();
    let mut state_entries = Vec::new();

    for file in files {
        let Some((anchor_package_name, anchor_source)) = trove_map.get(&file.trove_id) else {
            return Err(crate::Error::InternalError(format!(
                "orphaned file entry in generation input: path {} references missing trove_id {}",
                file.path, file.trove_id
            )));
        };
        let mut generation_owner = anchor_source
            .is_generation_input()
            .then_some((*anchor_package_name, file.trove_id));
        for claim in DirectoryClaim::find_retaining_path(conn, &file.path)? {
            let Some((claim_package_name, claim_source)) = trove_map.get(&claim.trove_id) else {
                return Err(crate::Error::InternalError(format!(
                    "orphaned directory claim in generation input: path {} references missing trove_id {}",
                    file.path, claim.trove_id
                )));
            };
            if generation_owner.is_none() && claim_source.is_generation_input() {
                generation_owner = Some((*claim_package_name, claim.trove_id));
            }
        }
        let Some((package_name, _generation_trove_id)) = generation_owner else {
            continue;
        };

        let mut entry = GenerationRootEntry {
            path: file.path,
            node: file.node,
            content: file.content,
        };
        if let Some(capability) = capability_xattrs.remove(&(file.trove_id, entry.path.clone())) {
            merge_capability_xattrs(package_name, &mut entry, capability)?;
        }
        entry
            .validate()
            .map_err(|error| runtime_input_error(package_name, &entry.path, error.to_string()))?;
        runtime_entries.push(entry);
    }

    for entry in aliases::canonicalize_runtime_alias_paths(runtime_entries)? {
        let domain = classify_root_path(&entry.path)?;
        if domain == RootPathDomain::EphemeralMountOrUser {
            continue;
        }
        match domain {
            RootPathDomain::Immutable => immutable_entries.push(entry),
            RootPathDomain::ConfigState | RootPathDomain::MutableState => {
                state_entries.push(entry);
            }
            RootPathDomain::EphemeralMountOrUser => unreachable!("handled above"),
        }
    }
    add_exact_selected_root_parent_closure(
        &mut immutable_entries,
        &mut state_entries,
        selected_root,
    )?;
    immutable_entries.sort_by(|left, right| left.path.cmp(&right.path));
    state_entries.sort_by(|left, right| left.path.cmp(&right.path));

    let generation = GenerationRootManifest {
        version: GENERATION_ROOT_MANIFEST_VERSION,
        root: capture_root_node(selected_root)?,
        entries: immutable_entries,
    };
    let state = MutableStateManifest {
        version: GENERATION_ROOT_MANIFEST_VERSION,
        entries: state_entries,
    };
    generation.validate()?;
    state.validate()?;

    Ok(RuntimeGenerationInputs {
        generation,
        state,
        adopted_track_count,
    })
}

/// Close package-owned inputs over exact live parent-directory structure.
///
/// Native tooling may create a structural parent that no package claims. Such
/// a node belongs in the generation tree, but not in removable package
/// ownership. Capture it from the selected root instead of inventing metadata
/// or weakening the manifest's explicit-parent contract.
fn add_exact_selected_root_parent_closure(
    immutable_entries: &mut Vec<GenerationRootEntry>,
    state_entries: &mut Vec<GenerationRootEntry>,
    selected_root: &Path,
) -> crate::Result<()> {
    let entry_paths = immutable_entries
        .iter()
        .chain(state_entries.iter())
        .map(|entry| entry.path.clone())
        .collect::<Vec<_>>();
    let present = immutable_entries
        .iter()
        .chain(state_entries.iter())
        .map(|entry| (entry.path.clone(), entry.node.source.kind.clone()))
        .collect::<HashMap<_, _>>();
    let mut missing = BTreeSet::new();

    for entry_path in &entry_paths {
        let mut parent = Path::new(entry_path).parent();
        while let Some(parent_path) = parent {
            if parent_path == Path::new("/") {
                break;
            }
            let parent_text = parent_path.to_str().ok_or_else(|| {
                crate::Error::InvalidPath(format!(
                    "runtime generation parent for {} is not UTF-8",
                    entry_path
                ))
            })?;
            match present.get(parent_text) {
                Some(PayloadNodeKind::Directory) => {}
                Some(kind) => {
                    return Err(crate::Error::InvalidPath(format!(
                        "runtime generation path {} has non-directory parent {} ({})",
                        entry_path,
                        parent_text,
                        payload_kind_name(kind)
                    )));
                }
                None => {
                    missing.insert(parent_text.to_string());
                }
            }
            parent = parent_path.parent();
        }
    }

    for virtual_path in missing {
        let relative = virtual_path.strip_prefix('/').ok_or_else(|| {
            crate::Error::InvalidPath(format!(
                "runtime generation parent is not absolute: {virtual_path}"
            ))
        })?;
        let live_path = selected_root.join(relative);
        let node = capture_existing_payload_node(&live_path).map_err(|error| {
            crate::Error::InvalidPath(format!(
                "runtime generation cannot capture exact selected-root parent {virtual_path}: {error}"
            ))
        })?;
        if !matches!(node.source.kind, PayloadNodeKind::Directory) {
            return Err(crate::Error::InvalidPath(format!(
                "runtime generation selected-root parent {virtual_path} is {}, not a directory",
                payload_kind_name(&node.source.kind)
            )));
        }
        let entry = GenerationRootEntry {
            path: virtual_path,
            node,
            content: None,
        };
        match classify_root_path(&entry.path)? {
            RootPathDomain::Immutable => immutable_entries.push(entry),
            RootPathDomain::ConfigState | RootPathDomain::MutableState => {
                state_entries.push(entry);
            }
            RootPathDomain::EphemeralMountOrUser => {
                return Err(crate::Error::InvalidPath(format!(
                    "generation-authoritative path requires parent {} in the ephemeral/user domain",
                    entry.path
                )));
            }
        }
    }
    Ok(())
}

fn collect_runtime_capability_xattrs(
    conn: &rusqlite::Connection,
    trove_map: &HashMap<i64, (&str, &InstallSource)>,
    files: &[FileEntry],
) -> crate::Result<RuntimeCapabilityXattrs> {
    let capability_rows = InstalledFileCapability::find_all_ordered(conn).map_err(|error| {
        crate::Error::InternalError(format!(
            "failed to load installed file capabilities: {error}"
        ))
    })?;
    if capability_rows.is_empty() {
        return Ok(HashMap::new());
    }

    let files_by_trove_path = files
        .iter()
        .map(|file| ((file.trove_id, file.path.as_str()), file))
        .collect::<HashMap<_, _>>();
    let mut xattrs = HashMap::new();
    for row in capability_rows {
        let Some((package_name, source)) = trove_map.get(&row.trove_id) else {
            return Err(capability_input_error(
                "unknown",
                &row.path,
                format!(
                    "persisted capability references missing trove_id {}",
                    row.trove_id
                ),
            ));
        };
        if !source.is_generation_input() {
            return Err(capability_input_error(
                package_name,
                &row.path,
                format!(
                    "trove has non-generation install source {}",
                    source.as_str()
                ),
            ));
        }
        if classify_root_path(&row.path)? == RootPathDomain::EphemeralMountOrUser {
            return Err(capability_input_error(
                package_name,
                &row.path,
                "path belongs to the finite ephemeral/user domain",
            ));
        }
        let Some(file) = files_by_trove_path.get(&(row.trove_id, row.path.as_str())) else {
            return Err(capability_input_error(
                package_name,
                &row.path,
                "has no matching installed file entry",
            ));
        };
        ensure_capability_target_is_regular(package_name, file)?;
        let capability = FileCapability {
            path: row.path.clone(),
            capabilities: row.capabilities.clone(),
            permitted: row.permitted,
            effective: row.effective,
            inheritable: row.inheritable,
        };
        let encoded = encode_security_capability_xattr(&capability)?;
        xattrs.insert(
            (row.trove_id, row.path),
            BTreeMap::from([(SECURITY_CAPABILITY_XATTR.to_string(), encoded)]),
        );
    }
    Ok(xattrs)
}

fn ensure_capability_target_is_regular(package_name: &str, file: &FileEntry) -> crate::Result<()> {
    if matches!(file.node.source.kind, PayloadNodeKind::Regular { .. }) {
        Ok(())
    } else {
        Err(capability_input_error(
            package_name,
            &file.path,
            format!(
                "file capability target is {}, not a regular file",
                payload_kind_name(&file.node.source.kind)
            ),
        ))
    }
}

fn merge_capability_xattrs(
    package_name: &str,
    entry: &mut GenerationRootEntry,
    capability: RuntimeXattrs,
) -> crate::Result<()> {
    for (name, value) in capability {
        if let Some(existing) = entry.node.source.xattrs.get(&name)
            && existing != &value
        {
            return Err(capability_input_error(
                package_name,
                &entry.path,
                format!("persisted {name} conflicts with payload-node xattr authority"),
            ));
        }
        entry.node.source.xattrs.insert(name, value);
    }
    Ok(())
}

fn payload_kind_name(kind: &PayloadNodeKind) -> &'static str {
    match kind {
        PayloadNodeKind::Regular { .. } => "regular",
        PayloadNodeKind::Directory => "directory",
        PayloadNodeKind::Symlink { .. } => "symlink",
        PayloadNodeKind::Hardlink { .. } => "hardlink",
        PayloadNodeKind::BlockDevice { .. } => "block-device",
        PayloadNodeKind::CharacterDevice { .. } => "character-device",
        PayloadNodeKind::Fifo => "fifo",
        PayloadNodeKind::Socket => "socket",
    }
}

fn runtime_input_error(
    package_name: &str,
    path: &str,
    detail: impl std::fmt::Display,
) -> crate::Error {
    crate::Error::InvalidPath(format!(
        "exact runtime generation input is invalid: package {package_name} path {path}: {detail}"
    ))
}

fn capability_input_error(
    package_name: &str,
    path: &str,
    detail: impl std::fmt::Display,
) -> crate::Error {
    crate::Error::InvalidPath(format!(
        "runtime generation cannot preserve file capability: package {package_name} path {path}: {detail}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    use crate::db::models::{DirectoryClaimAnchorPolicy, FileEntry, Trove, TroveType};
    use crate::db::testing::create_test_db;
    use crate::payload::{
        PayloadContentAuthority, PayloadNode, PayloadNodeKind, ResolvedPayloadNode,
    };

    fn resolved(node: PayloadNode) -> ResolvedPayloadNode {
        ResolvedPayloadNode::from_numeric_source(node).unwrap()
    }

    fn trove(id: i64, name: &str, source: InstallSource) -> Trove {
        let mut trove = Trove::new_with_source(
            name.to_string(),
            "1.0-1".to_string(),
            TroveType::Package,
            source,
            crate::repository::versioning::VersionScheme::Conary,
        );
        trove.id = Some(id);
        trove
    }

    fn directory(path: &str, trove_id: i64) -> FileEntry {
        let mut node = PayloadNode::regular(0o755);
        node.kind = PayloadNodeKind::Directory;
        node.mode = libc::S_IFDIR | 0o755;
        FileEntry::new(path.to_string(), resolved(node), None, trove_id)
    }

    fn regular(path: &str, bytes: &[u8], trove_id: i64) -> FileEntry {
        FileEntry::new(
            path.to_string(),
            resolved(PayloadNode::regular(0o755)),
            Some(PayloadContentAuthority {
                sha256: crate::hash::sha256(bytes),
                size: bytes.len() as u64,
            }),
            trove_id,
        )
    }

    fn symlink(path: &str, target: &str, trove_id: i64) -> FileEntry {
        let mut node = PayloadNode::regular(0o777);
        node.kind = PayloadNodeKind::Symlink {
            target: target.to_string(),
        };
        node.mode = libc::S_IFLNK | 0o777;
        FileEntry::new(path.to_string(), resolved(node), None, trove_id)
    }

    fn selected_root() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn exact_nodes_split_into_immutable_and_state_domains() {
        let (_tmp, conn) = create_test_db();
        let root = selected_root();
        let troves = vec![trove(1, "runtime", InstallSource::Repository)];
        let files = vec![
            directory("/etc", 1),
            regular("/etc/runtime.conf", b"state", 1),
            directory("/opt", 1),
            regular("/opt/runtime", b"immutable", 1),
            directory("/tmp", 1),
            regular("/tmp/ignored", b"ephemeral", 1),
        ];

        let inputs = collect_runtime_generation_inputs(&conn, &troves, files, root.path()).unwrap();

        assert_eq!(
            inputs
                .generation
                .entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            ["/opt", "/opt/runtime"]
        );
        assert_eq!(
            inputs
                .state
                .entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            ["/etc", "/etc/runtime.conf"]
        );
    }

    #[test]
    fn exact_live_parent_directories_close_immutable_inputs() {
        let (_tmp, conn) = create_test_db();
        let root = selected_root();
        std::fs::create_dir_all(root.path().join("boot/loader/entries")).unwrap();
        std::fs::set_permissions(
            root.path().join("boot/loader"),
            std::fs::Permissions::from_mode(0o711),
        )
        .unwrap();
        let troves = vec![trove(1, "bootloader", InstallSource::AdoptedFull)];
        let files = vec![directory("/boot", 1), directory("/boot/loader/entries", 1)];

        let inputs = collect_runtime_generation_inputs(&conn, &troves, files, root.path()).unwrap();

        assert_eq!(
            inputs
                .generation
                .entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            ["/boot", "/boot/loader", "/boot/loader/entries"]
        );
        let parent = inputs
            .generation
            .entries
            .iter()
            .find(|entry| entry.path == "/boot/loader")
            .unwrap();
        assert!(matches!(
            parent.node.source.kind,
            PayloadNodeKind::Directory
        ));
        assert_eq!(parent.node.source.mode & 0o7777, 0o711);
        assert!(parent.content.is_none());
    }

    #[test]
    fn exact_live_parent_directories_follow_the_child_state_domain() {
        let (_tmp, conn) = create_test_db();
        let root = selected_root();
        std::fs::create_dir_all(root.path().join("etc/runtime")).unwrap();
        let troves = vec![trove(1, "runtime", InstallSource::AdoptedFull)];
        let files = vec![regular("/etc/runtime/config", b"state", 1)];

        let inputs = collect_runtime_generation_inputs(&conn, &troves, files, root.path()).unwrap();

        assert!(inputs.generation.entries.is_empty());
        assert_eq!(
            inputs
                .state
                .entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            ["/etc", "/etc/runtime", "/etc/runtime/config"]
        );
    }

    #[test]
    fn absent_exact_live_parent_directory_is_rejected() {
        let (_tmp, conn) = create_test_db();
        let root = selected_root();
        let troves = vec![trove(1, "runtime", InstallSource::AdoptedFull)];
        let files = vec![regular("/opt/missing/tool", b"tool", 1)];

        let error =
            collect_runtime_generation_inputs(&conn, &troves, files, root.path()).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("cannot capture exact selected-root parent /opt")
        );
    }

    #[test]
    fn non_directory_exact_live_parent_is_rejected() {
        let (_tmp, conn) = create_test_db();
        let root = selected_root();
        std::fs::write(root.path().join("opt"), b"not a directory").unwrap();
        let troves = vec![trove(1, "runtime", InstallSource::AdoptedFull)];
        let files = vec![regular("/opt/tool", b"tool", 1)];

        let error =
            collect_runtime_generation_inputs(&conn, &troves, files, root.path()).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("selected-root parent /opt is regular, not a directory")
        );
    }

    #[test]
    fn special_nodes_remain_typed_generation_inputs() {
        let (_tmp, conn) = create_test_db();
        let root = selected_root();
        let troves = vec![trove(1, "runtime", InstallSource::Repository)];
        let mut fifo = PayloadNode::regular(0o640);
        fifo.kind = PayloadNodeKind::Fifo;
        fifo.mode = libc::S_IFIFO | 0o640;
        let files = vec![
            directory("/usr", 1),
            FileEntry::new("/usr/events".to_string(), resolved(fifo), None, 1),
        ];

        let inputs = collect_runtime_generation_inputs(&conn, &troves, files, root.path()).unwrap();
        assert!(matches!(
            inputs.generation.entries[1].node.source.kind,
            PayloadNodeKind::Fifo
        ));
    }

    #[test]
    fn runtime_projection_never_synthesizes_usr_merge_or_abi_links() {
        let (_tmp, conn) = create_test_db();
        let root = selected_root();
        let troves = vec![trove(1, "glibc", InstallSource::Repository)];
        let files = vec![
            directory("/usr", 1),
            directory("/usr/lib", 1),
            regular("/usr/lib/ld-linux-x86-64.so.2", b"loader", 1),
        ];

        let inputs = collect_runtime_generation_inputs(&conn, &troves, files, root.path()).unwrap();
        assert!(
            inputs
                .generation
                .entries
                .iter()
                .all(|entry| entry.path != "/lib64")
        );
    }

    #[test]
    fn descendants_of_owned_usr_merge_links_use_materialized_paths() {
        let (_tmp, conn) = create_test_db();
        let root = selected_root();
        let troves = vec![trove(1, "filesystem", InstallSource::AdoptedFull)];
        let files = vec![
            symlink("/lib", "usr/lib", 1),
            directory("/lib/modules", 1),
            directory("/usr", 1),
            directory("/usr/lib", 1),
        ];

        let inputs = collect_runtime_generation_inputs(&conn, &troves, files, root.path()).unwrap();

        assert_eq!(
            inputs
                .generation
                .entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            ["/lib", "/usr", "/usr/lib", "/usr/lib/modules"]
        );
    }

    #[test]
    fn nested_owned_symlink_ancestors_resolve_from_exact_manifest_authority() {
        let (_tmp, conn) = create_test_db();
        let root = selected_root();
        let troves = vec![trove(1, "filesystem", InstallSource::AdoptedFull)];
        let files = vec![
            symlink("/alias", "real", 1),
            symlink("/alias/link", "nested", 1),
            regular("/alias/link/tool", b"tool", 1),
            directory("/real", 1),
            directory("/real/nested", 1),
        ];

        let inputs = collect_runtime_generation_inputs(&conn, &troves, files, root.path()).unwrap();

        assert_eq!(
            inputs
                .generation
                .entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            [
                "/alias",
                "/real",
                "/real/link",
                "/real/nested",
                "/real/nested/tool"
            ]
        );
    }

    #[test]
    fn equivalent_alias_and_materialized_authority_coalesce() {
        let (_tmp, conn) = create_test_db();
        let root = selected_root();
        let troves = vec![trove(1, "filesystem", InstallSource::AdoptedFull)];
        let files = vec![
            symlink("/lib", "usr/lib", 1),
            directory("/lib/modules", 1),
            directory("/usr", 1),
            directory("/usr/lib", 1),
            directory("/usr/lib/modules", 1),
        ];

        let inputs = collect_runtime_generation_inputs(&conn, &troves, files, root.path()).unwrap();

        assert_eq!(
            inputs
                .generation
                .entries
                .iter()
                .filter(|entry| entry.path == "/usr/lib/modules")
                .count(),
            1
        );
    }

    #[test]
    fn conflicting_alias_and_materialized_authority_is_rejected() {
        let (_tmp, conn) = create_test_db();
        let root = selected_root();
        let troves = vec![trove(1, "filesystem", InstallSource::AdoptedFull)];
        let mut alias_directory = directory("/lib/modules", 1);
        alias_directory.node.source.mode = libc::S_IFDIR | 0o700;
        let files = vec![
            symlink("/lib", "usr/lib", 1),
            alias_directory,
            directory("/usr", 1),
            directory("/usr/lib", 1),
            directory("/usr/lib/modules", 1),
        ];

        let error =
            collect_runtime_generation_inputs(&conn, &troves, files, root.path()).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("with conflicting exact payload authority")
        );
    }

    #[test]
    fn alias_projection_reclassifies_the_materialized_path_domain() {
        let (_tmp, conn) = create_test_db();
        let root = selected_root();
        let troves = vec![trove(1, "configuration", InstallSource::AdoptedFull)];
        let files = vec![
            symlink("/configuration", "etc", 1),
            regular("/configuration/app.conf", b"state", 1),
            directory("/etc", 1),
        ];

        let inputs = collect_runtime_generation_inputs(&conn, &troves, files, root.path()).unwrap();

        assert_eq!(
            inputs
                .generation
                .entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            ["/configuration"]
        );
        assert_eq!(
            inputs
                .state
                .entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            ["/etc", "/etc/app.conf"]
        );
    }

    #[test]
    fn alias_projection_rejects_manifest_symlink_escape() {
        let (_tmp, conn) = create_test_db();
        let root = selected_root();
        let troves = vec![trove(1, "filesystem", InstallSource::AdoptedFull)];
        let files = vec![
            symlink("/lib", "../../outside", 1),
            directory("/lib/modules", 1),
        ];

        let error =
            collect_runtime_generation_inputs(&conn, &troves, files, root.path()).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("cannot resolve exact manifest symlink ancestry"),
            "{error}"
        );
    }

    #[test]
    fn exact_symlinks_are_preserved_without_mode_or_hash_inference() {
        let (_tmp, conn) = create_test_db();
        let root = selected_root();
        let troves = vec![trove(1, "init", InstallSource::Repository)];
        let files = vec![
            directory("/sbin", 1),
            symlink("/sbin/init", "../usr/lib/systemd/systemd", 1),
        ];

        let inputs = collect_runtime_generation_inputs(&conn, &troves, files, root.path()).unwrap();
        assert!(matches!(
            &inputs.generation.entries[1].node.source.kind,
            PayloadNodeKind::Symlink { target }
                if target == "../usr/lib/systemd/systemd"
        ));
    }

    #[test]
    fn adopted_track_is_counted_but_not_generation_authority() {
        let (_tmp, conn) = create_test_db();
        let root = selected_root();
        let troves = vec![trove(1, "tracked", InstallSource::AdoptedTrack)];
        let files = vec![regular("/tracked", b"tracked", 1)];

        let inputs = collect_runtime_generation_inputs(&conn, &troves, files, root.path()).unwrap();
        assert_eq!(inputs.adopted_track_count, 1);
        assert!(inputs.generation.entries.is_empty());
    }

    #[test]
    fn captured_root_is_exact_generation_and_mutable_state_authority() {
        let (_tmp, conn) = create_test_db();
        let root = selected_root();
        let troves = vec![trove(1, "captured-root", InstallSource::CapturedRoot)];
        let files = vec![
            directory("/etc", 1),
            regular("/etc/sshd_config", b"configured", 1),
            directory("/usr", 1),
            regular("/usr/local-state", b"captured", 1),
        ];

        let inputs = collect_runtime_generation_inputs(&conn, &troves, files, root.path()).unwrap();

        assert_eq!(
            inputs
                .generation
                .entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            ["/usr", "/usr/local-state"]
        );
        assert_eq!(
            inputs
                .state
                .entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            ["/etc", "/etc/sshd_config"]
        );
    }

    #[test]
    fn generation_claimant_keeps_a_non_generation_anchor_in_the_root() {
        let (_tmp, conn) = create_test_db();
        let root = selected_root();
        let mut anchor_trove = Trove::new_with_source(
            "tracked-anchor".to_string(),
            "1".to_string(),
            TroveType::Package,
            InstallSource::AdoptedTrack,
            crate::repository::versioning::VersionScheme::Arch,
        );
        anchor_trove.architecture = Some("x86_64".to_string());
        anchor_trove.native_package_identity = Some(
            crate::packages::InstalledPackageIdentity::pacman(
                "tracked-anchor",
                "tracked-anchor",
                "1",
                "x86_64",
            )
            .unwrap(),
        );
        let anchor_trove_id = anchor_trove.insert(&conn).unwrap();
        let mut claimant_trove = Trove::new_with_source(
            "repository-claimant".to_string(),
            "1".to_string(),
            TroveType::Package,
            InstallSource::Repository,
            crate::repository::versioning::VersionScheme::Conary,
        );
        let claimant_trove_id = claimant_trove.insert(&conn).unwrap();

        let directory_node = directory("/shared-directory", anchor_trove_id).node;
        let mut directory_anchor = FileEntry::new(
            "/shared-directory".to_string(),
            directory_node.clone(),
            None,
            anchor_trove_id,
        );
        directory_anchor.insert(&conn).unwrap();
        DirectoryClaim::new(
            "/shared-directory".to_string(),
            claimant_trove_id,
            directory_node,
        )
        .unwrap()
        .insert(&conn)
        .unwrap();

        let symlink_node = symlink("/shared-link", "shared-directory", anchor_trove_id).node;
        let mut symlink_anchor = FileEntry::new(
            "/shared-link".to_string(),
            symlink_node,
            None,
            anchor_trove_id,
        );
        symlink_anchor.insert(&conn).unwrap();
        DirectoryClaim::new(
            "/shared-link".to_string(),
            claimant_trove_id,
            directory("/shared-link", claimant_trove_id).node,
        )
        .unwrap()
        .with_anchor_policy(DirectoryClaimAnchorPolicy::DirectoryOrSymlinkToDirectory)
        .insert(&conn)
        .unwrap();

        let inputs = collect_runtime_generation_inputs(
            &conn,
            &Trove::list_packages(&conn).unwrap(),
            FileEntry::find_all_ordered(&conn).unwrap(),
            root.path(),
        )
        .unwrap();

        assert_eq!(
            inputs
                .generation
                .entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            ["/shared-directory", "/shared-link"]
        );
    }

    #[test]
    fn orphaned_file_authority_is_rejected() {
        let (_tmp, conn) = create_test_db();
        let root = selected_root();
        let error = collect_runtime_generation_inputs(
            &conn,
            &[trove(1, "runtime", InstallSource::Repository)],
            vec![regular("/orphan", b"orphan", 99)],
            root.path(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("orphaned file entry"));
    }
}
