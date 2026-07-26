// apps/conary/src/commands/installed_authority_snapshot/capture.rs

use super::{
    CcsRemoveHookSnapshot, CollectionMemberSnapshot, ComponentDependencySnapshot,
    ComponentProvideSnapshot, ComponentSnapshot, ConfigBackupSnapshot, ConfigFileSnapshot,
    DerivedPackageReferenceSnapshot, DirectoryAnchorSnapshot, DirectoryClaimSnapshot, FileSnapshot,
    FlavorSnapshot, InstalledConversionSnapshot, InstalledFileCapabilitySnapshot,
    InstalledRequirementSnapshot, MaterializedDirectorySnapshot, NativeLifecycleSnapshot,
    PackageCapabilitySnapshot, ProvenanceSnapshot, ProvideSnapshot, StableTroveIdentitySnapshot,
    TroveSnapshot,
};
use anyhow::{Context, Result, anyhow, bail};
use conary_core::capability::load_capabilities;
use conary_core::ccs::manifest::FileCapability;
use conary_core::db::models::{
    CollectionMember, Component, ComponentDependency, ComponentProvide, ConfigBackup, ConfigFile,
    ConvertedArtifactKind, ConvertedPackage, DirectoryClaim, DirectoryClaimAnchorPolicy, FileEntry,
    Flavor, InstalledFileCapability, InstalledNativeLifecycleBundle, InstalledRequirementGroup,
    ProvideEntry, Trove,
};
use conary_core::payload::PayloadNodeKind;
use rusqlite::{Connection, OptionalExtension};
use std::collections::BTreeMap;

pub(crate) fn capture_materialized_directory_snapshots(
    conn: &Connection,
    directories: impl IntoIterator<Item = FileEntry>,
) -> Result<Vec<MaterializedDirectorySnapshot>> {
    let mut captured = BTreeMap::new();
    for directory in directories {
        if !matches!(
            directory.node.source.kind,
            PayloadNodeKind::Directory | PayloadNodeKind::Symlink { .. }
        ) {
            bail!(
                "pre-mutation materialized path '{}' is neither a directory nor a verified symlink-to-directory",
                directory.path
            );
        }
        let installed_at = directory.installed_at.clone().ok_or_else(|| {
            anyhow!(
                "pre-mutation materialized directory '{}' has no installed_at authority",
                directory.path
            )
        })?;
        let trove = Trove::find_by_id(conn, directory.trove_id)?.ok_or_else(|| {
            anyhow!(
                "pre-mutation materialized directory '{}' references missing anchor trove {}",
                directory.path,
                directory.trove_id
            )
        })?;
        let trove_installed_at = trove.installed_at.clone().ok_or_else(|| {
            anyhow!(
                "materialized directory anchor trove '{}-{}' has no installed_at authority",
                trove.name,
                trove.version
            )
        })?;
        let anchor_component = directory
            .component_id
            .map(|component_id| {
                let component = Component::find_by_id(conn, component_id)?.ok_or_else(|| {
                    anyhow!(
                        "pre-mutation materialized directory '{}' references missing component {}",
                        directory.path,
                        component_id
                    )
                })?;
                if component.parent_trove_id != directory.trove_id {
                    bail!(
                        "pre-mutation materialized directory '{}' component {} does not belong to anchor trove {}",
                        directory.path,
                        component_id,
                        directory.trove_id
                    );
                }
                Ok(component.name)
            })
            .transpose()?;
        let claims = DirectoryClaim::find_retaining_path(conn, &directory.path)?;
        if claims.is_empty() {
            bail!(
                "pre-mutation materialized directory '{}' has no directory-claim authority",
                directory.path
            );
        }
        for claim in &claims {
            if !claim.anchor_policy.accepts(&directory.node.source.kind) {
                bail!(
                    "pre-mutation materialized directory '{}' is incompatible with claim policy '{}' for trove {}",
                    directory.path,
                    claim.anchor_policy.as_str(),
                    claim.trove_id
                );
            }
        }
        let anchor_policy = match directory.node.source.kind {
            PayloadNodeKind::Directory => DirectoryClaimAnchorPolicy::Directory,
            PayloadNodeKind::Symlink { .. } => {
                DirectoryClaimAnchorPolicy::DirectoryOrSymlinkToDirectory
            }
            _ => unreachable!("materialized node kind was checked above"),
        };
        let snapshot = MaterializedDirectorySnapshot {
            path: directory.path.clone(),
            materialized_node: directory.node,
            anchor_policy,
            installed_at,
            anchor_trove: StableTroveIdentitySnapshot {
                name: trove.name,
                version: trove.version,
                trove_type: trove.trove_type,
                architecture: trove.architecture,
                installed_at: trove_installed_at,
                version_scheme: trove.version_scheme,
            },
            anchor_component,
        };
        snapshot.validate()?;
        if captured.insert(directory.path.clone(), snapshot).is_some() {
            bail!(
                "pre-mutation materialized directory '{}' was captured more than once",
                directory.path
            );
        }
    }
    Ok(captured.into_values().collect())
}

