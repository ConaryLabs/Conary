// crates/conary-core/src/repository/catalog/store/stream.rs

//! Ordered relation-row replay and logical hashing for immutable catalogs.

use rusqlite::{Connection, Rows, Statement, params};

use super::super::record::CatalogLogicalDigestV1;
use super::super::{
    CatalogCountsV1, CatalogPackageOriginV1, CatalogRequirementAtomV1, CatalogRequirementGroupV1,
    CatalogScopeV1, CatalogSourceEvidenceV1,
};
use super::{
    CatalogReader, SELECT_PACKAGES, canonical_json_string, insert_package_base,
    insert_profile_package_base_if_absent, package_from_row, provide_from_row,
};
use crate::error::{Error, Result};
use crate::repository::dependency_model::ProvideVersionRelation;

const SELECT_ORDERED_PROVIDES: &str =
    "SELECT capability, version, version_relation, kind, raw, version_scheme,
            architecture_qualifier_json, provenance_json, package_key_sha256, ordinal
     FROM catalog_provides
     ORDER BY package_key_sha256, ordinal";

const SELECT_ORDERED_REQUIREMENT_GROUPS: &str =
    "SELECT ordinal, kind, behavior, description, native_text, expression_json,
            package_key_sha256
     FROM catalog_requirement_groups
     ORDER BY package_key_sha256, ordinal";

const SELECT_ORDERED_REQUIREMENT_ATOMS: &str =
    "SELECT capability, version_constraint, kind, dependency_type, raw,
            package_key_sha256, group_ordinal, ordinal
     FROM catalog_requirement_atoms
     ORDER BY package_key_sha256, group_ordinal, ordinal";

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
        let mut provides = self.connection.prepare(SELECT_ORDERED_PROVIDES)?;
        let mut groups = self.connection.prepare(SELECT_ORDERED_REQUIREMENT_GROUPS)?;
        let mut atoms = self.connection.prepare(SELECT_ORDERED_REQUIREMENT_ATOMS)?;
        let mut package_rows = packages.query([])?;
        let provide_rows = provides.query([])?;
        let group_rows = groups.query([])?;
        let atom_rows = atoms.query([])?;
        let mut relations = OrderedCatalogRelations::new(provide_rows, group_rows, atom_rows)?;
        let mut sink = ProfileRelationSink::new(destination)?;
        while let Some(row) = package_rows.next()? {
            let mut package = package_from_row(row)?;
            let source_key = package.package_key_sha256.clone();
            relations.require_not_before_package(&source_key)?;
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
                relations.copy_package(&mut sink, &source_key, &destination_key)?;
            } else {
                let selected = destination.query_row(
                    &format!("{SELECT_PACKAGES} WHERE package_key_sha256 = ?1"),
                    [&destination_key],
                    package_from_row,
                )?;
                let matches = selected.same_profile_base(&package)?
                    && relations.package_matches(destination, &source_key, &destination_key)?;
                if !matches {
                    return Err(profile_duplicate_conflict(&selected, &package));
                }
            }
        }
        relations.finish()?;
        Ok(())
    }
}

fn profile_duplicate_conflict(
    selected: &super::super::CatalogPackageRecordV1,
    duplicate: &super::super::CatalogPackageRecordV1,
) -> Error {
    let selected_origin = match &selected.origin {
        CatalogPackageOriginV1::Profile {
            repository_identity,
            ..
        } => repository_identity.as_str(),
        CatalogPackageOriginV1::Source { .. } => "source catalog",
    };
    let duplicate_origin = match &duplicate.origin {
        CatalogPackageOriginV1::Profile {
            repository_identity,
            ..
        } => repository_identity.as_str(),
        CatalogPackageOriginV1::Source { .. } => "source catalog",
    };
    Error::ConflictError(format!(
        "profile package identity {} {}-{} {:?} disagrees between repositories '{}' and '{}'",
        duplicate.name,
        duplicate.version,
        duplicate.package_release,
        duplicate.architecture,
        selected_origin,
        duplicate_origin
    ))
}

struct OrderedCatalogRelations<'statement> {
    provide_rows: Rows<'statement>,
    group_rows: Rows<'statement>,
    atom_rows: Rows<'statement>,
    next_provide: Option<OrderedProvide>,
    next_group: Option<OrderedRequirementGroup>,
    next_atom: Option<OrderedRequirementAtom>,
}

impl<'statement> OrderedCatalogRelations<'statement> {
    fn new(
        mut provide_rows: Rows<'statement>,
        mut group_rows: Rows<'statement>,
        mut atom_rows: Rows<'statement>,
    ) -> Result<Self> {
        let next_provide = next_ordered_provide(&mut provide_rows)?;
        let next_group = next_ordered_group(&mut group_rows)?;
        let next_atom = next_ordered_atom(&mut atom_rows)?;
        Ok(Self {
            provide_rows,
            group_rows,
            atom_rows,
            next_provide,
            next_group,
            next_atom,
        })
    }

