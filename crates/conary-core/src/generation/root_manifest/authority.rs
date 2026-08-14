// conary-core/src/generation/root_manifest/authority.rs

//! Indexed, append-only selected-root snapshot authority.

use super::{
    CapturedSelectedRoot, GENERATION_ROOT_MANIFEST_VERSION, GenerationRootEntry,
    GenerationRootManifest, MutableStateManifest, PayloadNodeKind, RootPathDomain,
    SelectedRootManifestDelta, classify_root_path, validate_hardlinks, validate_mode_matches_kind,
};
use crate::payload::ResolvedPayloadNode;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// One already-validated selected-root snapshot stored in SQLite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectedRootSnapshot {
    id: i64,
}

/// Deterministic work performed while committing one manifest delta.
///
/// This is deliberately independent of timing so regression tests can prove
/// that a fixed non-hardlink upsert does not enumerate unchanged authority.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SelectedRootSnapshotWork {
    pub point_lookups: usize,
    pub subtree_entries: usize,
    pub hardlink_entries: usize,
}

#[derive(Debug)]
struct LayerEntry {
    remove_self: bool,
    remove_descendants: bool,
    entry: Option<GenerationRootEntry>,
}

impl SelectedRootSnapshot {
    pub fn id(self) -> i64 {
        self.id
    }

    /// Import one complete validated authority at an explicit seed boundary.
    pub fn capture(conn: &Connection, captured: &CapturedSelectedRoot) -> crate::Result<Self> {
        captured.generation.validate()?;
        captured.state.validate()?;
        with_savepoint(conn, "conary_selected_root_capture", || {
            let root_json = serde_json::to_string(&captured.generation.root)?;
            conn.execute(
                "INSERT INTO selected_root_snapshots (parent_id, root_node_json)
                 VALUES (NULL, ?1)",
                [root_json],
            )?;
            let snapshot = Self {
                id: conn.last_insert_rowid(),
            };
            for entry in captured
                .generation
                .entries
                .iter()
                .chain(&captured.state.entries)
            {
                insert_layer_entry(
                    conn,
                    snapshot,
                    &LayerEntry {
                        remove_self: false,
                        remove_descendants: false,
                        entry: Some(entry.clone()),
                    },
                    &entry.path,
                )?;
            }
            Ok(snapshot)
        })
    }

