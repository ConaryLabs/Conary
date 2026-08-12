// apps/conary/src/commands/rollback_system_authority/repository_enrollments.rs

//! Exact normalized package-repository database authority for rollback.

use anyhow::{Result, bail};
use rusqlite::types::{Value, ValueRef};
use rusqlite::{Connection, Transaction, params_from_iter};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RepositoryEnrollmentSnapshot {
    tables: Vec<TableSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TableSnapshot {
    table: String,
    columns: Vec<String>,
    rows: Vec<Vec<DatabaseValue>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "kebab-case")]
enum DatabaseValue {
    Null,
    Integer(i64),
    Text(String),
    Blob(Vec<u8>),
}

struct TableContract {
    table: &'static str,
    columns: &'static [&'static str],
    select: &'static str,
}

const CONTRACTS: &[TableContract] = &[
    TableContract {
        table: "repository_source_policies",
        columns: &[
            "id",
            "source_identity",
            "scope_kind",
            "scope_identity",
            "ecosystem",
            "version_scheme",
            "stream_kind",
            "stream_identity",
            "update_mode",
        ],
        select: "SELECT DISTINCT policy.id, policy.source_identity, policy.scope_kind,
                         policy.scope_identity, policy.ecosystem, policy.version_scheme,
                         policy.stream_kind, policy.stream_identity, policy.update_mode
                 FROM repository_source_policies policy
                 JOIN repositories repository ON repository.source_policy_id = policy.id
                 WHERE repository.managed_by = 'package-projection'
                 ORDER BY policy.id",
    },
    TableContract {
        table: "repositories",
        columns: &[
            "id",
            "name",
            "url",
            "enabled",
            "priority",
            "trust_policy_json",
            "metadata_expire",
            "created_at",
            "content_url",
            "default_strategy",
            "default_strategy_endpoint",
            "source_profile",
            "tuf_enabled",
            "tuf_root_version",
            "tuf_root_url",
            "security_advisory_support",
            "package_format",
            "parser_config_json",
            "managed_by",
            "source_policy_id",
            "repository_identity",
            "stream_binding_sha256",
        ],
        select: "SELECT id, name, url, enabled, priority, trust_policy_json,
                        metadata_expire, created_at, content_url, default_strategy,
                        default_strategy_endpoint, source_profile, tuf_enabled, tuf_root_version,
                        tuf_root_url, security_advisory_support, package_format,
                        parser_config_json, managed_by, source_policy_id, repository_identity,
                        stream_binding_sha256
                 FROM repositories WHERE managed_by = 'package-projection' ORDER BY id",
    },
    TableContract {
        table: "repository_source_pins",
        columns: &["repository_id", "snapshot_sha256"],
        select: "SELECT pin.repository_id, pin.snapshot_sha256
                 FROM repository_source_pins pin
                 JOIN repositories repository ON repository.id = pin.repository_id
                 WHERE repository.managed_by = 'package-projection'
                 ORDER BY pin.repository_id",
    },
    TableContract {
        table: "package_repository_enrollments",
        columns: &[
            "id",
            "trove_id",
            "owner_kind",
            "source_package",
            "source_version",
            "intent_id",
            "intent_sha256",
            "intent_json",
            "last_owner",
            "created_at",
        ],
        select: "SELECT id, trove_id, owner_kind, source_package, source_version, intent_id,
                        intent_sha256, intent_json, last_owner, created_at
                 FROM package_repository_enrollments ORDER BY id",
    },
    TableContract {
        table: "package_repository_enrollment_members",
        columns: &[
            "enrollment_id",
            "repository_id",
            "repository_name",
            "definition_sha256",
        ],
        select: "SELECT enrollment_id, repository_id, repository_name, definition_sha256
                 FROM package_repository_enrollment_members
                 ORDER BY enrollment_id, repository_id",
    },
    TableContract {
        table: "package_repository_projection_owners",
        columns: &["enrollment_id", "path", "sha256", "mode", "role"],
        select: "SELECT enrollment_id, path, sha256, mode, role
                 FROM package_repository_projection_owners
                 ORDER BY enrollment_id, path",
    },
];

impl Default for RepositoryEnrollmentSnapshot {
    fn default() -> Self {
        Self {
            tables: CONTRACTS
                .iter()
                .map(|contract| TableSnapshot {
                    table: contract.table.to_string(),
                    columns: contract
                        .columns
                        .iter()
                        .map(|column| column.to_string())
                        .collect(),
                    rows: Vec::new(),
                })
                .collect(),
        }
    }
}

