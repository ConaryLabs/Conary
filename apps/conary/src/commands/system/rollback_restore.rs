// apps/conary/src/commands/system/rollback_restore.rs

//! Exact reconstruction of installed authority from rollback metadata.

mod materialized_directories;

use crate::commands::installed_authority_snapshot::{
    ComponentSnapshot, ConfigFileSnapshot, DirectoryClaimSnapshot, InstalledConversionSnapshot,
    ProvenanceSnapshot, TroveSnapshot,
};
use conary_core::Error;
use conary_core::db::models::{
    CollectionMember, Component, ComponentDependency, ComponentProvide, ConfigFile,
    ConvertedArtifactKind, ConvertedPackage, DirectoryClaim, FileEntry, Flavor,
    InstalledCcsRemoveHook, InstalledFileCapability, InstalledRequirementGroup,
    NativeLifecycleResidualState, ProvideEntry, Trove,
};
use rusqlite::{OptionalExtension, Transaction, params};
use std::collections::BTreeMap;

#[cfg(test)]
mod tests;

pub(super) use materialized_directories::restore_materialized_directories;

struct RestoredSnapshot<'a> {
    snapshot: &'a TroveSnapshot,
    trove_id: i64,
    component_ids: BTreeMap<String, i64>,
    file_ids: BTreeMap<String, i64>,
}

#[cfg(test)]
pub(super) fn restore_snapshot(
    tx: &Transaction<'_>,
    rollback_changeset_id: i64,
    snapshot: &TroveSnapshot,
) -> conary_core::Result<()> {
    restore_snapshots(tx, rollback_changeset_id, std::slice::from_ref(snapshot))
}

pub(super) fn restore_snapshots(
    tx: &Transaction<'_>,
    rollback_changeset_id: i64,
    snapshots: &[TroveSnapshot],
) -> conary_core::Result<()> {
    validate_snapshot_set(snapshots)?;

    let mut restored = snapshots
        .iter()
        .map(|snapshot| restore_snapshot_base(tx, rollback_changeset_id, snapshot))
        .collect::<conary_core::Result<Vec<_>>>()?;

    // Directory ownership is a graph, not a package ordering problem. First
    // materialize every canonical anchor, then restore all claims. This also
    // handles cyclic cross-claims such as package A anchoring /usr while
    // claiming package B's /opt, and package B doing the reverse.
    for authority in &mut restored {
        restore_directory_anchors(tx, authority)?;
    }
    for authority in &mut restored {
        restore_directory_claims(tx, authority)?;
    }

    // These projections reference restored paths or otherwise depend on the
    // complete package set. Keep them after the global directory graph.
    for authority in restored {
        restore_snapshot_dependents(tx, rollback_changeset_id, authority)?;
    }
    Ok(())
}

fn validate_snapshot_set(snapshots: &[TroveSnapshot]) -> conary_core::Result<()> {
    let mut anchors = BTreeMap::new();
    for snapshot in snapshots {
        snapshot.validate().map_err(snapshot_error)?;
        for claim in &snapshot.directory_claims {
            if claim.anchor.is_some()
                && let Some(existing) = anchors.insert(claim.path.as_str(), snapshot.name.as_str())
            {
                return Err(Error::ConflictError(format!(
                    "rollback snapshots '{}' and '{}' both claim canonical directory anchor '{}'",
                    existing, snapshot.name, claim.path
                )));
            }
        }
    }
    Ok(())
}

