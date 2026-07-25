// conary-core/src/generation/builder/runtime_inputs.rs

//! Exact projection of installed payload authority into generation artifacts.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use crate::ccs::manifest::FileCapability;
use crate::db::models::{FileEntry, InstallSource, InstalledFileCapability, Trove};
use crate::generation::root_manifest::{
    GENERATION_ROOT_MANIFEST_VERSION, GenerationRootEntry, GenerationRootManifest,
    MutableStateManifest, RootPathDomain, capture_root_node, classify_root_path,
};
use crate::payload::PayloadNodeKind;

use super::file_capabilities::{SECURITY_CAPABILITY_XATTR, encode_security_capability_xattr};

type RuntimeXattrs = BTreeMap<String, Vec<u8>>;
type CapabilityTargetKey = (i64, String);
type RuntimeCapabilityXattrs = HashMap<CapabilityTargetKey, RuntimeXattrs>;

pub(super) fn is_generation_input_source(source: InstallSource) -> bool {
    matches!(
        source,
        InstallSource::AdoptedFull
            | InstallSource::Taken
            | InstallSource::Repository
            | InstallSource::File
    )
}

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
    let mut immutable_entries = Vec::new();
    let mut state_entries = Vec::new();

    for file in files {
        let Some((package_name, source)) = trove_map.get(&file.trove_id) else {
            return Err(crate::Error::InternalError(format!(
                "orphaned file entry in generation input: path {} references missing trove_id {}",
                file.path, file.trove_id
            )));
        };
        if **source == InstallSource::AdoptedTrack || !is_generation_input_source((*source).clone())
        {
            continue;
        }

        let domain = classify_root_path(&file.path)?;
        if domain == RootPathDomain::EphemeralMountOrUser {
            continue;
        }

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
        match domain {
            RootPathDomain::Immutable => immutable_entries.push(entry),
            RootPathDomain::ConfigState | RootPathDomain::MutableState => {
                state_entries.push(entry);
            }
            RootPathDomain::EphemeralMountOrUser => unreachable!("handled above"),
        }
    }
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
        if !is_generation_input_source((*source).clone()) {
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
    use crate::db::models::{FileEntry, Trove, TroveType};
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
