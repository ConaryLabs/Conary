// crates/conary-core/src/repository/catalog/store/stream.rs

//! Ordered relation-row replay and logical hashing for immutable catalogs.

use rusqlite::{Connection, Rows};

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

/// Calculate and validate the exact V1 logical identity with one ordered scan
/// per normalized table and without retaining one package's potentially
/// repository-sized relation arrays.
///
/// The four cursors merge on package key and group ordinal. Catalog cardinality
/// therefore changes rows stepped, not SQLite statements prepared or point
/// queries executed.
pub(in crate::repository) fn digest_catalog_connection(
    connection: &Connection,
    scope: &CatalogScopeV1,
    source_evidence: &[CatalogSourceEvidenceV1],
) -> Result<(String, CatalogCountsV1)> {
    let mut digest = CatalogLogicalDigestV1::new(scope, source_evidence)?;
    let mut packages =
        connection.prepare(&format!("{SELECT_PACKAGES} ORDER BY package_key_sha256"))?;
    let mut provides = connection.prepare(
        "SELECT capability, version, version_relation, kind, raw, version_scheme,
                architecture_qualifier_json, provenance_json, package_key_sha256
         FROM catalog_provides
         ORDER BY package_key_sha256, ordinal",
    )?;
    let mut groups = connection.prepare(
        "SELECT ordinal, kind, behavior, description, native_text, expression_json,
                package_key_sha256
         FROM catalog_requirement_groups
         ORDER BY package_key_sha256, ordinal",
    )?;
    let mut atoms = connection.prepare(
        "SELECT capability, version_constraint, kind, dependency_type, raw,
                package_key_sha256, group_ordinal
         FROM catalog_requirement_atoms
         ORDER BY package_key_sha256, group_ordinal, ordinal",
    )?;
    let mut package_rows = packages.query([])?;
    let mut provide_rows = provides.query([])?;
    let mut group_rows = groups.query([])?;
    let mut atom_rows = atoms.query([])?;
    let mut next_provide = next_ordered_provide(&mut provide_rows)?;
    let mut next_group = next_ordered_group(&mut group_rows)?;
    let mut next_atom = next_ordered_atom(&mut atom_rows)?;

    while let Some(row) = package_rows.next()? {
        let package = package_from_row(row)?;
        let package_key = package.package_key_sha256.clone();
        require_relation_not_before_package(
            next_provide.as_ref().map(|row| row.package_key.as_str()),
            &package_key,
            "provide",
        )?;
        require_relation_not_before_package(
            next_group.as_ref().map(|row| row.package_key.as_str()),
            &package_key,
            "requirement group",
        )?;
        require_relation_not_before_package(
            next_atom.as_ref().map(|row| row.package_key.as_str()),
            &package_key,
            "requirement atom",
        )?;
        let mut package_digest = digest.begin_package(&package)?;

        while next_provide
            .as_ref()
            .is_some_and(|row| row.package_key == package_key)
        {
            let provide = next_provide.take().expect("provide lookahead exists");
            package_digest.provide(&provide.value)?;
            next_provide = next_ordered_provide(&mut provide_rows)?;
        }

        while next_group
            .as_ref()
            .is_some_and(|row| row.package_key == package_key)
        {
            let group = next_group.take().expect("group lookahead exists");
            require_atom_not_before_group(next_atom.as_ref(), &package_key, group.ordinal)?;
            let mut group_digest = package_digest.begin_requirement_group(&group.value)?;
            while next_atom.as_ref().is_some_and(|row| {
                row.package_key == package_key && row.group_ordinal == group.ordinal
            }) {
                let atom = next_atom.take().expect("atom lookahead exists");
                group_digest.atom(&atom.value)?;
                next_atom = next_ordered_atom(&mut atom_rows)?;
            }
            group_digest.finish()?;
            next_group = next_ordered_group(&mut group_rows)?;
        }
        if next_atom
            .as_ref()
            .is_some_and(|row| row.package_key == package_key)
        {
            return Err(Error::ConflictError(format!(
                "catalog requirement atom references a missing group for package {package_key}"
            )));
        }
        package_digest.finish()?;
    }
    require_no_remaining_relation(next_provide.map(|row| row.package_key), "provide")?;
    require_no_remaining_relation(next_group.map(|row| row.package_key), "requirement group")?;
    require_no_remaining_relation(next_atom.map(|row| row.package_key), "requirement atom")?;
    digest.finish()
}

struct OrderedProvide {
    package_key: String,
    value: super::super::CatalogProvideRecordV1,
}

struct OrderedRequirementGroup {
    package_key: String,
    ordinal: i64,
    value: CatalogRequirementGroupV1,
}

struct OrderedRequirementAtom {
    package_key: String,
    group_ordinal: i64,
    value: CatalogRequirementAtomV1,
}

fn next_ordered_provide(rows: &mut Rows<'_>) -> Result<Option<OrderedProvide>> {
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    Ok(Some(OrderedProvide {
        package_key: row.get(8)?,
        value: provide_from_row(row)?,
    }))
}

fn next_ordered_group(rows: &mut Rows<'_>) -> Result<Option<OrderedRequirementGroup>> {
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    Ok(Some(OrderedRequirementGroup {
        package_key: row.get(6)?,
        ordinal: row.get(0)?,
        value: requirement_group_from_row(row)?,
    }))
}

fn next_ordered_atom(rows: &mut Rows<'_>) -> Result<Option<OrderedRequirementAtom>> {
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    Ok(Some(OrderedRequirementAtom {
        package_key: row.get(5)?,
        group_ordinal: row.get(6)?,
        value: requirement_atom_from_row(row)?,
    }))
}

fn require_relation_not_before_package(
    relation_package_key: Option<&str>,
    package_key: &str,
    label: &str,
) -> Result<()> {
    if relation_package_key.is_some_and(|relation_key| relation_key < package_key) {
        return Err(Error::ConflictError(format!(
            "catalog {label} row references a missing package before {package_key}"
        )));
    }
    Ok(())
}

fn require_atom_not_before_group(
    atom: Option<&OrderedRequirementAtom>,
    package_key: &str,
    group_ordinal: i64,
) -> Result<()> {
    if atom.is_some_and(|atom| {
        atom.package_key.as_str() < package_key
            || (atom.package_key == package_key && atom.group_ordinal < group_ordinal)
    }) {
        return Err(Error::ConflictError(format!(
            "catalog requirement atom references a missing group before package {package_key} group {group_ordinal}"
        )));
    }
    Ok(())
}

fn require_no_remaining_relation(package_key: Option<String>, label: &str) -> Result<()> {
    if let Some(package_key) = package_key {
        return Err(Error::ConflictError(format!(
            "catalog {label} row references missing package {package_key}"
        )));
    }
    Ok(())
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