fn restore_snapshot_base<'a>(
    tx: &Transaction<'_>,
    rollback_changeset_id: i64,
    snapshot: &'a TroveSnapshot,
) -> conary_core::Result<RestoredSnapshot<'a>> {
    let mut trove = Trove::new_with_source(
        snapshot.name.clone(),
        snapshot.version.clone(),
        snapshot.trove_type.clone(),
        snapshot.install_source.clone(),
        snapshot.version_scheme,
    );
    trove.package_release = snapshot.package_release.clone();
    trove.architecture = snapshot.architecture.clone();
    trove.debian_multi_arch = snapshot.debian_multi_arch;
    trove.description = snapshot.description.clone();
    trove.installed_by_changeset_id = Some(rollback_changeset_id);
    trove.install_reason = snapshot.install_reason.clone();
    trove.flavor_spec = snapshot.flavor_spec.clone();
    trove.pinned = snapshot.pinned;
    trove.selection_reason = snapshot.selection_reason.clone();
    trove.label_id = snapshot.label_id;
    trove.source_profile = snapshot.source_profile.clone();
    trove.native_package_identity = snapshot.native_package_identity.clone();
    trove.installed_from_repository_id = snapshot.installed_from_repository_id;
    let trove_id = trove.insert(tx)?;
    tx.execute(
        "UPDATE troves SET installed_at = ?1, orphan_since = ?2 WHERE id = ?3",
        params![&snapshot.installed_at, &snapshot.orphan_since, trove_id],
    )?;

    restore_flavors(tx, trove_id, snapshot)?;
    restore_provenance(tx, trove_id, snapshot.provenance.as_ref())?;
    restore_conversions(tx, trove_id, &snapshot.installed_conversions)?;
    restore_collection_members(tx, trove_id, snapshot)?;
    let component_ids = restore_components(tx, trove_id, &snapshot.components)?;
    restore_requirements(tx, trove_id, snapshot)?;
    restore_provides(tx, trove_id, snapshot)?;
    let file_ids = restore_files(
        tx,
        rollback_changeset_id,
        trove_id,
        snapshot,
        &component_ids,
    )?;
    Ok(RestoredSnapshot {
        snapshot,
        trove_id,
        component_ids,
        file_ids,
    })
}

fn restore_directory_anchors(
    tx: &Transaction<'_>,
    authority: &mut RestoredSnapshot<'_>,
) -> conary_core::Result<()> {
    for claim in authority
        .snapshot
        .directory_claims
        .iter()
        .filter(|claim| claim.anchor.is_some())
    {
        let anchor = claim
            .anchor
            .as_ref()
            .expect("filtered rollback directory anchor");
        let component_id = directory_anchor_component_id(authority, claim)?;
        let file_id = FileEntry::restore_materialized_directory_anchor(
            tx,
            &claim.path,
            &anchor.materialized_node,
            claim.anchor_policy,
            authority.trove_id,
            component_id,
            &anchor.installed_at,
        )?;
        authority.file_ids.insert(claim.path.clone(), file_id);
    }
    Ok(())
}

fn directory_anchor_component_id(
    authority: &RestoredSnapshot<'_>,
    claim: &DirectoryClaimSnapshot,
) -> conary_core::Result<Option<i64>> {
    claim
        .anchor
        .as_ref()
        .and_then(|anchor| anchor.component.as_ref())
        .map(|component| {
            authority
                .component_ids
                .get(component)
                .copied()
                .ok_or_else(|| {
                    Error::InitError(format!(
                        "rollback directory anchor '{}' references missing component '{}'",
                        claim.path, component
                    ))
                })
        })
        .transpose()
}

fn restore_directory_claims(
    tx: &Transaction<'_>,
    authority: &mut RestoredSnapshot<'_>,
) -> conary_core::Result<()> {
    for claim in &authority.snapshot.directory_claims {
        let existing = FileEntry::find_by_path(tx, &claim.path)?.ok_or_else(|| {
            Error::ConflictError(format!(
                "rollback directory claim '{}' has no materialized anchor",
                claim.path
            ))
        })?;
        if !claim.anchor_policy.accepts(&existing.node.source.kind) {
            return Err(Error::ConflictError(format!(
                "rollback directory claim '{}' anchor is not accepted by policy '{}'",
                claim.path,
                claim.anchor_policy.as_str()
            )));
        }
        let file_id = existing.id.ok_or_else(|| {
            Error::MissingId(format!(
                "rollback directory anchor '{}' has no database identity",
                claim.path
            ))
        })?;
        DirectoryClaim::new(claim.path.clone(), authority.trove_id, claim.node.clone())?
            .with_component(directory_claim_component_id(authority, claim)?)
            .with_anchor_policy(claim.anchor_policy)
            .with_materialization_target(claim.materialization_target_path.clone())
            .insert(tx)?;
        let changed = tx.execute(
            "UPDATE directory_claims SET claimed_at = ?1
             WHERE path = ?2 AND trove_id = ?3",
            params![&claim.claimed_at, &claim.path, authority.trove_id],
        )?;
        if changed != 1 {
            return Err(Error::InternalError(format!(
                "rollback directory claim '{}' was not restored exactly",
                claim.path
            )));
        }
        if claim.anchor.is_some() {
            authority.file_ids.insert(claim.path.clone(), file_id);
        }
    }
    Ok(())
}

