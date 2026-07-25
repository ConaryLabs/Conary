// conary-core/src/db/models/file_entry.rs

//! Exact installed payload-node state.

use crate::error::{Error, Result};
use crate::payload::{PayloadContentAuthority, PayloadNodeKind, ResolvedPayloadNode};
use rusqlite::{Connection, OptionalExtension, Row, params, types::Type};

/// One package-owned node in the selected filesystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub id: Option<i64>,
    pub path: String,
    pub node: ResolvedPayloadNode,
    pub content: Option<PayloadContentAuthority>,
    pub trove_id: i64,
    pub installed_at: Option<String>,
    pub component_id: Option<i64>,
}

impl FileEntry {
    const COLUMNS: &'static str = "id, path, payload_node_json, content_sha256, \
         content_size, trove_id, installed_at, component_id";

    pub fn new(
        path: String,
        node: ResolvedPayloadNode,
        content: Option<PayloadContentAuthority>,
        trove_id: i64,
    ) -> Self {
        Self {
            id: None,
            path,
            node,
            content,
            trove_id,
            installed_at: None,
            component_id: None,
        }
    }

    pub fn new_with_component(
        path: String,
        node: ResolvedPayloadNode,
        content: Option<PayloadContentAuthority>,
        trove_id: i64,
        component_id: i64,
    ) -> Self {
        Self {
            component_id: Some(component_id),
            ..Self::new(path, node, content, trove_id)
        }
    }

    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        self.validate()?;
        let node_json = canonical_node_json(&self.node)?;
        let content_size = persisted_content_size(self.content.as_ref())?;
        conn.execute(
            "INSERT INTO files (
                path, payload_node_json, content_sha256, content_size,
                trove_id, component_id
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                &self.path,
                node_json,
                self.content.as_ref().map(|content| &content.sha256),
                content_size,
                self.trove_id,
                self.component_id,
            ],
        )?;
        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Insert or replace this path's exact installed authority.
    pub fn insert_or_replace(&mut self, conn: &Connection) -> Result<i64> {
        self.validate()?;
        let node_json = canonical_node_json(&self.node)?;
        let content_size = persisted_content_size(self.content.as_ref())?;
        conn.execute(
            "INSERT OR REPLACE INTO files (
                path, payload_node_json, content_sha256, content_size,
                trove_id, component_id
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                &self.path,
                node_json,
                self.content.as_ref().map(|content| &content.sha256),
                content_size,
                self.trove_id,
                self.component_id,
            ],
        )?;
        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    pub fn find_by_path(conn: &Connection, path: &str) -> Result<Option<Self>> {
        let sql = format!("SELECT {} FROM files WHERE path = ?1", Self::COLUMNS);
        conn.query_row(&sql, [path], Self::from_row)
            .optional()
            .map_err(Into::into)
    }

    pub fn find_by_trove(conn: &Connection, trove_id: i64) -> Result<Vec<Self>> {
        let sql = format!("SELECT {} FROM files WHERE trove_id = ?1", Self::COLUMNS);
        let mut stmt = conn.prepare(&sql)?;
        Ok(stmt
            .query_map([trove_id], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn find_by_component(conn: &Connection, component_id: i64) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM files WHERE component_id = ?1",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        Ok(stmt
            .query_map([component_id], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn find_all_ordered(conn: &Connection) -> Result<Vec<Self>> {
        let sql = format!("SELECT {} FROM files ORDER BY path", Self::COLUMNS);
        let mut stmt = conn.prepare(&sql)?;
        Ok(stmt
            .query_map([], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn find_by_path_pattern(conn: &Connection, pattern: &str) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM files WHERE path LIKE ?1 ORDER BY path",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        Ok(stmt
            .query_map([pattern], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn list_files_lsl(conn: &Connection, trove_id: i64) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM files WHERE trove_id = ?1 ORDER BY path",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        Ok(stmt
            .query_map([trove_id], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn format_permissions(&self) -> String {
        let mode = self.node.source.mode;
        let file_type = match &self.node.source.kind {
            PayloadNodeKind::Regular { .. } => '-',
            PayloadNodeKind::Directory => 'd',
            PayloadNodeKind::Symlink { .. } => 'l',
            PayloadNodeKind::Hardlink { .. } => 'h',
            PayloadNodeKind::BlockDevice { .. } => 'b',
            PayloadNodeKind::CharacterDevice { .. } => 'c',
            PayloadNodeKind::Fifo => 'p',
            PayloadNodeKind::Socket => 's',
        };
        let bit = |mask, set, unset| if mode & mask != 0 { set } else { unset };
        let owner_x = if mode & 0o4000 != 0 {
            bit(0o100, 's', 'S')
        } else {
            bit(0o100, 'x', '-')
        };
        let group_x = if mode & 0o2000 != 0 {
            bit(0o010, 's', 'S')
        } else {
            bit(0o010, 'x', '-')
        };
        let other_x = if mode & 0o1000 != 0 {
            bit(0o001, 't', 'T')
        } else {
            bit(0o001, 'x', '-')
        };
        format!(
            "{}{}{}{}{}{}{}{}{}{}",
            file_type,
            bit(0o400, 'r', '-'),
            bit(0o200, 'w', '-'),
            owner_x,
            bit(0o040, 'r', '-'),
            bit(0o020, 'w', '-'),
            group_x,
            bit(0o004, 'r', '-'),
            bit(0o002, 'w', '-'),
            other_x
        )
    }

    pub fn size_human(&self) -> String {
        let size = self
            .content
            .as_ref()
            .and_then(|content| i64::try_from(content.size).ok())
            .unwrap_or(0);
        super::format_size(size)
    }

    pub fn delete(conn: &Connection, path: &str) -> Result<()> {
        conn.execute("DELETE FROM files WHERE path = ?1", [path])?;
        Ok(())
    }

    pub fn batch_insert(conn: &Connection, entries: &[Self]) -> Result<usize> {
        if entries.is_empty() {
            return Ok(0);
        }
        let mut stmt = conn.prepare_cached(
            "INSERT OR REPLACE INTO files (
                path, payload_node_json, content_sha256, content_size,
                trove_id, component_id
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        for entry in entries {
            entry.validate()?;
            let node_json = canonical_node_json(&entry.node)?;
            let content_size = persisted_content_size(entry.content.as_ref())?;
            stmt.execute(params![
                &entry.path,
                node_json,
                entry.content.as_ref().map(|content| &content.sha256),
                content_size,
                entry.trove_id,
                entry.component_id,
            ])?;
        }
        Ok(entries.len())
    }

    fn validate(&self) -> Result<()> {
        self.node
            .validate()
            .and_then(|()| self.node.source.validate_content(self.content.as_ref()))
            .map_err(|error| {
                Error::ParseError(format!(
                    "installed payload authority for {} is invalid: {error}",
                    self.path
                ))
            })
    }

    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        let node_json: String = row.get(2)?;
        let node = serde_json::from_str::<ResolvedPayloadNode>(&node_json).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(2, Type::Text, error.into())
        })?;
        node.validate().map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(2, Type::Text, error.into())
        })?;
        let sha256: Option<String> = row.get(3)?;
        let size: Option<i64> = row.get(4)?;
        let content = match (sha256, size) {
            (Some(sha256), Some(size)) => Some(PayloadContentAuthority {
                sha256,
                size: u64::try_from(size).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(4, Type::Integer, error.into())
                })?,
            }),
            (None, None) => None,
            _ => {
                return Err(rusqlite::Error::FromSqlConversionFailure(
                    3,
                    Type::Text,
                    "installed content digest and size must both be NULL or both be present".into(),
                ));
            }
        };
        node.source
            .validate_content(content.as_ref())
            .map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(3, Type::Text, error.into())
            })?;
        Ok(Self {
            id: Some(row.get(0)?),
            path: row.get(1)?,
            node,
            content,
            trove_id: row.get(5)?,
            installed_at: row.get(6)?,
            component_id: row.get(7)?,
        })
    }
}

fn canonical_node_json(node: &ResolvedPayloadNode) -> Result<String> {
    let bytes = crate::ccs::attestation::canonical_json_bytes(node).map_err(|error| {
        Error::InternalError(format!("canonicalize installed payload node: {error}"))
    })?;
    String::from_utf8(bytes).map_err(|error| {
        Error::InternalError(format!("canonical payload node is not UTF-8: {error}"))
    })
}

fn persisted_content_size(content: Option<&PayloadContentAuthority>) -> Result<Option<i64>> {
    content
        .map(|content| {
            i64::try_from(content.size).map_err(|_| {
                Error::ParseError("payload content size exceeds SQLite INTEGER".to_string())
            })
        })
        .transpose()
}
