// crates/conary-core/src/repository/catalog/store/stream.rs

//! Ordered relation-row replay and logical hashing for immutable catalogs.

use rusqlite::Connection;

use super::super::record::CatalogLogicalDigestV1;
use super::super::{
    CatalogCountsV1, CatalogPackageOriginV1, CatalogRequirementAtomV1, CatalogRequirementGroupV1,
    CatalogScopeV1, CatalogSourceEvidenceV1,
};
use super::{
    CatalogReader, SELECT_PACKAGES, insert_package_base, insert_profile_package_base_if_absent,
    insert_provide, insert_requirement_group, load_requirement_atoms, package_from_row,
    provide_from_row,
};
use crate::error::{Error, Result};

impl CatalogReader {
    /// Copy normalized rows into another private catalog without rebuilding a
    /// package-sized relation vector. `profile_origin` rewrites only the
    /// profile member authority and its derived package key.
    pub(in crate::repository) fn copy_packages_to(
        &self,
        destination: &Connection,
        destination_scope: &CatalogScopeV1,
        profile_origin: Option<&CatalogPackageOriginV1>,
    ) -> Result<()> {
        if profile_origin.is_none() && &self.binding.scope != destination_scope {
            return Err(Error::ConflictError(
                "catalog row replay requires the exact source scope".to_string(),
            ));
        }
        let mut packages = self
            .connection
            .prepare(&format!("{SELECT_PACKAGES} ORDER BY package_key_sha256"))?;
        let mut package_rows = packages.query([])?;
        while let Some(row) = package_rows.next()? {
            let mut package = package_from_row(row)?;
            let source_key = package.package_key_sha256.clone();
            if let Some(origin) = profile_origin {
                package.origin = origin.clone();
            }
            package.canonicalize_for_scope(destination_scope)?;
            let destination_key = package.package_key_sha256.clone();
            let inserted = if profile_origin.is_some() {
                insert_profile_package_base_if_absent(destination, &package)?
            } else {
                insert_package_base(destination, &package)?;
                true
            };
            if inserted {
                self.copy_provides(destination, &source_key, &destination_key)?;
                self.copy_requirement_groups(destination, &source_key, &destination_key)?;
            } else {
                self.require_same_profile_record(
                    destination,
                    &source_key,
                    &destination_key,
                    &package,
                )?;
            }
        }
        Ok(())
    }

    fn require_same_profile_record(
        &self,
        destination: &Connection,
        source_key: &str,
        destination_key: &str,
        package: &super::super::CatalogPackageRecordV1,
    ) -> Result<()> {
        let selected = destination.query_row(
            &format!("{SELECT_PACKAGES} WHERE package_key_sha256 = ?1"),
            [destination_key],
            package_from_row,
        )?;
        let mut matches = selected.same_profile_base(package)?;
        for sql in [
            "SELECT ordinal, capability, version, version_relation, kind, raw,
                    version_scheme, architecture_qualifier_json, provenance_json
             FROM catalog_provides WHERE package_key_sha256 = ?1 ORDER BY ordinal",
            "SELECT ordinal, kind, behavior, description, native_text, expression_json
             FROM catalog_requirement_groups
             WHERE package_key_sha256 = ?1 ORDER BY ordinal",
            "SELECT group_ordinal, ordinal, capability, version_constraint, kind,
                    dependency_type, raw
             FROM catalog_requirement_atoms
             WHERE package_key_sha256 = ?1 ORDER BY group_ordinal, ordinal",
        ] {
            matches &= ordered_rows_match(
                &self.connection,
                source_key,
                destination,
                destination_key,
                sql,
            )?;
        }
        if !matches {
            let selected_origin = match &selected.origin {
                CatalogPackageOriginV1::Profile {
                    repository_identity,
                    ..
                } => repository_identity.as_str(),
                CatalogPackageOriginV1::Source { .. } => "source catalog",
            };
            let duplicate_origin = match &package.origin {
                CatalogPackageOriginV1::Profile {
                    repository_identity,
                    ..
                } => repository_identity.as_str(),
                CatalogPackageOriginV1::Source { .. } => "source catalog",
            };
            return Err(Error::ConflictError(format!(
                "profile package identity {} {}-{} {:?} disagrees between repositories '{}' \
                 and '{}'",
                package.name,
                package.version,
                package.package_release,
                package.architecture,
                selected_origin,
                duplicate_origin
            )));
        }
        Ok(())
    }

    fn copy_provides(
        &self,
        destination: &Connection,
        source_key: &str,
        destination_key: &str,
    ) -> Result<()> {
        let mut provides = self.connection.prepare(
            "SELECT capability, version, version_relation, kind, raw, version_scheme,
                    architecture_qualifier_json, provenance_json, ordinal
             FROM catalog_provides
             WHERE package_key_sha256 = ?1
             ORDER BY ordinal",
        )?;
        let mut rows = provides.query([source_key])?;
        while let Some(row) = rows.next()? {
            insert_provide(
                destination,
                destination_key,
                row.get(8)?,
                &provide_from_row(row)?,
            )?;
        }
        Ok(())
    }