fn directory_claim_component_id(
    authority: &RestoredSnapshot<'_>,
    claim: &DirectoryClaimSnapshot,
) -> conary_core::Result<Option<i64>> {
    claim
        .component
        .as_ref()
        .map(|component| {
            authority
                .component_ids
                .get(component)
                .copied()
                .ok_or_else(|| {
                    Error::InitError(format!(
                        "rollback directory claim '{}' references missing component '{}'",
                        claim.path, component
                    ))
                })
        })
        .transpose()
}

fn restore_snapshot_dependents(
    tx: &Transaction<'_>,
    rollback_changeset_id: i64,
    authority: RestoredSnapshot<'_>,
) -> conary_core::Result<()> {
    restore_config_files(
        tx,
        authority.trove_id,
        authority.snapshot,
        &authority.file_ids,
    )?;
    restore_package_capabilities(tx, authority.trove_id, authority.snapshot)?;
    restore_file_capabilities(tx, authority.trove_id, authority.snapshot)?;
    restore_native_lifecycle(
        tx,
        rollback_changeset_id,
        authority.trove_id,
        authority.snapshot,
    )?;
    restore_ccs_remove_hook(tx, authority.trove_id, authority.snapshot)?;
    restore_derived_references(tx, authority.trove_id, authority.snapshot)?;
    Ok(())
}

fn restore_flavors(
    tx: &Transaction<'_>,
    trove_id: i64,
    snapshot: &TroveSnapshot,
) -> conary_core::Result<()> {
    for flavor in &snapshot.flavors {
        Flavor::new(trove_id, flavor.key.clone(), flavor.value.clone()).insert(tx)?;
    }
    Ok(())
}

fn restore_collection_members(
    tx: &Transaction<'_>,
    trove_id: i64,
    snapshot: &TroveSnapshot,
) -> conary_core::Result<()> {
    for member in &snapshot.collection_members {
        let mut restored = CollectionMember::new(trove_id, member.member_name.clone());
        restored.member_version = member.member_version.clone();
        restored.is_optional = member.is_optional;
        restored.insert(tx)?;
    }
    Ok(())
}

fn restore_components(
    tx: &Transaction<'_>,
    trove_id: i64,
    snapshots: &[ComponentSnapshot],
) -> conary_core::Result<BTreeMap<String, i64>> {
    let mut component_ids = BTreeMap::new();
    for snapshot in snapshots {
        let mut component = Component::new(trove_id, snapshot.name.clone());
        component.description = snapshot.description.clone();
        component.is_installed = snapshot.is_installed;
        let component_id = component.insert(tx)?;
        tx.execute(
            "UPDATE components SET installed_at = ?1 WHERE id = ?2",
            params![&snapshot.installed_at, component_id],
        )?;
        component_ids.insert(snapshot.name.clone(), component_id);
        restore_component_relations(tx, component_id, snapshot)?;
    }
    Ok(component_ids)
}

fn restore_component_relations(
    tx: &Transaction<'_>,
    component_id: i64,
    snapshot: &ComponentSnapshot,
) -> conary_core::Result<()> {
    for dependency in &snapshot.dependencies {
        let mut restored = match dependency.depends_on_package.as_ref() {
            Some(package) => ComponentDependency::new_cross_package(
                component_id,
                dependency.depends_on_component.clone(),
                package.clone(),
                dependency.dependency_type,
            ),
            None => ComponentDependency::new_same_package(
                component_id,
                dependency.depends_on_component.clone(),
                dependency.dependency_type,
            ),
        };
        restored.version_constraint = dependency.version_constraint.clone();
        restored.insert(tx)?;
    }
    for provide in &snapshot.provides {
        let mut restored = ComponentProvide::new(component_id, provide.capability.clone());
        restored.version = provide.version.clone();
        restored.insert(tx)?;
    }
    Ok(())
}

fn restore_requirements(
    tx: &Transaction<'_>,
    trove_id: i64,
    snapshot: &TroveSnapshot,
) -> conary_core::Result<()> {
    for requirement in &snapshot.requirements {
        InstalledRequirementGroup::from_group(
            trove_id,
            requirement.version_scheme,
            requirement.requirement.clone(),
        )?
        .insert(tx)?;
    }
    Ok(())
}