pub(crate) fn capture_trove_snapshot(
    conn: &Connection,
    trove: &Trove,
    ccs_remove_hook: Option<&conary_core::db::models::InstalledCcsRemoveHook>,
    files: &[FileEntry],
) -> Result<TroveSnapshot> {
    let trove_id = trove.id.ok_or_else(|| anyhow!("Trove has no ID"))?;
    let installed_at = trove
        .installed_at
        .clone()
        .ok_or_else(|| anyhow!("installed trove '{}' has no installed_at", trove.name))?;
    let (components, component_names) = capture_components(conn, trove_id)?;
    let directory_claims = capture_directory_claims(conn, trove_id, files, &component_names)?;
    let file_snapshots = capture_files(files, &component_names, &directory_claims)?;
    let snapshot = TroveSnapshot {
        name: trove.name.clone(),
        version: trove.version.clone(),
        package_release: trove.package_release.clone(),
        trove_type: trove.trove_type.clone(),
        architecture: trove.architecture.clone(),
        debian_multi_arch: trove.debian_multi_arch,
        description: trove.description.clone(),
        installed_at,
        install_source: trove.install_source.clone(),
        install_reason: trove.install_reason.clone(),
        flavor_spec: trove.flavor_spec.clone(),
        pinned: trove.pinned,
        selection_reason: trove.selection_reason.clone(),
        label_id: trove.label_id,
        orphan_since: trove.orphan_since.clone(),
        source_profile: trove.source_profile.clone(),
        version_scheme: trove.version_scheme,
        native_package_identity: trove.native_package_identity.clone(),
        installed_from_repository_id: trove.installed_from_repository_id,
        flavors: Flavor::find_by_trove(conn, trove_id)?
            .into_iter()
            .map(|flavor| FlavorSnapshot {
                key: flavor.key,
                value: flavor.value,
            })
            .collect(),
        provenance: capture_provenance(conn, trove_id)?,
        installed_conversions: capture_installed_conversions(conn, trove_id)?,
        components,
        collection_members: CollectionMember::find_by_collection(conn, trove_id)?
            .into_iter()
            .map(|member| CollectionMemberSnapshot {
                member_name: member.member_name,
                member_version: member.member_version,
                is_optional: member.is_optional,
            })
            .collect(),
        requirements: InstalledRequirementGroup::find_by_trove(conn, trove_id)?
            .into_iter()
            .map(|requirement| InstalledRequirementSnapshot {
                version_scheme: requirement.version_scheme,
                requirement: requirement.requirement,
            })
            .collect(),
        provides: capture_provides(conn, trove_id)?,
        config_files: capture_config_files(conn, trove_id, files)?,
        package_capabilities: capture_package_capabilities(conn, trove_id)?,
        installed_file_capabilities: capture_file_capabilities(conn, trove_id)?,
        directory_claims,
        native_lifecycle: capture_native_lifecycle(conn, trove_id)?,
        ccs_remove_hook: ccs_remove_hook.map(|hook| CcsRemoveHookSnapshot {
            script: hook.script.clone(),
            reversible: hook.reversible,
        }),
        derived_package_references: capture_derived_references(conn, trove_id)?,
        files: file_snapshots,
    };
    snapshot.validate()?;
    Ok(snapshot)
}

