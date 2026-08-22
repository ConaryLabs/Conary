// crates/conary-core/src/repository/universe/index/replay.rs

//! Bounded SQLite-to-SQLite replay of normalized catalog authority.

use std::path::Path;

use rusqlite::{Connection, params};

use crate::error::{Error, Result};
use crate::repository::catalog::CatalogCountsV1;

const REPLAY_STATE_SCHEMA: &str = r#"
CREATE TEMP TABLE universe_package_ids (
    package_key_sha256 TEXT PRIMARY KEY,
    id INTEGER NOT NULL UNIQUE CHECK(id < 0)
) STRICT, WITHOUT ROWID;

CREATE TEMP TABLE universe_group_ids (
    package_key_sha256 TEXT NOT NULL,
    ordinal INTEGER NOT NULL,
    id INTEGER NOT NULL UNIQUE CHECK(id < 0),
    PRIMARY KEY(package_key_sha256, ordinal)
) STRICT, WITHOUT ROWID;
"#;

#[derive(Debug, Default)]
pub(super) struct RowOffsets {
    packages: i64,
    provides: i64,
    requirement_groups: i64,
    requirements: i64,
}

#[derive(Debug, Default)]
pub(super) struct ReplayCounts {
    pub packages: u64,
    pub provides: u64,
    pub requirement_groups: u64,
    pub requirements: u64,
}

impl ReplayCounts {
    pub fn add(&mut self, other: Self) -> Result<()> {
        self.packages = checked_add(self.packages, other.packages, "package")?;
        self.provides = checked_add(self.provides, other.provides, "provide")?;
        self.requirement_groups = checked_add(
            self.requirement_groups,
            other.requirement_groups,
            "requirement-group",
        )?;
        self.requirements = checked_add(self.requirements, other.requirements, "requirement")?;
        Ok(())
    }
}

pub(super) fn attach_catalog(index: &Connection, alias: &str, path: &Path) -> Result<()> {
    require_alias(alias)?;
    let mut uri = url::Url::from_file_path(path).map_err(|_| {
        Error::InvalidPath(format!(
            "catalog path {} cannot be represented as a SQLite URI",
            path.display()
        ))
    })?;
    uri.query_pairs_mut()
        .append_pair("mode", "ro")
        .append_pair("immutable", "1");
    index.execute(&format!("ATTACH DATABASE ?1 AS {alias}"), [uri.as_str()])?;
    Ok(())
}

pub(super) fn detach_catalog(index: &Connection, alias: &str) -> Result<()> {
    require_alias(alias)?;
    index.execute_batch(&format!("DETACH DATABASE {alias}"))?;
    Ok(())
}

pub(super) fn prepare(index: &Connection) -> Result<()> {
    index.execute_batch(REPLAY_STATE_SCHEMA)?;
    Ok(())
}