fn restore_provides(
    tx: &Transaction<'_>,
    trove_id: i64,
    snapshot: &TroveSnapshot,
) -> conary_core::Result<()> {
    for provide in &snapshot.provides {
        ProvideEntry::new_typed(
            trove_id,
            provide.kind,
            provide.capability.clone(),
            provide.version.clone(),
            provide.version_scheme,
            provide.architecture_qualifier.clone(),
        )
        .with_version_relation(provide.version_relation)
        .insert(tx)?;
    }
    Ok(())
}

fn restore_files(
    tx: &Transaction<'_>,
    rollback_changeset_id: i64,
    trove_id: i64,
    snapshot: &TroveSnapshot,
    component_ids: &BTreeMap<String, i64>,
) -> conary_core::Result<BTreeMap<String, i64>> {
    let mut file_ids = BTreeMap::new();
    for file in &snapshot.files {
        let component_id = file
            .component
            .as_ref()
            .map(|component| {
                component_ids.get(component).copied().ok_or_else(|| {
                    Error::InitError(format!(
                        "rollback snapshot file '{}' references missing component '{}'",
                        file.path, component
                    ))
                })
            })
            .transpose()?;
        let file_id = if let Some(existing) = FileEntry::find_by_path(tx, &file.path)? {
            restore_exact_reanchored_file(tx, &existing, file, trove_id, component_id)?
        } else if matches!(
            file.node.source.kind,
            conary_core::payload::PayloadNodeKind::Directory
        ) {
            FileEntry::restore_materialized_directory_anchor(
                tx,
                &file.path,
                &file.node,
                conary_core::db::models::DirectoryClaimAnchorPolicy::Directory,
                trove_id,
                component_id,
                &file.installed_at,
            )?
        } else {
            let mut restored = match component_id {
                Some(component_id) => FileEntry::new_with_component(
                    file.path.clone(),
                    file.node.clone(),
                    file.content.clone(),
                    trove_id,
                    component_id,
                ),
                None => FileEntry::new(
                    file.path.clone(),
                    file.node.clone(),
                    file.content.clone(),
                    trove_id,
                ),
            };
            let file_id = restored.insert(tx)?;
            tx.execute(
                "UPDATE files SET installed_at = ?1 WHERE id = ?2",
                params![&file.installed_at, file_id],
            )?;
            file_id
        };
        file_ids.insert(file.path.clone(), file_id);
        if let Some(content) = file.content.as_ref() {
            tx.execute(
                "INSERT INTO file_history
                 (changeset_id, path, sha256_hash, action)
                 VALUES (?1, ?2, ?3, 'add')",
                params![rollback_changeset_id, &file.path, &content.sha256],
            )?;
        }
    }
    Ok(file_ids)
}

fn restore_exact_reanchored_file(
    tx: &Transaction<'_>,
    existing: &FileEntry,
    snapshot: &crate::commands::installed_authority_snapshot::FileSnapshot,
    trove_id: i64,
    component_id: Option<i64>,
) -> conary_core::Result<i64> {
    if existing.node != snapshot.node
        || existing.content != snapshot.content
        || existing.installed_at.as_deref() != Some(snapshot.installed_at.as_str())
    {
        return Err(Error::ConflictError(format!(
            "rollback file '{}' conflicts with a different existing materialized node",
            snapshot.path
        )));
    }
    let retainers = DirectoryClaim::find_retaining_path(tx, &snapshot.path)?;
    if !retainers
        .iter()
        .any(|claim| claim.trove_id == existing.trove_id)
    {
        return Err(Error::ConflictError(format!(
            "rollback file '{}' already exists without exact shared-directory reanchor authority",
            snapshot.path
        )));
    }
    let file_id = existing.id.ok_or_else(|| {
        Error::MissingId(format!(
            "rollback file '{}' existing materialization has no database identity",
            snapshot.path
        ))
    })?;
    let changed = tx.execute(
        "UPDATE files
         SET trove_id = ?1, component_id = ?2
         WHERE id = ?3",
        params![trove_id, component_id, file_id],
    )?;
    if changed != 1 {
        return Err(Error::InternalError(format!(
            "rollback file '{}' was not reanchored exactly",
            snapshot.path
        )));
    }
    Ok(file_id)
}