fn capture_components(
    conn: &Connection,
    trove_id: i64,
) -> Result<(Vec<ComponentSnapshot>, BTreeMap<i64, String>)> {
    let mut names = BTreeMap::new();
    let mut snapshots = Vec::new();
    for component in Component::find_by_trove(conn, trove_id)? {
        let component_id = component
            .id
            .ok_or_else(|| anyhow!("installed component '{}' has no ID", component.name))?;
        let installed_at = component.installed_at.clone().ok_or_else(|| {
            anyhow!(
                "installed component '{}:{}' has no installed_at",
                trove_id,
                component.name
            )
        })?;
        names.insert(component_id, component.name.clone());
        let mut dependencies = ComponentDependency::find_by_component(conn, component_id)?
            .into_iter()
            .map(|dependency| ComponentDependencySnapshot {
                depends_on_component: dependency.depends_on_component,
                depends_on_package: dependency.depends_on_package,
                dependency_type: dependency.dependency_type,
                version_constraint: dependency.version_constraint,
            })
            .collect::<Vec<_>>();
        dependencies.sort_by(|left, right| {
            (
                left.depends_on_package.as_deref(),
                left.depends_on_component.as_str(),
            )
                .cmp(&(
                    right.depends_on_package.as_deref(),
                    right.depends_on_component.as_str(),
                ))
        });
        let mut provides = ComponentProvide::find_by_component(conn, component_id)?
            .into_iter()
            .map(|provide| ComponentProvideSnapshot {
                capability: provide.capability,
                version: provide.version,
            })
            .collect::<Vec<_>>();
        provides.sort_by(|left, right| left.capability.cmp(&right.capability));
        snapshots.push(ComponentSnapshot {
            name: component.name,
            description: component.description,
            installed_at,
            is_installed: component.is_installed,
            dependencies,
            provides,
        });
    }
    Ok((snapshots, names))
}

