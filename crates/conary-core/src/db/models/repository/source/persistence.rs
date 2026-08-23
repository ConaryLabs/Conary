// conary-core/src/db/models/repository/source/persistence.rs

//! Persisted repository-row decoding and typed source-policy reconstruction.

use std::io;

use rusqlite::Row;

use super::{
    AuthenticatedSnapshotIdentity, NativeSourceEcosystem, NativeSourceStream, ProfileSourceRole,
    Repository, RepositoryFormat, RepositoryOwnership, RepositoryParserConfig,
    RepositoryPolicyScope, RepositorySourcePolicy, RepositoryTrustPolicy, RepositoryUpdateMode,
    SecurityAdvisorySupport,
};

impl Repository {
    pub(super) fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        let package_format_value = row.get::<_, String>(20)?;
        let package_format = RepositoryFormat::from_db(&package_format_value).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                20,
                rusqlite::types::Type::Text,
                Box::new(io::Error::new(
                    io::ErrorKind::InvalidData,
                    error.to_string(),
                )),
            )
        })?;
        let parser_config = row
            .get::<_, Option<String>>(21)?
            .map(|value| RepositoryParserConfig::from_json(&value))
            .transpose()
            .map_err(|error| row_conversion_error(21, error.to_string()))?;
        let trust_policy = row
            .get::<_, Option<String>>(6)?
            .map(|value| RepositoryTrustPolicy::from_json(&value))
            .transpose()
            .map_err(|error| row_conversion_error(6, error.to_string()))?;
        let ownership_value = row.get::<_, String>(22)?;
        let managed_by = RepositoryOwnership::from_db(&ownership_value)
            .map_err(|error| row_conversion_error(22, error))?;
        let profile_member_role = row
            .get::<_, Option<String>>(25)?
            .map(|value| ProfileSourceRole::parse(&value))
            .transpose()
            .map_err(|error| row_conversion_error(25, error))?;
        let source_policy = if row.get::<_, Option<i64>>(27)?.is_some() {
            Some(source_policy_from_row(row, 27)?)
        } else {
            None
        };
        let pinned_snapshot = row
            .get::<_, Option<String>>(36)?
            .map(AuthenticatedSnapshotIdentity::from_sha256)
            .transpose()
            .map_err(|error| row_conversion_error(36, error.to_string()))?;
        let repository = Self {
            id: Some(row.get(0)?),
            name: row.get(1)?,
            url: row.get(2)?,
            content_url: row.get(3)?,
            enabled: row.get::<_, i32>(4)? != 0,
            priority: row.get(5)?,
            profile_member_role,
            profile_member_required: row.get::<_, i32>(26)? != 0,
            trust_policy,
            metadata_expire: row.get(7)?,
            last_checked_at: row.get(8)?,
            last_changed_at: row.get(9)?,
            last_validated_at: row.get(10)?,
            last_published_at: row.get(11)?,
            created_at: row.get(12)?,
            default_strategy: row.get(13)?,
            default_strategy_endpoint: row.get(14)?,
            source_profile: row.get(15)?,
            tuf_enabled: row.get::<_, i32>(16)? != 0,
            tuf_root_version: row.get(17)?,
            tuf_root_url: row.get(18)?,
            security_advisory_support: SecurityAdvisorySupport::from_db(
                row.get::<_, String>(19)?.as_str(),
            ),
            package_format,
            parser_config,
            managed_by,
            source_policy,
            repository_identity: row.get(23)?,
            stream_binding_sha256: row.get(24)?,
            pinned_snapshot,
        };
        repository
            .validate_parser_contract()
            .map_err(|error| row_conversion_error(23, error.to_string()))?;
        Ok(repository)
    }
}

pub(super) fn source_policy_from_row(
    row: &Row<'_>,
    offset: usize,
) -> rusqlite::Result<RepositorySourcePolicy> {
    let scope =
        RepositoryPolicyScope::from_db(&row.get::<_, String>(offset + 2)?, row.get(offset + 3)?)
            .map_err(|error| row_conversion_error(offset + 2, error))?;
    let ecosystem = NativeSourceEcosystem::from_db(&row.get::<_, String>(offset + 4)?)
        .map_err(|error| row_conversion_error(offset + 4, error))?;
    let version_scheme = row
        .get::<_, String>(offset + 5)?
        .parse()
        .map_err(|error: String| row_conversion_error(offset + 5, error))?;
    let stream =
        NativeSourceStream::from_db(&row.get::<_, String>(offset + 6)?, row.get(offset + 7)?)
            .map_err(|error| row_conversion_error(offset + 6, error))?;
    let update_mode = RepositoryUpdateMode::from_db(&row.get::<_, String>(offset + 8)?)
        .map_err(|error| row_conversion_error(offset + 8, error))?;
    let policy = RepositorySourcePolicy {
        id: Some(row.get(offset)?),
        source_identity: row.get(offset + 1)?,
        scope,
        ecosystem,
        version_scheme,
        stream,
        update_mode,
    };
    policy
        .validate()
        .map_err(|error| row_conversion_error(offset + 1, error.to_string()))?;
    Ok(policy)
}

fn row_conversion_error(index: usize, error: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        index,
        rusqlite::types::Type::Text,
        Box::new(io::Error::new(io::ErrorKind::InvalidData, error)),
    )
}