fn restore_config_files(
    tx: &Transaction<'_>,
    trove_id: i64,
    snapshot: &TroveSnapshot,
    file_ids: &BTreeMap<String, i64>,
) -> conary_core::Result<()> {
    for config in &snapshot.config_files {
        validate_existing_config_owner(tx, snapshot, config)?;
        let file_id = if config.file_backed {
            Some(*file_ids.get(&config.path).ok_or_else(|| {
                Error::InitError(format!(
                    "rollback snapshot config '{}' references a missing restored file",
                    config.path
                ))
            })?)
        } else {
            None
        };
        let mut restored = ConfigFile {
            id: None,
            file_id,
            path: config.path.clone(),
            trove_id: Some(trove_id),
            original_hash: config.original_hash.clone(),
            original_md5: config.original_md5.clone(),
            current_hash: config.current_hash.clone(),
            noreplace: config.noreplace,
            ghost: config.ghost,
            remove_on_upgrade: config.remove_on_upgrade,
            status: config.status,
            modified_at: config.modified_at.clone(),
            source: config.source,
        };
        let config_id = restored.upsert(tx)?;
        tx.execute(
            "DELETE FROM config_backups WHERE config_file_id = ?1",
            [config_id],
        )?;
        for backup in &config.backups {
            tx.execute(
                "INSERT INTO config_backups (
                    config_file_id, backup_hash, reason, changeset_id, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    config_id,
                    &backup.backup_hash,
                    &backup.reason,
                    backup.changeset_id,
                    &backup.created_at,
                ],
            )?;
        }
    }
    Ok(())
}

fn validate_existing_config_owner(
    tx: &Transaction<'_>,
    snapshot: &TroveSnapshot,
    config: &ConfigFileSnapshot,
) -> conary_core::Result<()> {
    let existing = tx
        .query_row(
            "SELECT trove_id, package_name, package_version, package_architecture
             FROM config_files WHERE path = ?1",
            [&config.path],
            |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .optional()?;
    let Some((owner_id, name, version, architecture)) = existing else {
        return Ok(());
    };
    if owner_id.is_some()
        || name != snapshot.name
        || version != snapshot.version
        || architecture != snapshot.architecture
    {
        return Err(Error::InitError(format!(
            "rollback cannot reclaim config '{}' from installed owner {} {} {:?}",
            config.path, name, version, architecture
        )));
    }
    Ok(())
}

fn restore_package_capabilities(
    tx: &Transaction<'_>,
    trove_id: i64,
    snapshot: &TroveSnapshot,
) -> conary_core::Result<()> {
    let Some(capabilities) = &snapshot.package_capabilities else {
        return Ok(());
    };
    conary_core::capability::store_capabilities(tx, trove_id, &capabilities.declaration)
        .map_err(snapshot_error)?;
    tx.execute(
        "UPDATE capabilities
         SET declaration_version = ?1, declared_at = ?2
         WHERE trove_id = ?3",
        params![
            capabilities.declaration_version,
            &capabilities.declared_at,
            trove_id
        ],
    )?;
    Ok(())
}

fn restore_file_capabilities(
    tx: &Transaction<'_>,
    trove_id: i64,
    snapshot: &TroveSnapshot,
) -> conary_core::Result<()> {
    let capabilities = snapshot
        .installed_file_capabilities
        .iter()
        .map(|capability| capability.capability.clone())
        .collect::<Vec<_>>();
    InstalledFileCapability::replace_for_trove(tx, trove_id, &capabilities)
        .map_err(snapshot_error)?;
    for capability in &snapshot.installed_file_capabilities {
        tx.execute(
            "UPDATE installed_file_capabilities
             SET created_at = ?1
             WHERE trove_id = ?2 AND path = ?3",
            params![
                &capability.created_at,
                trove_id,
                &capability.capability.path
            ],
        )?;
    }
    Ok(())
}

fn restore_native_lifecycle(
    tx: &Transaction<'_>,
    rollback_changeset_id: i64,
    trove_id: i64,
    snapshot: &TroveSnapshot,
) -> conary_core::Result<()> {
    let Some(native) = &snapshot.native_lifecycle else {
        return Ok(());
    };
    let bundle: conary_core::ccs::native_lifecycle::NativeLifecycleBundle =
        toml::from_str(&native.bundle_toml).map_err(snapshot_error)?;
    if bundle.source_package != snapshot.name
        || bundle.source_version != snapshot.version
        || bundle.version_scheme.as_str() != snapshot.version_scheme.as_str()
        || bundle.source_arch != snapshot.architecture
    {
        return Err(Error::InitError(format!(
            "rollback snapshot '{}' native lifecycle identity does not match its installed package identity",
            snapshot.name
        )));
    }
    let lifecycle_state =
        conary_core::ccs::native_transaction::DebPackageState::parse(&native.lifecycle_state)
            .map_err(snapshot_error)?;
    tx.execute(
        "INSERT INTO installed_native_lifecycle_bundles (
            trove_id, source_format, source_family, source_profile, source_release,
            source_arch, source_package, source_version, scriptlet_fidelity,
            evidence_digest, lifecycle_state, pending_triggers_json,
            awaited_packages_json, bundle_toml, installed_changeset_id, installed_at
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16
         )",
        params![
            trove_id,
            &native.source_format,
            &native.source_family,
            &native.source_profile,
            &native.source_release,
            &native.source_arch,
            &native.source_package,
            &native.source_version,
            &native.scriptlet_fidelity,
            &native.evidence_digest,
            lifecycle_state.as_str(),
            serde_json::to_string(&native.pending_triggers).map_err(snapshot_error)?,
            serde_json::to_string(&native.awaited_packages).map_err(snapshot_error)?,
            &native.bundle_toml,
            rollback_changeset_id,
            &native.installed_at,
        ],
    )?;
    let identity = conary_core::ccs::native_transaction::NativePackageIdentity::new(
        &bundle.source_package,
        &bundle.source_version,
        bundle.source_arch.as_deref(),
    );
    NativeLifecycleResidualState::delete_exact(tx, bundle.source_format.as_str(), &identity)
        .map_err(snapshot_error)?;
    Ok(())
}