    /// Resolve an existing snapshot ID and validate its root authority.
    pub fn find(conn: &Connection, id: i64) -> crate::Result<Option<Self>> {
        let snapshot = conn
            .query_row(
                "SELECT id FROM selected_root_snapshots WHERE id = ?1",
                [id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .map(|id| Self { id });
        if let Some(snapshot) = snapshot {
            snapshot.root(conn)?;
        }
        Ok(snapshot)
    }

    pub fn root(self, conn: &Connection) -> crate::Result<ResolvedPayloadNode> {
        let raw: String = conn.query_row(
            "SELECT root_node_json FROM selected_root_snapshots WHERE id = ?1",
            [self.id],
            |row| row.get(0),
        )?;
        let root: ResolvedPayloadNode = serde_json::from_str(&raw)?;
        root.validate()
            .map_err(|error| crate::Error::InvalidPath(error.to_string()))?;
        if !matches!(root.source.kind, PayloadNodeKind::Directory) {
            return Err(crate::Error::InvalidPath(
                "selected-root snapshot root metadata must describe a directory".to_string(),
            ));
        }
        validate_mode_matches_kind("/", &root.source)?;
        Ok(root)
    }

    pub fn entry(
        self,
        conn: &Connection,
        path: &str,
    ) -> crate::Result<Option<GenerationRootEntry>> {
        effective_entry(conn, self, path)
    }

    /// Resolve the effective entries at and below one path.
    ///
    /// Callers use this only for transaction-owned recursive changes; fixed
    /// point updates do not enumerate unrelated authority.
    pub fn subtree(self, conn: &Connection, path: &str) -> crate::Result<Vec<GenerationRootEntry>> {
        effective_subtree(conn, self, path)
    }

    /// Resolve one effective hardlink group by its typed identity.
    pub fn hardlink_group(
        self,
        conn: &Connection,
        identity: &str,
    ) -> crate::Result<Vec<GenerationRootEntry>> {
        effective_hardlink_group(conn, self, identity)
    }

    /// Apply and persist only transaction-owned changed authority.
    pub fn apply_delta(
        self,
        conn: &Connection,
        delta: &SelectedRootManifestDelta,
    ) -> crate::Result<Self> {
        self.apply_delta_with_work(conn, delta)
            .map(|(snapshot, _)| snapshot)
    }

    pub fn apply_delta_with_work(
        self,
        conn: &Connection,
        delta: &SelectedRootManifestDelta,
    ) -> crate::Result<(Self, SelectedRootSnapshotWork)> {
        delta.validate()?;
        let prior_root = self.root(conn)?;
        let root = delta.root.as_ref().unwrap_or(&prior_root);
        root.validate()
            .map_err(|error| crate::Error::InvalidPath(error.to_string()))?;
        validate_mode_matches_kind("/", &root.source)?;

        let mut work = SelectedRootSnapshotWork::default();
        for path in &delta.removals {
            work.point_lookups += 1;
            if effective_entry(conn, self, path)?.is_none() {
                return Err(crate::Error::InvalidPath(format!(
                    "selected-root delta removal {path} does not name prior authority"
                )));
            }
        }
        for path in &delta.opaque_directories {
            work.point_lookups += 1;
            let Some(entry) = effective_entry(conn, self, path)? else {
                return Err(crate::Error::InvalidPath(format!(
                    "opaque directory {path} does not name prior authority"
                )));
            };
            if !matches!(entry.node.source.kind, PayloadNodeKind::Directory) {
                return Err(crate::Error::InvalidPath(format!(
                    "opaque directory {path} does not replace a prior directory"
                )));
            }
        }

        with_savepoint(conn, "conary_selected_root_apply", || {
            let root_json = serde_json::to_string(root)?;
            conn.execute(
                "INSERT INTO selected_root_snapshots (parent_id, root_node_json)
                 VALUES (?1, ?2)",
                params![self.id, root_json],
            )?;
            let next = Self {
                id: conn.last_insert_rowid(),
            };
            let layers = merge_delta_layers(delta);
            for (path, layer) in &layers {
                insert_layer_entry(conn, next, layer, path)?;
            }
            validate_changed_authority(conn, self, next, delta, &mut work)?;
            Ok((next, work))
        })
    }

    /// Derive the complete canonical artifact projection.
    pub fn materialize(self, conn: &Connection) -> crate::Result<CapturedSelectedRoot> {
        let root = self.root(conn)?;
        let lineage = lineage_ids(conn, self)?;
        let mut entries = BTreeMap::<String, GenerationRootEntry>::new();
        for snapshot_id in lineage.into_iter().rev() {
            let mut stmt = conn.prepare(
                "SELECT path, remove_self, remove_descendants, entry_json,
                        hardlink_identity
                 FROM selected_root_snapshot_entries
                 WHERE snapshot_id = ?1
                 ORDER BY path",
            )?;
            let rows = stmt.query_map([snapshot_id], read_layer_entry)?;
            for row in rows {
                let (path, layer) = row?;
                validate_layer(&path, &layer)?;
                if layer.remove_self {
                    entries.remove(&path);
                }
                if layer.remove_descendants {
                    let descendants = entries
                        .range(path.clone()..)
                        .skip_while(|(candidate, _)| candidate.as_str() == path)
                        .take_while(|(candidate, _)| is_descendant(candidate, &path))
                        .map(|(candidate, _)| candidate.clone())
                        .collect::<Vec<_>>();
                    for descendant in descendants {
                        entries.remove(&descendant);
                    }
                }
                if let Some(entry) = layer.entry {
                    entries.insert(path, entry);
                }
            }
        }
        let mut generation = Vec::new();
        let mut state = Vec::new();
        for entry in entries.into_values() {
            match classify_root_path(&entry.path)? {
                RootPathDomain::Immutable => generation.push(entry),
                RootPathDomain::ConfigState | RootPathDomain::MutableState => state.push(entry),
                RootPathDomain::EphemeralMountOrUser => {
                    return Err(crate::Error::InvalidPath(format!(
                        "selected-root snapshot produced ephemeral path {}",
                        entry.path
                    )));
                }
            }
        }
        let captured = CapturedSelectedRoot {
            generation: GenerationRootManifest {
                version: GENERATION_ROOT_MANIFEST_VERSION,
                root,
                entries: generation,
            },
            state: MutableStateManifest {
                version: GENERATION_ROOT_MANIFEST_VERSION,
                entries: state,
            },
        };
        captured.generation.validate()?;
        captured.state.validate()?;
        Ok(captured)
    }
}

fn with_savepoint<T>(
    conn: &Connection,
    name: &str,
    operation: impl FnOnce() -> crate::Result<T>,
) -> crate::Result<T> {
    conn.execute_batch(&format!("SAVEPOINT {name}"))?;
    match operation() {
        Ok(value) => {
            conn.execute_batch(&format!("RELEASE SAVEPOINT {name}"))?;
            Ok(value)
        }
        Err(error) => {
            conn.execute_batch(&format!(
                "ROLLBACK TO SAVEPOINT {name}; RELEASE SAVEPOINT {name}"
            ))?;
            Err(error)
        }
    }
}

fn merge_delta_layers(delta: &SelectedRootManifestDelta) -> BTreeMap<String, LayerEntry> {
    let mut layers = BTreeMap::<String, LayerEntry>::new();
    for path in &delta.removals {
        let layer = layers.entry(path.clone()).or_insert_with(empty_layer);
        layer.remove_self = true;
        layer.remove_descendants = true;
    }
    for path in &delta.opaque_directories {
        layers
            .entry(path.clone())
            .or_insert_with(empty_layer)
            .remove_descendants = true;
    }
    for entry in &delta.upserts {
        layers
            .entry(entry.path.clone())
            .or_insert_with(empty_layer)
            .entry = Some(entry.clone());
    }
    layers
}

fn empty_layer() -> LayerEntry {
    LayerEntry {
        remove_self: false,
        remove_descendants: false,
        entry: None,
    }
}

fn validate_changed_authority(
    conn: &Connection,
    prior: SelectedRootSnapshot,
    next: SelectedRootSnapshot,
    delta: &SelectedRootManifestDelta,
    work: &mut SelectedRootSnapshotWork,
) -> crate::Result<()> {
    let mut hardlink_identities = BTreeSet::<String>::new();
    let recursive = delta
        .removals
        .iter()
        .chain(&delta.opaque_directories)
        .collect::<BTreeSet<_>>();
    for path in recursive {
        let removed = effective_subtree(conn, prior, path)?;
        work.subtree_entries += removed.len();
        hardlink_identities.extend(removed.iter().filter_map(entry_hardlink_identity));
    }

    for entry in &delta.upserts {
        work.point_lookups += 1;
        if let Some(old) = effective_entry(conn, prior, &entry.path)?
            && let Some(identity) = entry_hardlink_identity(&old)
        {
            hardlink_identities.insert(identity);
        }
        if let Some(identity) = entry_hardlink_identity(entry) {
            hardlink_identities.insert(identity);
        }

        let parent = Path::new(&entry.path)
            .parent()
            .and_then(Path::to_str)
            .unwrap_or("/");
        if parent != "/" {
            work.point_lookups += 1;
            let Some(parent_entry) = effective_entry(conn, next, parent)? else {
                return Err(crate::Error::InvalidPath(format!(
                    "root manifest path {} has no explicit parent directory {parent}",
                    entry.path
                )));
            };
            if !matches!(parent_entry.node.source.kind, PayloadNodeKind::Directory) {
                return Err(crate::Error::InvalidPath(format!(
                    "root manifest path {} has non-directory parent {parent}",
                    entry.path
                )));
            }
        }
        if !matches!(entry.node.source.kind, PayloadNodeKind::Directory) {
            let descendants = effective_subtree(conn, next, &entry.path)?;
            work.subtree_entries += descendants.len();
            if descendants
                .iter()
                .any(|candidate| candidate.path != entry.path)
            {
                return Err(crate::Error::InvalidPath(format!(
                    "non-directory root manifest path {} retains descendants",
                    entry.path
                )));
            }
        }
    }

    for identity in hardlink_identities {
        let group = effective_hardlink_group(conn, next, &identity)?;
        work.hardlink_entries += group.len();
        if !group.is_empty() {
            validate_hardlinks(&group)?;
        }
    }
    Ok(())
}

fn effective_entry(
    conn: &Connection,
    snapshot: SelectedRootSnapshot,
    path: &str,
) -> crate::Result<Option<GenerationRootEntry>> {
    let candidates = path_and_ancestors(path);
    let placeholders = std::iter::repeat_n("?", candidates.len())
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "WITH RECURSIVE lineage(id, depth) AS (
             SELECT ?, 0
             UNION ALL
             SELECT snapshots.parent_id, lineage.depth + 1
             FROM selected_root_snapshots AS snapshots
             JOIN lineage ON snapshots.id = lineage.id
             WHERE snapshots.parent_id IS NOT NULL
         )
         SELECT entries.path, entries.remove_self,
                entries.remove_descendants, entries.entry_json,
                entries.hardlink_identity
         FROM selected_root_snapshot_entries AS entries
         JOIN lineage ON lineage.id = entries.snapshot_id
         WHERE entries.path IN ({placeholders})
           AND (entries.path = ? OR entries.remove_descendants = 1)
         ORDER BY lineage.depth ASC,
                  (entries.path = ?) DESC,
                  length(entries.path) DESC
         LIMIT 1"
    );
    let mut values = Vec::<&dyn rusqlite::ToSql>::with_capacity(candidates.len() + 3);
    values.push(&snapshot.id);
    values.extend(
        candidates
            .iter()
            .map(|candidate| candidate as &dyn rusqlite::ToSql),
    );
    values.push(&path);
    values.push(&path);
    let row = conn
        .query_row(&sql, values.as_slice(), read_layer_entry)
        .optional()?;
    let Some((stored_path, layer)) = row else {
        return Ok(None);
    };
    validate_layer(&stored_path, &layer)?;
    if stored_path == path {
        Ok(layer.entry)
    } else {
        Ok(None)
    }
}

