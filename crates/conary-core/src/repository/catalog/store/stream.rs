// crates/conary-core/src/repository/catalog/store/stream.rs

//! Ordered relation-row replay and logical hashing for immutable catalogs.

use rusqlite::Connection;

use super::super::record::CatalogLogicalDigestV1;
use super::super::{
    CatalogCountsV1, CatalogPackageOriginV1, CatalogRequirementGroupV1, CatalogScopeV1,
    CatalogSourceEvidenceV1,
};
use super::{
    CatalogReader, SELECT_PACKAGES, insert_package_base, insert_provide, insert_requirement_group,
    load_requirement_atoms, package_from_row, provide_from_row,
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
            insert_package_base(destination, &package)?;

            self.copy_provides(destination, &source_key, &destination_key)?;
            self.copy_requirement_groups(destination, &source_key, &destination_key)?;
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
            let mut group = requirement_group_from_row(group_row)?;
            group.atoms = load_requirement_atoms(connection, &package_key, ordinal)?;
            package_digest.requirement_group(&group)?;
        }
        package_digest.finish()?;
    }
    digest.finish()
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