pub(super) fn copy_catalog(
    index: &Connection,
    alias: &str,
    repository_id: i64,
    synced_at: &str,
    expected: CatalogCountsV1,
    offsets: &mut RowOffsets,
) -> Result<ReplayCounts> {
    require_alias(alias)?;
    index.execute_batch(
        "DELETE FROM temp.universe_group_ids;
         DELETE FROM temp.universe_package_ids;",
    )?;
    let package_base = reserve(&mut offsets.packages, expected.packages, "package")?;
    let provide_base = reserve(&mut offsets.provides, expected.provides, "provide")?;
    let group_base = reserve(
        &mut offsets.requirement_groups,
        expected.requirement_groups,
        "requirement-group",
    )?;
    let requirement_base = reserve(
        &mut offsets.requirements,
        expected.requirement_atoms,
        "requirement",
    )?;

    execute_exact(
        index,
        &format!(
            "INSERT INTO temp.universe_package_ids (package_key_sha256, id)
             SELECT package_key_sha256,
                    ?1 - ROW_NUMBER() OVER (ORDER BY package_key_sha256)
             FROM {alias}.catalog_packages"
        ),
        [package_base],
        expected.packages,
        "package ID map",
    )?;
    execute_exact(
        index,
        &format!(
            "INSERT INTO repository_packages (
                 id, repository_id, name, version, package_release, architecture,
                 debian_multi_arch, description, checksum, size, download_url,
                 metadata, synced_at, is_security_update, severity, cve_ids,
                 advisory_id, advisory_url, source_profile, version_scheme, canonical_id
             )
             SELECT ids.id, ?1, package.name, package.version, package.package_release,
                    package.architecture, package.debian_multi_arch, package.description,
                    package.checksum, package.size, package.download_url, package.metadata,
                    ?2, package.is_security_update, package.severity, package.cve_ids,
                    package.advisory_id, package.advisory_url, package.source_profile,
                    package.version_scheme, resolution.canonical_id
             FROM {alias}.catalog_packages package
             JOIN temp.universe_package_ids ids USING(package_key_sha256)
             LEFT JOIN canonical_resolution resolution
               ON resolution.distro = package.source_profile
              AND resolution.distro_name = package.name
             ORDER BY package.package_key_sha256"
        ),
        params![repository_id, synced_at],
        expected.packages,
        "packages",
    )?;
    execute_exact(
        index,
        &format!(
            "INSERT INTO repository_provides (
                 id, repository_package_id, capability, version, version_relation,
                 kind, raw, version_scheme, architecture_qualifier_kind,
                 architecture, provenance
             )
             SELECT ?1 - ROW_NUMBER() OVER (
                        ORDER BY provide.package_key_sha256, provide.ordinal
                    ),
                    package_ids.id, provide.capability, provide.version,
                    provide.version_relation, provide.kind, provide.raw,
                    provide.version_scheme,
                    json_extract(provide.architecture_qualifier_json, '$.kind'),
                    CASE WHEN json_extract(
                                       provide.architecture_qualifier_json, '$.kind'
                                   ) = 'exact'
                         THEN json_extract(
                                  provide.architecture_qualifier_json, '$.architecture'
                              )
                    END,
                    provide.provenance_json
             FROM {alias}.catalog_provides provide
             JOIN temp.universe_package_ids package_ids USING(package_key_sha256)
             ORDER BY provide.package_key_sha256, provide.ordinal"
        ),
        [provide_base],
        expected.provides,
        "provides",
    )?;
    execute_exact(
        index,
        &format!(
            "INSERT INTO temp.universe_group_ids (package_key_sha256, ordinal, id)
             SELECT package_key_sha256, ordinal,
                    ?1 - ROW_NUMBER() OVER (ORDER BY package_key_sha256, ordinal)
             FROM {alias}.catalog_requirement_groups"
        ),
        [group_base],
        expected.requirement_groups,
        "requirement-group ID map",
    )?;
    execute_exact(
        index,
        &format!(
            "INSERT INTO repository_requirement_groups (
                 id, repository_package_id, kind, behavior, description,
                 native_text, expression_json
             )
             SELECT group_ids.id, package_ids.id, requirement.kind,
                    requirement.behavior, requirement.description,
                    requirement.native_text, requirement.expression_json
             FROM {alias}.catalog_requirement_groups requirement
             JOIN temp.universe_package_ids package_ids USING(package_key_sha256)
             JOIN temp.universe_group_ids group_ids
               ON group_ids.package_key_sha256 = requirement.package_key_sha256
              AND group_ids.ordinal = requirement.ordinal
             ORDER BY requirement.package_key_sha256, requirement.ordinal"
        ),
        [],
        expected.requirement_groups,
        "requirement groups",
    )?;
    execute_exact(
        index,
        &format!(
            "INSERT INTO repository_requirements (
                 id, repository_package_id, group_id, capability,
                 version_constraint, kind, dependency_type, raw
             )
             SELECT ?1 - ROW_NUMBER() OVER (
                        ORDER BY atom.package_key_sha256, atom.group_ordinal, atom.ordinal
                    ),
                    package_ids.id, group_ids.id, atom.capability,
                    atom.version_constraint, atom.kind, atom.dependency_type, atom.raw
             FROM {alias}.catalog_requirement_atoms atom
             JOIN temp.universe_package_ids package_ids USING(package_key_sha256)
             JOIN temp.universe_group_ids group_ids
               ON group_ids.package_key_sha256 = atom.package_key_sha256
              AND group_ids.ordinal = atom.group_ordinal
             ORDER BY atom.package_key_sha256, atom.group_ordinal, atom.ordinal"
        ),
        [requirement_base],
        expected.requirement_atoms,
        "requirements",
    )?;

    Ok(ReplayCounts {
        packages: expected.packages,
        provides: expected.provides,
        requirement_groups: expected.requirement_groups,
        requirements: expected.requirement_atoms,
    })
}

fn reserve(slot: &mut i64, count: u64, label: &str) -> Result<i64> {
    let count = i64::try_from(count).map_err(|_| {
        Error::ConfigError(format!(
            "client universe {label} count exceeds SQLite integer range"
        ))
    })?;
    let base = *slot;
    *slot = slot.checked_sub(count).ok_or_else(|| {
        Error::InternalError(format!("client universe {label} ID space exhausted"))
    })?;
    Ok(base)
}

fn checked_add(left: u64, right: u64, label: &str) -> Result<u64> {
    left.checked_add(right)
        .ok_or_else(|| Error::InternalError(format!("client universe {label} count overflow")))
}

fn execute_exact<P: rusqlite::Params>(
    index: &Connection,
    sql: &str,
    params: P,
    expected: u64,
    label: &str,
) -> Result<()> {
    let changed = index.execute(sql, params)?;
    if u64::try_from(changed).ok() != Some(expected) {
        return Err(Error::ConflictError(format!(
            "client universe replay copied {changed} {label}; manifest requires {expected}"
        )));
    }
    Ok(())
}

fn require_alias(alias: &str) -> Result<()> {
    if !alias
        .strip_prefix("universe_catalog_")
        .is_some_and(|suffix| {
            !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
        })
    {
        return Err(Error::ConfigError(
            "client universe catalog alias is invalid".to_string(),
        ));
    }
    Ok(())
}