fn path_and_ancestors(path: &str) -> Vec<String> {
    let mut candidates = vec![path.to_string()];
    let mut cursor = path;
    while let Some(separator) = cursor.rfind('/') {
        if separator == 0 {
            break;
        }
        cursor = &cursor[..separator];
        candidates.push(cursor.to_string());
    }
    candidates
}

fn effective_subtree(
    conn: &Connection,
    snapshot: SelectedRootSnapshot,
    path: &str,
) -> crate::Result<Vec<GenerationRootEntry>> {
    let lineage = lineage_ids(conn, snapshot)?;
    let placeholders = std::iter::repeat_n("?", lineage.len())
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT DISTINCT path FROM selected_root_snapshot_entries
         WHERE snapshot_id IN ({placeholders})
           AND path >= ? AND path < ?
         ORDER BY path"
    );
    let descendant_start = format!("{}/", path.trim_end_matches('/'));
    let descendant_end = format!("{}0", path.trim_end_matches('/'));
    let mut values = lineage
        .iter()
        .map(|id| id as &dyn rusqlite::ToSql)
        .collect::<Vec<_>>();
    values.push(&descendant_start);
    values.push(&descendant_end);
    let mut stmt = conn.prepare(&sql)?;
    let mut paths = stmt
        .query_map(values.as_slice(), |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if effective_entry(conn, snapshot, path)?.is_some() {
        paths.insert(0, path.to_string());
    }
    paths
        .into_iter()
        .filter_map(
            |candidate| match effective_entry(conn, snapshot, &candidate) {
                Ok(Some(entry)) => Some(Ok(entry)),
                Ok(None) => None,
                Err(error) => Some(Err(error)),
            },
        )
        .collect()
}