fn restore_ccs_remove_hook(
    tx: &Transaction<'_>,
    trove_id: i64,
    snapshot: &TroveSnapshot,
) -> conary_core::Result<()> {
    if let Some(hook) = &snapshot.ccs_remove_hook {
        InstalledCcsRemoveHook::new(trove_id, hook.script.clone(), hook.reversible)
            .insert_or_replace(tx)?;
    }
    Ok(())
}

fn restore_derived_references(
    tx: &Transaction<'_>,
    trove_id: i64,
    snapshot: &TroveSnapshot,
) -> conary_core::Result<()> {
    for reference in &snapshot.derived_package_references {
        if reference.parent {
            let changed = tx.execute(
                "UPDATE derived_packages SET parent_trove_id = ?1
                 WHERE id = ?2 AND parent_trove_id IS NULL",
                params![trove_id, reference.derived_package_id],
            )?;
            if changed != 1 {
                return Err(reference_conflict(reference.derived_package_id, "parent"));
            }
        }
        if reference.built {
            let changed = tx.execute(
                "UPDATE derived_packages SET built_trove_id = ?1
                 WHERE id = ?2 AND built_trove_id IS NULL",
                params![trove_id, reference.derived_package_id],
            )?;
            if changed != 1 {
                return Err(reference_conflict(reference.derived_package_id, "built"));
            }
        }
    }
    Ok(())
}

fn restore_conversions(
    tx: &Transaction<'_>,
    trove_id: i64,
    snapshots: &[InstalledConversionSnapshot],
) -> conary_core::Result<()> {
    for snapshot in snapshots {
        validate_conversion_snapshot(trove_id, snapshot)?;
        tx.execute(
            "INSERT INTO converted_packages (
                artifact_kind, trove_id, original_format, original_checksum,
                conversion_version, converted_at, enhancement_version,
                extracted_provenance_json, enhancement_status, enhancement_error,
                enhancement_attempted_at, enhancement_priority, scriptlet_fidelity,
                evidence_digest, scriptlet_summary_json
             ) VALUES (
                'installed', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14
             )",
            params![
                trove_id,
                &snapshot.original_format,
                &snapshot.original_checksum,
                snapshot.conversion_version,
                &snapshot.converted_at,
                snapshot.enhancement_version,
                &snapshot.extracted_provenance_json,
                &snapshot.enhancement_status,
                &snapshot.enhancement_error,
                &snapshot.enhancement_attempted_at,
                snapshot.enhancement_priority,
                &snapshot.scriptlet_fidelity,
                &snapshot.evidence_digest,
                &snapshot.scriptlet_summary_json,
            ],
        )?;
    }
    Ok(())
}