    fn copy_requirement_groups(
        &self,
        destination: &Connection,
        source_key: &str,
        destination_key: &str,
    ) -> Result<()> {
        let mut groups = self.connection.prepare(
            "SELECT ordinal, kind, behavior, description, native_text, expression_json
             FROM catalog_requirement_groups
             WHERE package_key_sha256 = ?1
             ORDER BY ordinal",
        )?;
        let mut rows = groups.query([source_key])?;
        while let Some(row) = rows.next()? {
            let ordinal: i64 = row.get(0)?;
            let mut group = requirement_group_from_row(row)?;
            group.atoms = load_requirement_atoms(&self.connection, source_key, ordinal)?;
            insert_requirement_group(destination, destination_key, ordinal, &group)?;
        }
        Ok(())
    }
}

fn ordered_rows_match(
    source: &Connection,
    source_key: &str,
    destination: &Connection,
    destination_key: &str,
    sql: &str,
) -> Result<bool> {
    let mut source_statement = source.prepare(sql)?;
    let mut destination_statement = destination.prepare(sql)?;
    let column_count = source_statement.column_count();
    if destination_statement.column_count() != column_count {
        return Err(Error::InternalError(
            "profile duplicate comparison column count differs".to_string(),
        ));
    }
    let mut source_rows = source_statement.query([source_key])?;
    let mut destination_rows = destination_statement.query([destination_key])?;
    loop {
        match (source_rows.next()?, destination_rows.next()?) {
            (None, None) => return Ok(true),
            (Some(source_row), Some(destination_row)) => {
                for column in 0..column_count {
                    if source_row.get_ref(column)? != destination_row.get_ref(column)? {
                        return Ok(false);
                    }
                }
            }
            _ => return Ok(false),
        }
    }
}

/// Calculate and validate the exact V1 logical identity without retaining one
/// package's potentially repository-sized provide array.
pub(in crate::repository) fn digest_catalog_connection(
    connection: &Connection,
    scope: &CatalogScopeV1,
    source_evidence: &[CatalogSourceEvidenceV1],
) -> Result<(String, CatalogCountsV1)> {
    let mut digest = CatalogLogicalDigestV1::new(scope, source_evidence)?;
    let mut packages =
        connection.prepare(&format!("{SELECT_PACKAGES} ORDER BY package_key_sha256"))?;
    let mut package_rows = packages.query([])?;
    while let Some(row) = package_rows.next()? {
        let package = package_from_row(row)?;
        let package_key = package.package_key_sha256.clone();
        let mut package_digest = digest.begin_package(&package)?;

        let mut provides = connection.prepare(
            "SELECT capability, version, version_relation, kind, raw, version_scheme,
                    architecture_qualifier_json, provenance_json
             FROM catalog_provides
             WHERE package_key_sha256 = ?1
             ORDER BY ordinal",
        )?;
        let mut provide_rows = provides.query([&package_key])?;
        while let Some(provide_row) = provide_rows.next()? {
            package_digest.provide(&provide_from_row(provide_row)?)?;
        }

        let mut groups = connection.prepare(
            "SELECT ordinal, kind, behavior, description, native_text, expression_json
             FROM catalog_requirement_groups
             WHERE package_key_sha256 = ?1
             ORDER BY ordinal",
        )?;
        let mut group_rows = groups.query([&package_key])?;
        while let Some(group_row) = group_rows.next()? {
            let ordinal: i64 = group_row.get(0)?;
            let group = requirement_group_from_row(group_row)?;
            let mut group_digest = package_digest.begin_requirement_group(&group)?;
            let mut atoms = connection.prepare(
                "SELECT capability, version_constraint, kind, dependency_type, raw
                 FROM catalog_requirement_atoms
                 WHERE package_key_sha256 = ?1 AND group_ordinal = ?2
                 ORDER BY ordinal",
            )?;
            let mut atom_rows = atoms.query(rusqlite::params![&package_key, ordinal])?;
            while let Some(atom_row) = atom_rows.next()? {
                group_digest.atom(&requirement_atom_from_row(atom_row)?)?;
            }
            group_digest.finish()?;
        }
        package_digest.finish()?;
    }
    digest.finish()
}

fn requirement_atom_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<CatalogRequirementAtomV1> {
    Ok(CatalogRequirementAtomV1 {
        capability: row.get(0)?,
        version_constraint: row.get(1)?,
        kind: row.get(2)?,
        dependency_type: row.get(3)?,
        raw: row.get(4)?,
    })
}

fn requirement_group_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<CatalogRequirementGroupV1> {
    Ok(CatalogRequirementGroupV1 {
        kind: row.get(1)?,
        behavior: row.get(2)?,
        description: row.get(3)?,
        native_text: row.get(4)?,
        expression_json: row.get(5)?,
        atoms: Vec::new(),
    })
}