fn effective_hardlink_group(
    conn: &Connection,
    snapshot: SelectedRootSnapshot,
    identity: &str,
) -> crate::Result<Vec<GenerationRootEntry>> {
    let lineage = lineage_ids(conn, snapshot)?;
    let placeholders = std::iter::repeat_n("?", lineage.len())
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT DISTINCT path FROM selected_root_snapshot_entries
         WHERE snapshot_id IN ({placeholders}) AND hardlink_identity = ?
         ORDER BY path"
    );
    let mut values = lineage
        .iter()
        .map(|id| id as &dyn rusqlite::ToSql)
        .collect::<Vec<_>>();
    values.push(&identity);
    let mut stmt = conn.prepare(&sql)?;
    let paths = stmt
        .query_map(values.as_slice(), |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    paths
        .into_iter()
        .filter_map(|path| match effective_entry(conn, snapshot, &path) {
            Ok(Some(entry)) if entry_hardlink_identity(&entry).as_deref() == Some(identity) => {
                Some(Ok(entry))
            }
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
        .collect()
}

fn lineage_ids(conn: &Connection, snapshot: SelectedRootSnapshot) -> crate::Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "WITH RECURSIVE lineage(id, parent_id, depth) AS (
             SELECT id, parent_id, 0 FROM selected_root_snapshots WHERE id = ?1
             UNION ALL
             SELECT snapshots.id, snapshots.parent_id, lineage.depth + 1
             FROM selected_root_snapshots AS snapshots
             JOIN lineage ON snapshots.id = lineage.parent_id
         )
         SELECT id FROM lineage ORDER BY depth",
    )?;
    let ids = stmt
        .query_map([snapshot.id], |row| row.get::<_, i64>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if ids.is_empty() {
        return Err(crate::Error::NotFound(format!(
            "selected-root snapshot {} does not exist",
            snapshot.id
        )));
    }
    Ok(ids)
}