    fn require_not_before_package(&self, package_key: &str) -> Result<()> {
        require_relation_not_before_package(
            self.next_provide
                .as_ref()
                .map(|row| row.package_key.as_str()),
            package_key,
            "provide",
        )?;
        require_relation_not_before_package(
            self.next_group.as_ref().map(|row| row.package_key.as_str()),
            package_key,
            "requirement group",
        )?;
        require_relation_not_before_package(
            self.next_atom.as_ref().map(|row| row.package_key.as_str()),
            package_key,
            "requirement atom",
        )
    }

    fn copy_package(
        &mut self,
        sink: &mut ProfileRelationSink<'_>,
        source_key: &str,
        destination_key: &str,
    ) -> Result<()> {
        while self
            .next_provide
            .as_ref()
            .is_some_and(|row| row.package_key == source_key)
        {
            let provide = self.next_provide.take().expect("provide lookahead exists");
            sink.provide(destination_key, provide.ordinal, &provide.value)?;
            self.next_provide = next_ordered_provide(&mut self.provide_rows)?;
        }

        while self
            .next_group
            .as_ref()
            .is_some_and(|row| row.package_key == source_key)
        {
            let group = self.next_group.take().expect("group lookahead exists");
            require_atom_not_before_group(self.next_atom.as_ref(), source_key, group.ordinal)?;
            sink.requirement_group(destination_key, group.ordinal, &group.value)?;
            while self.next_atom.as_ref().is_some_and(|row| {
                row.package_key == source_key && row.group_ordinal == group.ordinal
            }) {
                let atom = self.next_atom.take().expect("atom lookahead exists");
                sink.requirement_atom(
                    destination_key,
                    atom.group_ordinal,
                    atom.ordinal,
                    &atom.value,
                )?;
                self.next_atom = next_ordered_atom(&mut self.atom_rows)?;
            }
            self.next_group = next_ordered_group(&mut self.group_rows)?;
        }
        self.require_no_unowned_atom(source_key)
    }

    fn package_matches(
        &mut self,
        destination: &Connection,
        source_key: &str,
        destination_key: &str,
    ) -> Result<bool> {
        if !self.provides_match(destination, source_key, destination_key)? {
            return Ok(false);
        }
        if !self.groups_match(destination, source_key, destination_key)? {
            return Ok(false);
        }
        if !self.atoms_match(destination, source_key, destination_key)? {
            return Ok(false);
        }
        self.require_no_unowned_atom(source_key)?;
        Ok(true)
    }

    fn provides_match(
        &mut self,
        destination: &Connection,
        source_key: &str,
        destination_key: &str,
    ) -> Result<bool> {
        let mut statement = destination.prepare_cached(
            "SELECT capability, version, version_relation, kind, raw, version_scheme,
                    architecture_qualifier_json, provenance_json, ordinal
             FROM catalog_provides
             WHERE package_key_sha256 = ?1
             ORDER BY ordinal",
        )?;
        let mut destination_rows = statement.query([destination_key])?;
        while self
            .next_provide
            .as_ref()
            .is_some_and(|row| row.package_key == source_key)
        {
            let source = self.next_provide.take().expect("provide lookahead exists");
            let Some(destination_row) = destination_rows.next()? else {
                return Ok(false);
            };
            if source.ordinal != destination_row.get::<_, i64>(8)?
                || source.value != provide_from_row(destination_row)?
            {
                return Ok(false);
            }
            self.next_provide = next_ordered_provide(&mut self.provide_rows)?;
        }
        Ok(destination_rows.next()?.is_none())
    }

    fn groups_match(
        &mut self,
        destination: &Connection,
        source_key: &str,
        destination_key: &str,
    ) -> Result<bool> {
        let mut statement = destination.prepare_cached(
            "SELECT ordinal, kind, behavior, description, native_text, expression_json
             FROM catalog_requirement_groups
             WHERE package_key_sha256 = ?1
             ORDER BY ordinal",
        )?;
        let mut destination_rows = statement.query([destination_key])?;
        while self
            .next_group
            .as_ref()
            .is_some_and(|row| row.package_key == source_key)
        {
            let source = self.next_group.take().expect("group lookahead exists");
            let Some(destination_row) = destination_rows.next()? else {
                return Ok(false);
            };
            if source.ordinal != destination_row.get::<_, i64>(0)?
                || source.value != requirement_group_from_row(destination_row)?
            {
                return Ok(false);
            }
            self.next_group = next_ordered_group(&mut self.group_rows)?;
        }
        Ok(destination_rows.next()?.is_none())
    }