impl RepositoryEnrollmentSnapshot {
    pub(super) fn capture(conn: &Connection) -> Result<Self> {
        let tables = CONTRACTS
            .iter()
            .map(|contract| capture_table(conn, contract))
            .collect::<Result<Vec<_>>>()?;
        let snapshot = Self { tables };
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub(super) fn validate(&self) -> Result<()> {
        if self.tables.len() != CONTRACTS.len() {
            bail!(
                "rollback repository enrollment snapshot has {} tables; expected {}",
                self.tables.len(),
                CONTRACTS.len()
            );
        }
        for (table, contract) in self.tables.iter().zip(CONTRACTS) {
            if table.table != contract.table
                || table.columns.iter().map(String::as_str).collect::<Vec<_>>() != contract.columns
            {
                bail!(
                    "rollback repository enrollment snapshot table contract changed at '{}'",
                    table.table
                );
            }
            if table
                .rows
                .iter()
                .any(|row| row.len() != contract.columns.len())
            {
                bail!(
                    "rollback repository enrollment snapshot table '{}' has malformed rows",
                    table.table
                );
            }
        }
        self.validate_enrollment_authority()?;
        Ok(())
    }

    fn validate_enrollment_authority(&self) -> Result<()> {
        let repositories = self.table("repositories")?;
        let repository_ids = repositories
            .rows
            .iter()
            .map(|row| integer(&row[0], "repositories.id"))
            .collect::<Result<BTreeSet<_>>>()?;
        for row in &repositories.rows {
            if text(&row[18], "repositories.managed_by")? != "package-projection" {
                bail!("rollback package repository snapshot contains non-package ownership");
            }
        }

        let enrollments = self.table("package_repository_enrollments")?;
        let mut intents = BTreeMap::new();
        for row in &enrollments.rows {
            let id = integer(&row[0], "package_repository_enrollments.id")?;
            if intents.contains_key(&id) {
                bail!("rollback repository enrollment snapshot repeats enrollment {id}");
            }
            let owner_kind = text(&row[2], "package_repository_enrollments.owner_kind")?;
            match (owner_kind, &row[1]) {
                ("package", DatabaseValue::Integer(_)) | ("retained", DatabaseValue::Null) => {}
                _ => bail!("rollback repository enrollment {id} has inconsistent owner identity"),
            }
            let intent_json = text(&row[7], "package_repository_enrollments.intent_json")?;
            let intent: conary_core::repository::enrollment::PackageRepositoryEnrollmentIntent =
                serde_json::from_str(intent_json)?;
            intent.validate()?;
            if text(&row[5], "package_repository_enrollments.intent_id")? != intent.intent_id
                || text(&row[6], "package_repository_enrollments.intent_sha256")?
                    != intent.sha256()?
                || text(&row[8], "package_repository_enrollments.last_owner")?
                    != last_owner_value(intent.last_owner)
            {
                bail!("rollback repository enrollment {id} disagrees with its signed intent");
            }
            intents.insert(id, intent);
        }

        let mut observed_repositories = BTreeMap::<i64, BTreeSet<String>>::new();
        for row in &self.table("package_repository_enrollment_members")?.rows {
            let enrollment_id = integer(&row[0], "enrollment member enrollment_id")?;
            let repository_id = integer(&row[1], "enrollment member repository_id")?;
            if !repository_ids.contains(&repository_id) {
                bail!("rollback enrollment member names absent repository {repository_id}");
            }
            let intent = intents.get(&enrollment_id).ok_or_else(|| {
                anyhow::anyhow!(
                    "rollback enrollment member names absent enrollment {enrollment_id}"
                )
            })?;
            let name = text(&row[2], "enrollment member repository_name")?;
            let definition = intent
                .repositories
                .iter()
                .find(|definition| definition.name == name)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "rollback enrollment {enrollment_id} has undeclared repository member '{name}'"
                    )
                })?;
            let expected = conary_core::hash::sha256(&serde_json::to_vec(definition)?);
            if text(&row[3], "enrollment member definition_sha256")? != expected {
                bail!(
                    "rollback enrollment {enrollment_id} repository member '{name}' digest mismatch"
                );
            }
            if !observed_repositories
                .entry(enrollment_id)
                .or_default()
                .insert(name.to_string())
            {
                bail!("rollback enrollment {enrollment_id} repeats repository member '{name}'");
            }
        }

        let mut observed_projections = BTreeMap::<i64, BTreeSet<String>>::new();
        for row in &self.table("package_repository_projection_owners")?.rows {
            let enrollment_id = integer(&row[0], "projection owner enrollment_id")?;
            let intent = intents.get(&enrollment_id).ok_or_else(|| {
                anyhow::anyhow!("rollback projection owner names absent enrollment {enrollment_id}")
            })?;
            let path = text(&row[1], "projection owner path")?;
            let projection = intent
                .projections
                .iter()
                .find(|projection| projection.path == path)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "rollback enrollment {enrollment_id} has undeclared projection '{path}'"
                    )
                })?;
            if text(&row[2], "projection owner sha256")? != projection.sha256
                || integer(&row[3], "projection owner mode")? != i64::from(projection.mode)
                || text(&row[4], "projection owner role")? != projection_role_value(projection.role)
            {
                bail!("rollback enrollment {enrollment_id} projection '{path}' changed");
            }
            if !observed_projections
                .entry(enrollment_id)
                .or_default()
                .insert(path.to_string())
            {
                bail!("rollback enrollment {enrollment_id} repeats projection '{path}'");
            }
        }

        for (id, intent) in intents {
            let expected_repositories = intent
                .repositories
                .iter()
                .map(|definition| definition.name.clone())
                .collect::<BTreeSet<_>>();
            let expected_projections = intent
                .projections
                .iter()
                .map(|projection| projection.path.clone())
                .collect::<BTreeSet<_>>();
            if observed_repositories.remove(&id).unwrap_or_default() != expected_repositories
                || observed_projections.remove(&id).unwrap_or_default() != expected_projections
            {
                bail!("rollback repository enrollment {id} has an incomplete authority graph");
            }
        }
        Ok(())
    }

    fn table(&self, name: &str) -> Result<&TableSnapshot> {
        self.tables
            .iter()
            .find(|table| table.table == name)
            .ok_or_else(|| anyhow::anyhow!("rollback repository snapshot has no '{name}' table"))
    }

    pub(super) fn restore(&self, tx: &Transaction<'_>) -> Result<()> {
        self.validate()?;
        tx.execute_batch(
            "DELETE FROM package_repository_projection_owners;
             DELETE FROM package_repository_enrollment_members;
             DELETE FROM package_repository_enrollments;
             DELETE FROM repositories WHERE managed_by = 'package-projection';
             DELETE FROM repository_source_policies
             WHERE NOT EXISTS (
                 SELECT 1 FROM repositories WHERE source_policy_id = repository_source_policies.id
             );",
        )?;
        for table in &self.tables {
            let placeholders = (1..=table.columns.len())
                .map(|index| format!("?{index}"))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "INSERT INTO {} ({}) VALUES ({})",
                table.table,
                table.columns.join(", "),
                placeholders
            );
            for row in &table.rows {
                tx.execute(
                    &sql,
                    params_from_iter(row.iter().map(DatabaseValue::to_sql_value)),
                )?;
            }
        }
        Ok(())
    }
}