fn insert_layer_entry(
    conn: &Connection,
    snapshot: SelectedRootSnapshot,
    layer: &LayerEntry,
    path: &str,
) -> crate::Result<()> {
    validate_layer(path, layer)?;
    let entry_json = layer
        .entry
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;
    let hardlink_identity = layer.entry.as_ref().and_then(entry_hardlink_identity);
    conn.execute(
        "INSERT INTO selected_root_snapshot_entries (
             snapshot_id, path, remove_self, remove_descendants,
             entry_json, hardlink_identity
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            snapshot.id,
            path,
            layer.remove_self,
            layer.remove_descendants,
            entry_json,
            hardlink_identity,
        ],
    )?;
    Ok(())
}

fn read_layer_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<(String, LayerEntry)> {
    let path: String = row.get(0)?;
    let entry_json: Option<String> = row.get(3)?;
    let entry = entry_json
        .map(|raw| {
            serde_json::from_str(&raw).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    raw.len(),
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })
        })
        .transpose()?;
    let stored_identity: Option<String> = row.get(4)?;
    let derived_identity = entry.as_ref().and_then(entry_hardlink_identity);
    if stored_identity != derived_identity {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok((
        path,
        LayerEntry {
            remove_self: row.get(1)?,
            remove_descendants: row.get(2)?,
            entry,
        },
    ))
}

fn validate_layer(path: &str, layer: &LayerEntry) -> crate::Result<()> {
    if !layer.remove_self && !layer.remove_descendants && layer.entry.is_none() {
        return Err(crate::Error::InvalidPath(format!(
            "selected-root snapshot layer for {path} has no operation"
        )));
    }
    if let Some(entry) = &layer.entry {
        entry.validate()?;
        if entry.path != path {
            return Err(crate::Error::InvalidPath(format!(
                "selected-root snapshot layer path {path} disagrees with entry {}",
                entry.path
            )));
        }
        if classify_root_path(path)? == RootPathDomain::EphemeralMountOrUser {
            return Err(crate::Error::InvalidPath(format!(
                "selected-root snapshot cannot own ephemeral path {path}"
            )));
        }
    }
    Ok(())
}

fn entry_hardlink_identity(entry: &GenerationRootEntry) -> Option<String> {
    match &entry.node.source.kind {
        PayloadNodeKind::Regular {
            hardlink_identity: Some(identity),
        }
        | PayloadNodeKind::Hardlink { identity, .. } => Some(identity.clone()),
        _ => None,
    }
}

