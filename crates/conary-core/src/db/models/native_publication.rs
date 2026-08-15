// conary-core/src/db/models/native_publication.rs

//! Native CCS publication model.

use crate::error::Result;
use rusqlite::{Connection, Row, params};

pub const NATIVE_NOARCH: &str = "noarch";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativePublicationStatus {
    Public,
    Superseded,
    RolledBack,
}

impl NativePublicationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Superseded => "superseded",
            Self::RolledBack => "rolled_back",
        }
    }

    pub fn from_db(value: &str, column: usize) -> rusqlite::Result<Self> {
        match value {
            "public" => Ok(Self::Public),
            "superseded" => Ok(Self::Superseded),
            "rolled_back" => Ok(Self::RolledBack),
            other => Err(rusqlite::Error::FromSqlConversionFailure(
                column,
                rusqlite::types::Type::Text,
                format!("invalid native publication status {other}").into(),
            )),
        }
    }
}

pub fn normalize_native_architecture(architecture: Option<&str>) -> String {
    architecture
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(NATIVE_NOARCH)
        .to_string()
}

#[derive(Debug, Clone)]
pub struct NativePackagePublication {
    pub id: Option<i64>,
    pub repository_id: i64,
    pub repository_package_id: i64,
    pub source_profile: String,
    pub name: String,
    pub version: String,
    pub package_release: String,
    pub architecture: String,
    pub package_kind: String,
    pub authority_format_version: i64,
    pub status: NativePublicationStatus,
    pub content_hash: String,
    pub transport_json: String,
    pub total_size: i64,
    pub package_path: String,
    pub target_path: String,
    pub trust_status: String,
}

impl NativePackagePublication {
    const COLUMNS: &'static str = "id, repository_id, repository_package_id, source_profile, name, \
         version, package_release, architecture, package_kind, authority_format_version, \
         status, content_hash, transport_json, total_size, package_path, target_path, \
         trust_status";

    pub fn find_active(
        conn: &Connection,
        source_profile: &str,
        name: &str,
        version: Option<&str>,
        package_release: Option<&str>,
        architecture: Option<&str>,
    ) -> Result<Vec<Self>> {
        Self::require_public_source_profile(source_profile)?;
        let mut sql = format!(
            "SELECT {} FROM native_package_publications \
             WHERE status = 'public' AND source_profile = ?1 AND name = ?2",
            Self::COLUMNS
        );
        let mut values: Vec<String> = vec![source_profile.to_string(), name.to_string()];
        if let Some(version) = version {
            values.push(version.to_string());
            sql.push_str(&format!(" AND version = ?{}", values.len()));
        }
        if let Some(package_release) = package_release {
            values.push(package_release.to_string());
            sql.push_str(&format!(" AND package_release = ?{}", values.len()));
        }
        if let Some(architecture) = architecture {
            values.push(normalize_native_architecture(Some(architecture)));
            sql.push_str(&format!(" AND architecture = ?{}", values.len()));
        }
        sql.push_str(" ORDER BY name, version, package_release, architecture");

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(values.iter()), Self::from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn transport(&self) -> Result<crate::ccs::CcsTransportEnvelopeV1> {
        serde_json::from_str(&self.transport_json).map_err(|error| {
            crate::Error::InternalError(format!(
                "native package publication {} has malformed transport_json: {error}",
                self.id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| self.content_hash.clone())
            ))
        })
    }

    pub fn active_referencing_object(conn: &Connection, sha256: &str) -> Result<Option<Self>> {
        let pattern = format!("%\"{sha256}\"%");
        let sql = format!(
            "SELECT {} FROM native_package_publications \
             WHERE status = 'public' AND transport_json LIKE ?1",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let candidates = stmt
            .query_map([pattern], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for candidate in candidates {
            if candidate
                .transport()?
                .objects
                .iter()
                .any(|object| object.sha256 == sha256)
            {
                return Ok(Some(candidate));
            }
        }
        Ok(None)
    }

    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        Self::require_public_source_profile(&self.source_profile)?;
        conn.execute(
            "INSERT INTO native_package_publications (
                repository_id, repository_package_id, source_profile, name, version, package_release,
                architecture, package_kind, authority_format_version, status, content_hash,
                transport_json, total_size, package_path, target_path, trust_status
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                self.repository_id,
                self.repository_package_id,
                &self.source_profile,
                &self.name,
                &self.version,
                &self.package_release,
                &self.architecture,
                &self.package_kind,
                self.authority_format_version,
                self.status.as_str(),
                &self.content_hash,
                &self.transport_json,
                self.total_size,
                &self.package_path,
                &self.target_path,
                &self.trust_status,
            ],
        )?;
        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    fn require_public_source_profile(source_profile: &str) -> Result<()> {
        if crate::repository::supported_profiles::profile_by_public_id(source_profile).is_none() {
            return Err(crate::Error::ConfigError(format!(
                "unsupported public source profile '{source_profile}' for native publication"
            )));
        }
        Ok(())
    }

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        let raw_status: String = row.get(10)?;
        Ok(Self {
            id: row.get(0)?,
            repository_id: row.get(1)?,
            repository_package_id: row.get(2)?,
            source_profile: row.get(3)?,
            name: row.get(4)?,
            version: row.get(5)?,
            package_release: row.get(6)?,
            architecture: row.get(7)?,
            package_kind: row.get(8)?,
            authority_format_version: row.get(9)?,
            status: NativePublicationStatus::from_db(&raw_status, 10)?,
            content_hash: row.get(11)?,
            transport_json: row.get(12)?,
            total_size: row.get(13)?,
            package_path: row.get(14)?,
            target_path: row.get(15)?,
            trust_status: row.get(16)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_architecture_normalizes_absent_values() {
        assert_eq!(normalize_native_architecture(None), "noarch");
        assert_eq!(normalize_native_architecture(Some("")), "noarch");
        assert_eq!(normalize_native_architecture(Some(" x86_64 ")), "x86_64");
    }

    #[test]
    fn native_publication_status_fails_closed_for_unknown_value() {
        let err = NativePublicationStatus::from_db("future-status", 10).unwrap_err();
        assert!(matches!(
            err,
            rusqlite::Error::FromSqlConversionFailure(10, rusqlite::types::Type::Text, _)
        ));
    }
}