fn integer(value: &DatabaseValue, field: &str) -> Result<i64> {
    match value {
        DatabaseValue::Integer(value) => Ok(*value),
        _ => bail!("rollback repository snapshot field '{field}' is not an integer"),
    }
}

fn text<'a>(value: &'a DatabaseValue, field: &str) -> Result<&'a str> {
    match value {
        DatabaseValue::Text(value) => Ok(value),
        _ => bail!("rollback repository snapshot field '{field}' is not text"),
    }
}

fn last_owner_value(
    value: conary_core::repository::enrollment::LastOwnerDisposition,
) -> &'static str {
    match value {
        conary_core::repository::enrollment::LastOwnerDisposition::RemoveWhenUnowned => {
            "remove-when-unowned"
        }
        conary_core::repository::enrollment::LastOwnerDisposition::Retain => "retain",
    }
}

fn projection_role_value(
    value: conary_core::repository::enrollment::PackageProjectionRole,
) -> &'static str {
    match value {
        conary_core::repository::enrollment::PackageProjectionRole::Declaration => "declaration",
        conary_core::repository::enrollment::PackageProjectionRole::TrustRoot => "trust-root",
    }
}

fn capture_table(conn: &Connection, contract: &TableContract) -> Result<TableSnapshot> {
    let mut statement = conn.prepare(contract.select)?;
    let rows = statement
        .query_map([], |row| {
            (0..contract.columns.len())
                .map(|index| DatabaseValue::from_value(row.get_ref(index)?))
                .collect::<rusqlite::Result<Vec<_>>>()
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(TableSnapshot {
        table: contract.table.to_string(),
        columns: contract
            .columns
            .iter()
            .map(|column| column.to_string())
            .collect(),
        rows,
    })
}

impl DatabaseValue {
    fn from_value(value: ValueRef<'_>) -> rusqlite::Result<Self> {
        match value {
            ValueRef::Null => Ok(Self::Null),
            ValueRef::Integer(value) => Ok(Self::Integer(value)),
            ValueRef::Text(value) => std::str::from_utf8(value)
                .map(|value| Self::Text(value.to_string()))
                .map_err(|error| rusqlite::Error::Utf8Error(0, error)),
            ValueRef::Blob(value) => Ok(Self::Blob(value.to_vec())),
            ValueRef::Real(_) => Err(rusqlite::Error::InvalidColumnType(
                0,
                "repository enrollment rollback value".to_string(),
                rusqlite::types::Type::Real,
            )),
        }
    }

    fn to_sql_value(&self) -> Value {
        match self {
            Self::Null => Value::Null,
            Self::Integer(value) => Value::Integer(*value),
            Self::Text(value) => Value::Text(value.clone()),
            Self::Blob(value) => Value::Blob(value.clone()),
        }
    }
}