fn is_descendant(candidate: &str, ancestor: &str) -> bool {
    candidate
        .strip_prefix(ancestor)
        .is_some_and(|suffix| suffix.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::payload::{
        PayloadContentAuthority, PayloadIdentity, PayloadNode, ResolvedPayloadNode,
    };

    #[test]
    fn fixed_upsert_work_is_independent_of_unchanged_path_count() {
        let small = fixed_upsert_work(1_000);
        let large = fixed_upsert_work(100_000);
        assert_eq!(small, large);
        assert_eq!(small.subtree_entries, 1);
        assert_eq!(small.hardlink_entries, 0);
    }

    #[test]
    fn recursive_removal_and_opaque_replacement_are_exact() {
        let conn = connection();
        let base = SelectedRootSnapshot::capture(
            &conn,
            &captured(vec![
                directory("/opt"),
                directory("/opt/cache"),
                regular("/opt/cache/old", 1),
                directory("/usr"),
            ]),
        )
        .unwrap();
        let delta = SelectedRootManifestDelta {
            version: super::super::SELECTED_ROOT_MANIFEST_DELTA_VERSION,
            root: None,
            removals: Vec::new(),
            opaque_directories: vec!["/opt/cache".to_string()],
            upserts: vec![directory("/opt/cache"), regular("/opt/cache/new", 2)],
        };
        let next = base.apply_delta(&conn, &delta).unwrap();
        assert!(next.entry(&conn, "/opt/cache/old").unwrap().is_none());
        assert!(next.entry(&conn, "/opt/cache/new").unwrap().is_some());
        assert_eq!(next.materialize(&conn).unwrap().generation.entries.len(), 4);
    }

    #[test]
    fn broken_changed_parent_and_hardlink_fail_closed() {
        let conn = connection();
        let base = SelectedRootSnapshot::capture(
            &conn,
            &captured(vec![directory("/usr"), directory("/usr/lib")]),
        )
        .unwrap();
        let missing_parent = SelectedRootManifestDelta {
            version: super::super::SELECTED_ROOT_MANIFEST_DELTA_VERSION,
            root: None,
            removals: Vec::new(),
            opaque_directories: Vec::new(),
            upserts: vec![regular("/usr/bin/tool", 1)],
        };
        assert!(base.apply_delta(&conn, &missing_parent).is_err());

        let mut link = regular("/usr/lib/link", 1);
        link.content = None;
        link.node.source.kind = PayloadNodeKind::Hardlink {
            target: "/usr/lib/missing".to_string(),
            identity: "missing".to_string(),
        };
        let broken_link = SelectedRootManifestDelta {
            version: super::super::SELECTED_ROOT_MANIFEST_DELTA_VERSION,
            root: None,
            removals: Vec::new(),
            opaque_directories: Vec::new(),
            upserts: vec![link],
        };
        let snapshots_before: i64 = conn
            .query_row("SELECT COUNT(*) FROM selected_root_snapshots", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(base.apply_delta(&conn, &broken_link).is_err());
        let snapshots_after: i64 = conn
            .query_row("SELECT COUNT(*) FROM selected_root_snapshots", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(snapshots_after, snapshots_before);
    }

    #[test]
    fn persisted_snapshot_authority_is_append_only() {
        let conn = connection();
        let snapshot = SelectedRootSnapshot::capture(
            &conn,
            &captured(vec![directory("/usr"), directory("/usr/lib")]),
        )
        .unwrap();

        assert!(
            conn.execute(
                "UPDATE selected_root_snapshots SET root_node_json = '{}' WHERE id = ?1",
                [snapshot.id()],
            )
            .is_err()
        );
        assert!(
            conn.execute(
                "DELETE FROM selected_root_snapshot_entries WHERE snapshot_id = ?1",
                [snapshot.id()],
            )
            .is_err()
        );
        assert!(snapshot.materialize(&conn).is_ok());
    }

    fn fixed_upsert_work(paths: usize) -> SelectedRootSnapshotWork {
        let conn = connection();
        let mut entries = vec![directory("/usr"), directory("/usr/lib")];
        entries
            .extend((0..paths).map(|index| regular(&format!("/usr/lib/lib{index:06}.so"), index)));
        let base = SelectedRootSnapshot::capture(&conn, &captured(entries)).unwrap();
        let delta = SelectedRootManifestDelta {
            version: super::super::SELECTED_ROOT_MANIFEST_DELTA_VERSION,
            root: None,
            removals: Vec::new(),
            opaque_directories: Vec::new(),
            upserts: vec![regular("/usr/lib/lib000000.so", usize::MAX)],
        };
        base.apply_delta_with_work(&conn, &delta).unwrap().1
    }

    fn connection() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        db::schema::ensure_current(&conn).unwrap();
        conn
    }

    fn captured(mut entries: Vec<GenerationRootEntry>) -> CapturedSelectedRoot {
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        CapturedSelectedRoot {
            generation: GenerationRootManifest {
                version: GENERATION_ROOT_MANIFEST_VERSION,
                root: directory_node(),
                entries,
            },
            state: MutableStateManifest::empty(),
        }
    }

    fn directory(path: &str) -> GenerationRootEntry {
        GenerationRootEntry {
            path: path.to_string(),
            node: directory_node(),
            content: None,
        }
    }

    fn regular(path: &str, value: usize) -> GenerationRootEntry {
        GenerationRootEntry {
            path: path.to_string(),
            node: ResolvedPayloadNode::from_numeric_source(source_node()).unwrap(),
            content: Some(PayloadContentAuthority {
                sha256: format!("{value:064x}"),
                size: 1,
            }),
        }
    }

    fn directory_node() -> ResolvedPayloadNode {
        let mut node = source_node();
        node.kind = PayloadNodeKind::Directory;
        node.mode = libc::S_IFDIR | 0o755;
        ResolvedPayloadNode::from_numeric_source(node).unwrap()
    }

    fn source_node() -> PayloadNode {
        let mut node = PayloadNode::regular(0o644);
        node.user = PayloadIdentity::Numeric { id: 0 };
        node.group = PayloadIdentity::Numeric { id: 0 };
        node
    }
}
