// apps/conary/src/commands/install/batch/preparation.rs

use super::*;

/// Result of batch planning
pub(super) struct BatchPlan {
    pub(super) total_files: usize,
    pub(super) conflicts: Vec<BatchConflict>,
}

/// Conflict detected during batch planning
#[derive(Debug)]
pub(super) enum BatchConflict {
    /// Two packages in the batch both try to install the same file
    CrossPackageConflict {
        path: String,
        package1: String,
        package2: String,
        reason: String,
    },
}

impl fmt::Display for BatchConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BatchConflict::CrossPackageConflict {
                path,
                package1,
                package2,
                reason,
            } => write!(
                f,
                "{}: conflict between {} and {}: {}",
                path, package1, package2, reason
            ),
        }
    }
}

/// Prepare a package for batch installation
///
/// This extracts the package, parses metadata, and checks for upgrades.
/// A returned `PreparedPackage` can be passed to
/// `BatchInstaller::install_batch()`. `None` means the exact package identity
/// is already installed.
pub fn prepare_package_for_batch(
    package_path: &Path,
    db_path: &str,
    install_reason: InstallReason,
    selection_reason: &str,
    allow_downgrade: bool,
    source_profile_id: Option<&str>,
) -> Result<Option<PreparedPackage>> {
    // Detect format
    let path_str = package_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Invalid package path (non-UTF8)"))?;

    let format = detect_package_format(path_str)
        .with_context(|| format!("Failed to detect package format for '{}'", path_str))?;

    // Parse package
    let pkg = parse_package(package_path, format)?;
    let semantics = InstallSemantics::native_package(format);

    // Open database
    let conn = open_db(db_path)?;

    // Check for existing installation
    let (is_upgrade, old_trove) = match check_upgrade_status(
        &conn,
        pkg.as_ref(),
        &semantics,
        allow_downgrade,
        InstallIntent::PackageChange,
    )? {
        UpgradeCheck::FreshInstall => (false, None),
        UpgradeCheck::AlreadyInstalled(_) => return Ok(None),
        UpgradeCheck::Upgrade(trove)
        | UpgradeCheck::Downgrade(trove)
        | UpgradeCheck::Replatform(trove) => (true, Some(trove)),
    };

    // Extract files
    info!("Extracting files from {}...", pkg.name());
    let extracted_files = pkg
        .package_payload()
        .map(conary_core::packages::payload::PackagePayload::into_files)
        .with_context(|| format!("Failed to extract files from package '{}'", pkg.name()))?;
    let repository_enrollments = if format == super::super::PackageFormatType::Rpm {
        let architecture = pkg.architecture().ok_or_else(|| {
            anyhow::anyhow!("RPM repository enrollment requires package architecture authority")
        })?;
        conary_core::repository::enrollment::derive::derive_rpm_repository_enrollments(
            &extracted_files,
            architecture,
            source_profile_id,
        )?
    } else {
        Vec::new()
    };

    // Native package formats do not carry Conary component authority. Keep
    // their complete payload losslessly in one explicit runtime component.
    let file_paths: Vec<String> = extracted_files.iter().map(|f| f.path.clone()).collect();
    let classified_files = HashMap::from([(ComponentType::Runtime, file_paths)]);
    let installed_components = vec![ComponentType::Runtime];

    let native_lifecycle_state =
        NativeLifecycleInstallState::from_native_package(pkg.as_ref(), format, source_profile_id)?;

    // A native header declares only its own `Provides`; the payload it ships is
    // the only authority for the file providers that satisfy path dependencies.
    let mut provides = pkg.resolution_capabilities()?;
    super::super::transaction::extend_materialized_payload_provides(
        &mut provides,
        semantics,
        &extracted_files,
    )?;

    Ok(Some(PreparedPackage {
        name: pkg.name().to_string(),
        version: pkg.version().to_string(),
        semantics,
        package_release: pkg.package_release().map(str::to_string),
        debian_multi_arch: pkg.debian_multi_arch(),
        architecture: pkg.architecture().map(|s| s.to_string()),
        description: pkg.description().map(|s| s.to_string()),
        extracted_files,
        repository_enrollments,
        provides,
        requirements: pkg.requirements().to_vec(),
        relations: pkg.relations().to_vec(),
        relation_removals: Vec::new(),
        relation_deconfigurations: Vec::new(),
        config_declarations: pkg.config_declarations()?,
        install_reason,
        selection_reason: selection_reason.to_string(),
        is_upgrade,
        old_trove,
        installed_components,
        classified_files,
        installed_component_names: None,
        component_names_by_path: None,
        repository_provenance: None,
        requested_source_identity: None,
        native_lifecycle_state,
        ccs: None,
    }))
}

impl BatchInstaller<'_> {
    /// Plan batch payload ownership and reject two selected packages claiming
    /// the same path.
    pub(super) fn plan_batch(
        &self,
        packages: &[PreparedPackage],
        _conn: &Connection,
    ) -> Result<BatchPlan> {
        let mut all_paths = HashMap::<
            String,
            (
                String,
                conary_core::payload::PayloadSharingPolicy,
                conary_core::payload::PayloadNode,
                Option<conary_core::payload::PayloadContentAuthority>,
            ),
        >::new();
        let mut conflicts = Vec::new();
        let mut total_files = 0;

        for pkg in packages {
            for file in &pkg.extracted_files {
                let incoming_policy = pkg.semantics.payload_sharing_policy();
                let incoming_is_directory = matches!(
                    file.node.kind,
                    conary_core::payload::PayloadNodeKind::Directory
                );
                if let Some((other_package, other_policy, other_node, other_content)) =
                    all_paths.get(&file.path)
                {
                    let other_is_directory = matches!(
                        other_node.kind,
                        conary_core::payload::PayloadNodeKind::Directory
                    );
                    let comparison = if incoming_is_directory && other_is_directory {
                        Ok(())
                    } else {
                        incoming_policy.compare(
                            *other_policy,
                            &file.node,
                            file.content_authority.as_ref(),
                            other_node,
                            other_content.as_ref(),
                        )
                    };
                    if let Err(reason) = comparison {
                        conflicts.push(BatchConflict::CrossPackageConflict {
                            path: file.path.clone(),
                            package1: other_package.clone(),
                            package2: pkg.name.clone(),
                            reason: reason.to_string(),
                        });
                    }
                } else {
                    all_paths.insert(
                        file.path.clone(),
                        (
                            pkg.name.clone(),
                            incoming_policy,
                            file.node.clone(),
                            file.content_authority.clone(),
                        ),
                    );
                }
                total_files += 1;
            }
        }

        Ok(BatchPlan {
            total_files,
            conflicts,
        })
    }
}