    fn atoms_match(
        &mut self,
        destination: &Connection,
        source_key: &str,
        destination_key: &str,
    ) -> Result<bool> {
        let mut statement = destination.prepare_cached(
            "SELECT capability, version_constraint, kind, dependency_type, raw,
                    group_ordinal, ordinal
             FROM catalog_requirement_atoms
             WHERE package_key_sha256 = ?1
             ORDER BY group_ordinal, ordinal",
        )?;
        let mut destination_rows = statement.query([destination_key])?;
        while self
            .next_atom
            .as_ref()
            .is_some_and(|row| row.package_key == source_key)
        {
            let source = self.next_atom.take().expect("atom lookahead exists");
            let Some(destination_row) = destination_rows.next()? else {
                return Ok(false);
            };
            if source.group_ordinal != destination_row.get::<_, i64>(5)?
                || source.ordinal != destination_row.get::<_, i64>(6)?
                || source.value != requirement_atom_from_row(destination_row)?
            {
                return Ok(false);
            }
            self.next_atom = next_ordered_atom(&mut self.atom_rows)?;
        }
        Ok(destination_rows.next()?.is_none())
    }

    fn require_no_unowned_atom(&self, package_key: &str) -> Result<()> {
        if self
            .next_atom
            .as_ref()
            .is_some_and(|row| row.package_key == package_key)
        {
            return Err(Error::ConflictError(format!(
                "catalog requirement atom references a missing group for package {package_key}"
            )));
        }
        Ok(())
    }

    fn finish(self) -> Result<()> {
        require_no_remaining_relation(self.next_provide.map(|row| row.package_key), "provide")?;
        require_no_remaining_relation(
            self.next_group.map(|row| row.package_key),
            "requirement group",
        )?;
        require_no_remaining_relation(
            self.next_atom.map(|row| row.package_key),
            "requirement atom",
        )
    }
}

struct ProfileRelationSink<'connection> {
    provide: Statement<'connection>,
    requirement_group: Statement<'connection>,
    requirement_atom: Statement<'connection>,
}

impl<'connection> ProfileRelationSink<'connection> {
    fn new(destination: &'connection Connection) -> Result<Self> {
        Ok(Self {
            provide: destination.prepare(
                "INSERT INTO catalog_provides (
                     package_key_sha256, ordinal, capability, version, version_relation,
                     kind, raw, version_scheme, architecture_qualifier_json, provenance_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )?,
            requirement_group: destination.prepare(
                "INSERT INTO catalog_requirement_groups (
                     package_key_sha256, ordinal, kind, behavior, description,
                     native_text, expression_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?,
            requirement_atom: destination.prepare(
                "INSERT INTO catalog_requirement_atoms (
                     package_key_sha256, group_ordinal, ordinal, capability,
                     version_constraint, kind, dependency_type, raw
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?,
        })
    }

    fn provide(
        &mut self,
        package_key: &str,
        ordinal: i64,
        provide: &super::super::CatalogProvideRecordV1,
    ) -> Result<()> {
        self.provide.execute(params![
            package_key,
            ordinal,
            &provide.capability,
            &provide.version,
            provide.version_relation.map(ProvideVersionRelation::as_str),
            &provide.kind,
            &provide.raw,
            provide.version_scheme.as_str(),
            canonical_json_string(&provide.architecture_qualifier)?,
            canonical_json_string(&provide.provenance)?,
        ])?;
        Ok(())
    }

    fn requirement_group(
        &mut self,
        package_key: &str,
        ordinal: i64,
        group: &CatalogRequirementGroupV1,
    ) -> Result<()> {
        self.requirement_group.execute(params![
            package_key,
            ordinal,
            &group.kind,
            &group.behavior,
            &group.description,
            &group.native_text,
            &group.expression_json,
        ])?;
        Ok(())
    }

    fn requirement_atom(
        &mut self,
        package_key: &str,
        group_ordinal: i64,
        ordinal: i64,
        atom: &CatalogRequirementAtomV1,
    ) -> Result<()> {
        self.requirement_atom.execute(params![
            package_key,
            group_ordinal,
            ordinal,
            &atom.capability,
            &atom.version_constraint,
            &atom.kind,
            &atom.dependency_type,
            &atom.raw,
        ])?;
        Ok(())
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
    let mut provides = connection.prepare(SELECT_ORDERED_PROVIDES)?;
    let mut groups = connection.prepare(SELECT_ORDERED_REQUIREMENT_GROUPS)?;
    let mut atoms = connection.prepare(SELECT_ORDERED_REQUIREMENT_ATOMS)?;
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
    ordinal: i64,
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
    ordinal: i64,
    value: CatalogRequirementAtomV1,
}

fn next_ordered_provide(rows: &mut Rows<'_>) -> Result<Option<OrderedProvide>> {
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    Ok(Some(OrderedProvide {
        package_key: row.get(8)?,
        ordinal: row.get(9)?,
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
        ordinal: row.get(7)?,
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