fn validate_conversion_snapshot(
    trove_id: i64,
    snapshot: &InstalledConversionSnapshot,
) -> conary_core::Result<()> {
    let converted = ConvertedPackage {
        id: None,
        artifact_kind: ConvertedArtifactKind::Installed,
        trove_id: Some(trove_id),
        original_format: snapshot.original_format.clone(),
        original_checksum: snapshot.original_checksum.clone(),
        conversion_version: snapshot.conversion_version,
        converted_at: Some(snapshot.converted_at.clone()),
        enhancement_version: snapshot.enhancement_version,
        extracted_provenance_json: snapshot.extracted_provenance_json.clone(),
        enhancement_status: snapshot.enhancement_status.clone(),
        enhancement_error: snapshot.enhancement_error.clone(),
        enhancement_attempted_at: snapshot.enhancement_attempted_at.clone(),
        package_name: None,
        package_version: None,
        source_profile: None,
        package_architecture: None,
        chunk_hashes_json: None,
        total_size: None,
        content_hash: None,
        ccs_path: None,
        scriptlet_fidelity: snapshot.scriptlet_fidelity.clone(),
        evidence_digest: snapshot.evidence_digest.clone(),
        scriptlet_summary_json: snapshot.scriptlet_summary_json.clone(),
    };
    converted.scriptlet_summary()?;
    Ok(())
}

fn restore_provenance(
    tx: &Transaction<'_>,
    trove_id: i64,
    snapshot: Option<&ProvenanceSnapshot>,
) -> conary_core::Result<()> {
    let Some(snapshot) = snapshot else {
        return Ok(());
    };
    tx.execute(
        "INSERT INTO provenance (
            trove_id, source_url, source_branch, source_commit, build_host, build_time,
            builder, upstream_url, upstream_hash, git_repo, git_tag, fetch_timestamp,
            patches_json, recipe_hash, build_deps_json, host_arch, host_kernel,
            host_distro, build_start, build_end, build_log_hash, isolation_level,
            reproducibility_json, signatures_json, rekor_log_index, sbom_spdx_hash,
            sbom_cyclonedx_hash, merkle_root, component_hashes_json,
            chunk_manifest_json, total_size, file_count, dna_hash
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
            ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26,
            ?27, ?28, ?29, ?30, ?31, ?32, ?33
         )",
        params![
            trove_id,
            &snapshot.source_url,
            &snapshot.source_branch,
            &snapshot.source_commit,
            &snapshot.build_host,
            &snapshot.build_time,
            &snapshot.builder,
            &snapshot.upstream_url,
            &snapshot.upstream_hash,
            &snapshot.git_repo,
            &snapshot.git_tag,
            &snapshot.fetch_timestamp,
            &snapshot.patches_json,
            &snapshot.recipe_hash,
            &snapshot.build_deps_json,
            &snapshot.host_arch,
            &snapshot.host_kernel,
            &snapshot.host_distro,
            &snapshot.build_start,
            &snapshot.build_end,
            &snapshot.build_log_hash,
            &snapshot.isolation_level,
            &snapshot.reproducibility_json,
            &snapshot.signatures_json,
            snapshot.rekor_log_index,
            &snapshot.sbom_spdx_hash,
            &snapshot.sbom_cyclonedx_hash,
            &snapshot.merkle_root,
            &snapshot.component_hashes_json,
            &snapshot.chunk_manifest_json,
            snapshot.total_size,
            snapshot.file_count,
            &snapshot.dna_hash,
        ],
    )?;
    Ok(())
}

fn reference_conflict(derived_package_id: i64, role: &str) -> Error {
    Error::InitError(format!(
        "rollback cannot restore derived package {} {} reference because it no longer has the expected empty target",
        derived_package_id, role
    ))
}

fn snapshot_error(error: impl std::fmt::Display) -> Error {
    Error::InitError(error.to_string())
}