fn capture_files(
    files: &[FileEntry],
    component_names: &BTreeMap<i64, String>,
    directory_claims: &[DirectoryClaimSnapshot],
) -> Result<Vec<FileSnapshot>> {
    let target_paths = directory_claims
        .iter()
        .filter_map(|claim| claim.materialization_target_path.as_deref())
        .collect::<std::collections::BTreeSet<_>>();
    let mut snapshots = files
        .iter()
        .filter(|file| {
            !matches!(file.node.source.kind, PayloadNodeKind::Directory)
                || target_paths.contains(file.path.as_str())
        })
        .map(|file| {
            let installed_at = file
                .installed_at
                .clone()
                .ok_or_else(|| anyhow!("installed file '{}' has no installed_at", file.path))?;
            let component = file
                .component_id
                .map(|component_id| {
                    component_names.get(&component_id).cloned().ok_or_else(|| {
                        anyhow!(
                            "installed file '{}' references unknown component ID {}",
                            file.path,
                            component_id
                        )
                    })
                })
                .transpose()?;
            Ok(FileSnapshot {
                path: file.path.clone(),
                node: file.node.clone(),
                content: file.content.clone(),
                installed_at,
                component,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    snapshots.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(snapshots)
}

fn capture_directory_claims(
    conn: &Connection,
    trove_id: i64,
    files: &[FileEntry],
    component_names: &BTreeMap<i64, String>,
) -> Result<Vec<DirectoryClaimSnapshot>> {
    let anchors = files
        .iter()
        .filter(|file| {
            matches!(
                file.node.source.kind,
                PayloadNodeKind::Directory | PayloadNodeKind::Symlink { .. }
            )
        })
        .map(|file| (file.path.as_str(), file))
        .collect::<BTreeMap<_, _>>();
    DirectoryClaim::find_by_trove(conn, trove_id)?
        .into_iter()
        .map(|claim| {
            let claimed_at = claim.claimed_at.ok_or_else(|| {
                anyhow!(
                    "installed directory claim '{}' has no claimed_at authority",
                    claim.path
                )
            })?;
            let component = claim
                .component_id
                .map(|component_id| {
                    component_names.get(&component_id).cloned().ok_or_else(|| {
                        anyhow!(
                            "installed directory claim '{}' references unknown component ID {}",
                            claim.path,
                            component_id
                        )
                    })
                })
                .transpose()?;
            let anchor = anchors
                .get(claim.path.as_str())
                .map(|file| -> Result<DirectoryAnchorSnapshot> {
                    let installed_at = file.installed_at.clone().ok_or_else(|| {
                        anyhow!(
                            "installed directory anchor '{}' has no installed_at authority",
                            file.path
                        )
                    })?;
                    let component = file
                        .component_id
                        .map(|component_id| {
                            component_names.get(&component_id).cloned().ok_or_else(|| {
                                anyhow!(
                                    "installed directory anchor '{}' references unknown component ID {}",
                                    file.path,
                                    component_id
                                )
                            })
                        })
                        .transpose()?;
                    Ok(DirectoryAnchorSnapshot {
                        materialized_node: file.node.clone(),
                        installed_at,
                        component,
                    })
                })
                .transpose()?;
            Ok(DirectoryClaimSnapshot {
                path: claim.path,
                node: claim.node,
                component,
                anchor_policy: claim.anchor_policy,
                materialization_target_path: claim.materialization_target_path,
                claimed_at,
                anchor,
            })
        })
        .collect()
}

fn capture_provides(conn: &Connection, trove_id: i64) -> Result<Vec<ProvideSnapshot>> {
    let mut provides = ProvideEntry::find_by_trove(conn, trove_id)?
        .into_iter()
        .map(|provide| ProvideSnapshot {
            capability: provide.capability,
            version: provide.version,
            version_relation: provide.version_relation,
            kind: provide.kind,
            version_scheme: provide.version_scheme,
            architecture_qualifier: provide.architecture_qualifier,
        })
        .collect::<Vec<_>>();
    provides.sort_by(|left, right| {
        (
            &left.capability,
            format!("{:?}", left.kind),
            &left.version,
            left.version_relation,
            left.version_scheme.as_str(),
            format!("{:?}", left.architecture_qualifier),
        )
            .cmp(&(
                &right.capability,
                format!("{:?}", right.kind),
                &right.version,
                right.version_relation,
                right.version_scheme.as_str(),
                format!("{:?}", right.architecture_qualifier),
            ))
    });
    Ok(provides)
}

fn capture_config_files(
    conn: &Connection,
    trove_id: i64,
    files: &[FileEntry],
) -> Result<Vec<ConfigFileSnapshot>> {
    let trove_identity: (String, String, Option<String>) = conn.query_row(
        "SELECT name, version, architecture FROM troves WHERE id = ?1",
        [trove_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let file_ids = files
        .iter()
        .map(|file| {
            file.id
                .map(|id| (id, file.path.as_str()))
                .ok_or_else(|| anyhow!("installed file '{}' has no ID", file.path))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    ConfigFile::find_by_trove(conn, trove_id)?
        .into_iter()
        .map(|config| {
            let config_id = config
                .id
                .ok_or_else(|| anyhow!("installed config '{}' has no ID", config.path))?;
            if let Some(file_id) = config.file_id {
                match file_ids.get(&file_id) {
                    Some(path) if *path == config.path => {}
                    _ => bail!(
                        "installed config '{}' references file ID {} outside its exact owning path",
                        config.path,
                        file_id
                    ),
                }
            }
            let stable_identity: (String, String, Option<String>) = conn.query_row(
                "SELECT package_name, package_version, package_architecture
                 FROM config_files WHERE id = ?1",
                [config_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            if stable_identity != trove_identity {
                bail!(
                    "installed config '{}' stable package identity differs from its exact owning trove",
                    config.path
                );
            }
            let mut backups = ConfigBackup::find_by_config_file(conn, config_id)?
                .into_iter()
                .map(|backup| {
                    Ok(ConfigBackupSnapshot {
                        backup_hash: backup.backup_hash,
                        reason: backup.reason,
                        changeset_id: backup.changeset_id,
                        created_at: backup.created_at.ok_or_else(|| {
                            anyhow!("installed config backup has no created_at authority")
                        })?,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            backups.sort_by(|left, right| {
                (
                    &left.created_at,
                    &left.backup_hash,
                    &left.reason,
                    left.changeset_id,
                )
                    .cmp(&(
                        &right.created_at,
                        &right.backup_hash,
                        &right.reason,
                        right.changeset_id,
                    ))
            });
            Ok(ConfigFileSnapshot {
                path: config.path,
                file_backed: config.file_id.is_some(),
                original_hash: config.original_hash,
                original_md5: config.original_md5,
                current_hash: config.current_hash,
                noreplace: config.noreplace,
                ghost: config.ghost,
                remove_on_upgrade: config.remove_on_upgrade,
                status: config.status,
                modified_at: config.modified_at,
                source: config.source,
                backups,
            })
        })
        .collect()
}

fn capture_file_capabilities(
    conn: &Connection,
    trove_id: i64,
) -> Result<Vec<InstalledFileCapabilitySnapshot>> {
    InstalledFileCapability::find_by_trove(conn, trove_id)?
        .into_iter()
        .map(|capability| {
            Ok(InstalledFileCapabilitySnapshot {
                capability: FileCapability {
                    path: capability.path,
                    capabilities: capability.capabilities,
                    permitted: capability.permitted,
                    effective: capability.effective,
                    inheritable: capability.inheritable,
                },
                created_at: capability.created_at.ok_or_else(|| {
                    anyhow!("installed file capability has no created_at authority")
                })?,
            })
        })
        .collect()
}

fn capture_package_capabilities(
    conn: &Connection,
    trove_id: i64,
) -> Result<Option<PackageCapabilitySnapshot>> {
    let Some(declaration) =
        load_capabilities(conn, trove_id).map_err(|error| anyhow!(error.to_string()))?
    else {
        return Ok(None);
    };
    let (declaration_version, declared_at) = conn.query_row(
        "SELECT declaration_version, declared_at FROM capabilities WHERE trove_id = ?1",
        [trove_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    Ok(Some(PackageCapabilitySnapshot {
        declaration,
        declaration_version,
        declared_at,
    }))
}

fn capture_native_lifecycle(
    conn: &Connection,
    trove_id: i64,
) -> Result<Option<NativeLifecycleSnapshot>> {
    InstalledNativeLifecycleBundle::find_by_trove(conn, trove_id)?
        .map(|installed| {
            installed.bundle()?;
            Ok(NativeLifecycleSnapshot {
                source_format: installed.source_format,
                source_family: installed.source_family,
                source_profile: installed.source_profile,
                source_release: installed.source_release,
                source_arch: installed.source_arch,
                source_package: installed.source_package,
                source_version: installed.source_version,
                scriptlet_fidelity: installed.scriptlet_fidelity,
                evidence_digest: installed.evidence_digest,
                bundle_toml: installed.bundle_toml,
                lifecycle_state: installed.lifecycle_state.as_str().to_string(),
                pending_triggers: installed.pending_triggers,
                awaited_packages: installed.awaited_packages,
                installed_at: installed.installed_at.ok_or_else(|| {
                    anyhow!("installed native lifecycle bundle has no installed_at")
                })?,
            })
        })
        .transpose()
}

fn capture_derived_references(
    conn: &Connection,
    trove_id: i64,
) -> Result<Vec<DerivedPackageReferenceSnapshot>> {
    let mut statement = conn.prepare(
        "SELECT id, parent_trove_id = ?1, built_trove_id = ?1
         FROM derived_packages
         WHERE parent_trove_id = ?1 OR built_trove_id = ?1
         ORDER BY id",
    )?;
    Ok(statement
        .query_map([trove_id], |row| {
            Ok(DerivedPackageReferenceSnapshot {
                derived_package_id: row.get(0)?,
                parent: row.get::<_, i64>(1)? != 0,
                built: row.get::<_, i64>(2)? != 0,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

fn capture_installed_conversions(
    conn: &Connection,
    trove_id: i64,
) -> Result<Vec<InstalledConversionSnapshot>> {
    let mut statement = conn.prepare(
        "SELECT original_format, original_checksum, conversion_version, converted_at,
                enhancement_version, extracted_provenance_json, enhancement_status,
                enhancement_error, enhancement_attempted_at, enhancement_priority,
                scriptlet_fidelity, evidence_digest, scriptlet_summary_json
         FROM converted_packages
         WHERE artifact_kind = 'installed' AND trove_id = ?1
         ORDER BY original_checksum",
    )?;
    let snapshots = statement
        .query_map([trove_id], |row| {
            Ok(InstalledConversionSnapshot {
                original_format: row.get(0)?,
                original_checksum: row.get(1)?,
                conversion_version: row.get(2)?,
                converted_at: row.get(3)?,
                enhancement_version: row.get(4)?,
                extracted_provenance_json: row.get(5)?,
                enhancement_status: row.get(6)?,
                enhancement_error: row.get(7)?,
                enhancement_attempted_at: row.get(8)?,
                enhancement_priority: row.get(9)?,
                scriptlet_fidelity: row.get(10)?,
                evidence_digest: row.get(11)?,
                scriptlet_summary_json: row.get(12)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for snapshot in &snapshots {
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
        converted.scriptlet_summary().with_context(|| {
            format!(
                "installed conversion '{}' has invalid lifecycle evidence",
                snapshot.original_checksum
            )
        })?;
    }
    Ok(snapshots)
}

fn capture_provenance(conn: &Connection, trove_id: i64) -> Result<Option<ProvenanceSnapshot>> {
    conn.query_row(
        "SELECT source_url, source_branch, source_commit, build_host, build_time, builder,
                upstream_url, upstream_hash, git_repo, git_tag, fetch_timestamp,
                patches_json, recipe_hash, build_deps_json, host_arch, host_kernel,
                host_distro, build_start, build_end, build_log_hash, isolation_level,
                reproducibility_json, signatures_json, rekor_log_index, sbom_spdx_hash,
                sbom_cyclonedx_hash, merkle_root, component_hashes_json,
                chunk_manifest_json, total_size, file_count, dna_hash
         FROM provenance WHERE trove_id = ?1",
        [trove_id],
        |row| {
            Ok(ProvenanceSnapshot {
                source_url: row.get(0)?,
                source_branch: row.get(1)?,
                source_commit: row.get(2)?,
                build_host: row.get(3)?,
                build_time: row.get(4)?,
                builder: row.get(5)?,
                upstream_url: row.get(6)?,
                upstream_hash: row.get(7)?,
                git_repo: row.get(8)?,
                git_tag: row.get(9)?,
                fetch_timestamp: row.get(10)?,
                patches_json: row.get(11)?,
                recipe_hash: row.get(12)?,
                build_deps_json: row.get(13)?,
                host_arch: row.get(14)?,
                host_kernel: row.get(15)?,
                host_distro: row.get(16)?,
                build_start: row.get(17)?,
                build_end: row.get(18)?,
                build_log_hash: row.get(19)?,
                isolation_level: row.get(20)?,
                reproducibility_json: row.get(21)?,
                signatures_json: row.get(22)?,
                rekor_log_index: row.get(23)?,
                sbom_spdx_hash: row.get(24)?,
                sbom_cyclonedx_hash: row.get(25)?,
                merkle_root: row.get(26)?,
                component_hashes_json: row.get(27)?,
                chunk_manifest_json: row.get(28)?,
                total_size: row.get(29)?,
                file_count: row.get(30)?,
                dna_hash: row.get(31)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}
