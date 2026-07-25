// src/commands/install/restore.rs
//! Shared install preparation/execution helpers for state restore.

use super::acquire::install_provenance_from_resolved;
use super::ccs_removal_hooks::CcsRemovalHookPlan;
use super::native_events::PreparedNativeTransaction;
use super::prepare::{UpgradeCheck, check_upgrade_status, parse_package};
use super::resolve::{
    PolicyOptions, ResolutionOutcome, ResolvedSourceType, resolve_package_path_with_policy,
};
use super::{
    ExtractionResult, InstallOptions, InstallPhase, InstallProgress, InstallSemantics,
    NativeLifecycleInstallState, TransactionContext, build_resolution_policy,
    extract_and_classify_files, resolve_canonical_name, verify_ccs_package_authority,
};
use anyhow::{Context, Result};
use conary_core::ccs::CcsPackage;
use conary_core::db::models::{ProvideEntry, StateMember, Trove, TroveType};
use conary_core::packages::PackageFormat;
use conary_core::repository::dependency_model::RepositoryRequirementKind;
use conary_core::resolver::identity::{PackageIdentity, ProvidedCapability};
use rusqlite::Connection;
use tempfile::TempDir;

pub(crate) struct PreparedInstall {
    pkg: Box<dyn PackageFormat>,
    extraction: ExtractionResult,
    selection_reason: Option<String>,
    old_trove_to_upgrade: Option<Trove>,
    semantics: InstallSemantics,
    _temp_dir: Option<TempDir>,
    native_lifecycle_state: NativeLifecycleInstallState,
    native_transaction: PreparedNativeTransaction,
    ccs_removal_hook_plan: CcsRemovalHookPlan,
}

#[derive(Debug, Default)]
pub(crate) struct TargetStateView {
    packages: Vec<PackageIdentity>,
}

impl TargetStateView {
    fn add_installed_trove(&mut self, conn: &Connection, trove: &Trove) -> Result<()> {
        let version_scheme = trove.version_scheme;
        let mut provided_capabilities = Vec::new();
        if let Some(trove_id) = trove.id {
            for provide in ProvideEntry::find_by_trove(conn, trove_id)? {
                provided_capabilities.push(ProvidedCapability {
                    name: provide.capability.clone(),
                    version: provide.version.clone(),
                    version_scheme,
                });
                let typed = provide.to_typed_string();
                if typed != provide.capability {
                    provided_capabilities.push(ProvidedCapability {
                        name: typed,
                        version: provide.version,
                        version_scheme,
                    });
                }
            }
        }
        self.packages.push(package_identity(
            trove.name.clone(),
            trove.version.clone(),
            trove.architecture.clone(),
            version_scheme,
            provided_capabilities,
            trove.id,
        ));
        Ok(())
    }

    fn add_prepared_install(&mut self, prepared: &PreparedInstall) {
        let version_scheme = prepared.pkg.version_scheme();
        let mut provided_capabilities = prepared
            .pkg
            .provides()
            .iter()
            .map(|provide| ProvidedCapability {
                name: provide.name.clone(),
                version: provide.version.clone(),
                version_scheme,
            })
            .collect::<Vec<_>>();
        for provide in &prepared.extraction.language_provides {
            provided_capabilities.push(ProvidedCapability {
                name: provide.to_dep_string(),
                version: None,
                version_scheme,
            });
            provided_capabilities.push(ProvidedCapability {
                name: provide.name.clone(),
                version: None,
                version_scheme,
            });
        }
        self.packages.push(package_identity(
            prepared.pkg.name().to_string(),
            prepared.pkg.version().to_string(),
            prepared.pkg.architecture().map(str::to_string),
            version_scheme,
            provided_capabilities,
            None,
        ));
    }
}

fn package_identity(
    name: String,
    version: String,
    architecture: Option<String>,
    version_scheme: conary_core::repository::versioning::VersionScheme,
    provided_capabilities: Vec<ProvidedCapability>,
    installed_trove_id: Option<i64>,
) -> PackageIdentity {
    PackageIdentity {
        repo_package_id: None,
        name,
        version,
        package_release: None,
        architecture,
        version_scheme,
        repository_id: None,
        repository_name: String::new(),
        repository_distro: None,
        repository_priority: 0,
        canonical_id: None,
        canonical_name: None,
        installed_trove_id,
        installed_pinned: false,
        provided_capabilities,
    }
}

pub(crate) fn build_target_state_view(
    conn: &Connection,
    members: &[StateMember],
) -> Result<TargetStateView> {
    let mut target_state = TargetStateView::default();

    for member in members {
        if let Some(trove) = Trove::find_by_name(conn, &member.trove_name)?
            .into_iter()
            .find(|trove| {
                trove.version == member.trove_version
                    && (member.architecture == trove.architecture
                        || member.architecture.is_none()
                        || trove.architecture.is_none())
                    && trove.trove_type == TroveType::Package
            })
        {
            target_state.add_installed_trove(conn, &trove)?;
        }
    }

    Ok(target_state)
}

pub(crate) fn add_prepared_install_to_target_state(
    target_state: &mut TargetStateView,
    prepared: &PreparedInstall,
) {
    target_state.add_prepared_install(prepared);
}

pub(crate) fn validate_prepared_install_dependencies(
    prepared: &PreparedInstall,
    target_state: &TargetStateView,
) -> Result<()> {
    let mut unsatisfied = Vec::new();
    for requirement in prepared.pkg.requirements().iter().filter(|requirement| {
        matches!(
            requirement.kind,
            RepositoryRequirementKind::Depends | RepositoryRequirementKind::PreDepends
        )
    }) {
        conary_core::repository::requirement::validate_requirement_group(
            requirement,
            prepared.pkg.version_scheme(),
        )
        .map_err(anyhow::Error::msg)?;
        if !conary_core::resolver::requirement_expression_satisfied(
            &requirement.expression,
            prepared.pkg.version_scheme(),
            &target_state.packages,
        )? {
            unsatisfied.push(requirement);
        }
    }

    if unsatisfied.is_empty() {
        return Ok(());
    }

    let summary = unsatisfied
        .iter()
        .map(|requirement| {
            requirement.native_text.clone().unwrap_or_else(|| {
                serde_json::to_string(&requirement.expression)
                    .unwrap_or_else(|_| "<invalid typed requirement>".to_string())
            })
        })
        .collect::<Vec<_>>()
        .join(", ");

    anyhow::bail!(
        "Restore target '{}' has unsatisfied dependencies in the destination state: {}",
        prepared.pkg.name(),
        summary
    );
}

pub(crate) async fn prepare_install_for_restore(
    conn: &Connection,
    package: &str,
    opts: InstallOptions<'_>,
) -> Result<PreparedInstall> {
    let InstallOptions {
        db_path,
        root: _,
        version,
        repo,
        architecture,
        selection_reason,
        allow_downgrade,
        from_distro,
        sandbox_mode: _,
        ..
    } = opts;

    let effective_source_policy = conary_core::repository::load_effective_policy(
        conn,
        conary_core::repository::resolution_policy::RequestScope::Any,
    )?;
    let policy = build_resolution_policy(
        effective_source_policy.resolution,
        from_distro.as_deref(),
        repo.as_deref(),
    )?;
    let primary_flavor = effective_source_policy.primary_flavor;
    let resolved_name = resolve_canonical_name(conn, package, from_distro.as_deref(), &policy)?;
    let package_name = resolved_name.unwrap_or_else(|| package.to_string());

    let progress = InstallProgress::single("Restoring");
    progress.set_phase(&package_name, InstallPhase::Downloading);
    let policy_opts = PolicyOptions {
        policy: Some(policy),
        is_root: true,
        primary_flavor,
    };

    let resolved = match resolve_package_path_with_policy(
        &package_name,
        db_path,
        version.as_deref(),
        repo.as_deref(),
        architecture.as_deref(),
        &progress,
        &policy_opts,
    )
    .await?
    {
        ResolutionOutcome::AlreadyInstalled { name, version } => {
            anyhow::bail!(
                "Restore preflight expected '{}' to be absent/pending, but resolver reported {} {} already installed",
                package,
                name,
                version
            );
        }
        ResolutionOutcome::Resolved(pkg) => pkg,
    };

    let path_str = resolved
        .path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Invalid package path (non-UTF8)"))?;

    let has_ccs_contract =
        conary_core::ccs::archive_reader::has_current_ccs_archive_contract(&resolved.path)
            .context("Failed to inspect package archive contract")?;
    let (pkg, semantics, native_lifecycle_bundle, ccs_remove_hook) = if resolved.source_type
        == ResolvedSourceType::Remi
        || has_ccs_contract
    {
        let repository_provenance = install_provenance_from_resolved(&resolved);
        let verified =
            verify_ccs_package_authority(db_path, &resolved.path, repository_provenance.as_ref())?;
        let ccs_pkg = CcsPackage::from_verified_archive(path_str, &verified)
            .context("Failed to construct verified CCS package")?;
        let native_lifecycle_bundle = ccs_pkg.manifest().native_lifecycle.clone();
        let ccs_remove_hook = ccs_pkg.manifest().hooks.pre_remove.clone();
        let version_scheme = ccs_pkg.manifest().package.version_scheme;
        (
            Box::new(ccs_pkg) as Box<dyn PackageFormat>,
            InstallSemantics::ccs(version_scheme),
            native_lifecycle_bundle,
            ccs_remove_hook,
        )
    } else {
        let format = super::detect_package_format(path_str)
            .with_context(|| format!("Failed to detect package format for '{}'", path_str))?;
        (
            parse_package(&resolved.path, format)?,
            InstallSemantics::native_package(format),
            None,
            None,
        )
    };

    progress.set_phase(package, InstallPhase::Parsing);
    let mut extraction = extract_and_classify_files(
        pkg.as_ref(),
        &super::ComponentSelection::Defaults,
        &progress,
    )?;
    extraction.ccs_remove_hook = ccs_remove_hook;

    let old_trove_to_upgrade = match check_upgrade_status(
        conn,
        pkg.as_ref(),
        &semantics,
        allow_downgrade,
    )? {
        UpgradeCheck::FreshInstall => None,
        UpgradeCheck::AlreadyInstalled(trove) => {
            anyhow::bail!(
                "Restore preflight expected '{}' to be absent/pending, but resolver reported {} {} already installed",
                package,
                trove.name,
                trove.version
            )
        }
        UpgradeCheck::Upgrade(trove) | UpgradeCheck::Downgrade(trove) => Some(*trove),
    };
    let native_lifecycle_state =
        NativeLifecycleInstallState::from_bundle(native_lifecycle_bundle.as_ref())?;
    let native_transaction = PreparedNativeTransaction::prepare_install(
        conn,
        pkg.name(),
        pkg.version(),
        pkg.architecture(),
        semantics.version_scheme,
        pkg.provides(),
        native_lifecycle_bundle.as_ref(),
        old_trove_to_upgrade.as_ref(),
        &[],
        &[],
        &extraction,
    )?;
    let ccs_removal_hook_plan = CcsRemovalHookPlan::prepare(
        conn,
        old_trove_to_upgrade.as_ref().into_iter(),
        std::iter::empty(),
    )?;

    Ok(PreparedInstall {
        pkg,
        extraction,
        selection_reason: selection_reason.map(str::to_string),
        old_trove_to_upgrade,
        semantics,
        _temp_dir: resolved._temp_dir,
        native_lifecycle_state,
        native_transaction,
        ccs_removal_hook_plan,
    })
}

pub(crate) fn execute_prepared_install_graph(
    conn: &mut Connection,
    db_path: &str,
    root: &str,
    prepared: PreparedInstall,
) -> Result<()> {
    let progress = InstallProgress::single("Restoring");
    let native_execution_mode = super::build_execution_mode(
        prepared
            .old_trove_to_upgrade
            .as_ref()
            .map(|trove| trove.version.as_str()),
    );
    let mut selected_root = crate::commands::generation::selected_root::SelectedRootSession::begin(
        conn,
        db_path,
        format!("Restore {}-{}", prepared.pkg.name(), prepared.pkg.version()),
    )?;
    let transaction_root = selected_root.selected_root().to_string_lossy().into_owned();
    prepared.native_transaction.preflight(
        std::path::Path::new(&transaction_root),
        &native_execution_mode,
    )?;
    let preflighted_ccs_removal_hooks = prepared.ccs_removal_hook_plan.preflight(
        std::path::Path::new(&transaction_root),
        conary_core::scriptlet::SandboxMode::Always,
    )?;
    preflighted_ccs_removal_hooks.execute()?;

    let tx_ctx = TransactionContext {
        db_path,
        root: &transaction_root,
        semantics: prepared.semantics,
        selection_reason: prepared.selection_reason.as_deref(),
        old_trove_to_upgrade: prepared.old_trove_to_upgrade.as_ref(),
        ccs_manifest_provides: None,
        ccs_capabilities: None,
        ccs_file_capabilities: None,
        defer_generation: false,
        repository_provenance: None,
        native_lifecycle_bundle: prepared.native_lifecycle_state.bundle_to_persist.as_ref(),
        relation_removals: &[],
        relation_deconfigurations: &[],
        retain_replaced_payload_until_lifecycle: prepared
            .native_transaction
            .requires_upgrade_payload_boundary(),
    };
    let tx_result = super::execute_install_transaction_in_selected_root(
        conn,
        prepared.pkg.as_ref(),
        &prepared.extraction,
        &tx_ctx,
        &progress,
        conary_core::transaction::TransactionConfig::from_paths(
            selected_root.selected_root().to_path_buf(),
            std::path::PathBuf::from(db_path),
        ),
        &mut selected_root,
        &prepared.native_transaction,
        &native_execution_mode,
    )?;
    super::finalize_install_without_snapshot(
        conn,
        prepared.pkg.as_ref(),
        &prepared.extraction,
        root,
        &tx_result,
        super::FinalizeInstallOutput::new(&progress, false),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::install::{InstallSemantics, PackageFormatType};
    use conary_core::components::ComponentType;
    use conary_core::db::models::{Trove, TroveType};
    use conary_core::packages::traits::{ExtractedFile, PackageFile, PackageFormat};

    struct FakePackage {
        requirements: Vec<conary_core::repository::dependency_model::RepositoryRequirementGroup>,
    }

    impl PackageFormat for FakePackage {
        fn parse(_path: &str) -> conary_core::Result<Self> {
            unreachable!("test constructs package directly")
        }

        fn name(&self) -> &str {
            "restore-fixture"
        }

        fn version(&self) -> &str {
            "1.0.0"
        }

        fn version_scheme(&self) -> conary_core::repository::versioning::VersionScheme {
            conary_core::repository::versioning::VersionScheme::Rpm
        }

        fn architecture(&self) -> Option<&str> {
            Some("x86_64")
        }

        fn description(&self) -> Option<&str> {
            None
        }

        fn files(&self) -> &[PackageFile] {
            &[]
        }

        fn requirements(
            &self,
        ) -> &[conary_core::repository::dependency_model::RepositoryRequirementGroup] {
            &self.requirements
        }

        fn extract_file_contents(&self) -> conary_core::Result<Vec<ExtractedFile>> {
            Ok(vec![])
        }

        fn to_trove(&self) -> Trove {
            Trove::new(
                "restore-fixture".to_string(),
                "1.0.0".to_string(),
                TroveType::Package,
                conary_core::repository::versioning::VersionScheme::Conary,
            )
        }
    }

    fn prepared_restore_fixture() -> PreparedInstall {
        prepared_restore_fixture_with_requirements(Vec::new())
    }

    fn prepared_restore_fixture_with_requirements(
        requirements: Vec<conary_core::repository::dependency_model::RepositoryRequirementGroup>,
    ) -> PreparedInstall {
        PreparedInstall {
            pkg: Box::new(FakePackage { requirements }),
            extraction: ExtractionResult {
                extracted_files: Vec::new(),
                classified: std::collections::HashMap::new(),
                component_names_by_path: None,
                installed_component_names: None,
                ccs_remove_hook: None,
                installed_component_types: vec![ComponentType::Runtime],
                skipped_components: Vec::new(),
                language_provides: Vec::new(),
            },
            selection_reason: None,
            old_trove_to_upgrade: None,
            semantics: InstallSemantics::native_package(PackageFormatType::Rpm),
            _temp_dir: None,
            native_lifecycle_state: super::super::NativeLifecycleInstallState::default(),
            native_transaction: PreparedNativeTransaction::default(),
            ccs_removal_hook_plan: CcsRemovalHookPlan::default(),
        }
    }

    #[test]
    fn prepared_restore_carries_native_lifecycle_state() {
        let prepared = prepared_restore_fixture();

        assert!(prepared.native_lifecycle_state.bundle_to_persist.is_none());
    }

    #[test]
    fn restore_dependency_validation_preserves_same_provider_semantics() {
        let requirement = conary_core::repository::requirement::parse_native_requirement(
            RepositoryRequirementKind::Depends,
            conary_core::repository::versioning::VersionScheme::Rpm,
            "(feature-a with feature-b)",
        )
        .unwrap();
        let prepared = prepared_restore_fixture_with_requirements(vec![requirement]);
        let capability = |name: &str| ProvidedCapability {
            name: name.to_string(),
            version: None,
            version_scheme: conary_core::repository::versioning::VersionScheme::Rpm,
        };
        let split = TargetStateView {
            packages: vec![
                package_identity(
                    "provider-a".to_string(),
                    "1".to_string(),
                    None,
                    conary_core::repository::versioning::VersionScheme::Rpm,
                    vec![capability("feature-a")],
                    None,
                ),
                package_identity(
                    "provider-b".to_string(),
                    "1".to_string(),
                    None,
                    conary_core::repository::versioning::VersionScheme::Rpm,
                    vec![capability("feature-b")],
                    None,
                ),
            ],
        };
        assert!(validate_prepared_install_dependencies(&prepared, &split).is_err());

        let combined = TargetStateView {
            packages: vec![package_identity(
                "provider-both".to_string(),
                "1".to_string(),
                None,
                conary_core::repository::versioning::VersionScheme::Rpm,
                vec![capability("feature-a"), capability("feature-b")],
                None,
            )],
        };
        validate_prepared_install_dependencies(&prepared, &combined).unwrap();
    }

    #[test]
    fn restore_pre_install_fails_closed_on_dangling_current_before_lifecycle() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let db_path = temp.path().join("conary.db");
        std::fs::create_dir_all(&root).unwrap();
        std::os::unix::fs::symlink("generations/7", temp.path().join("current")).unwrap();
        conary_core::db::init(&db_path).unwrap();
        let mut conn = conary_core::db::open(&db_path).unwrap();
        let prepared = prepared_restore_fixture();
        let db_path_string = db_path.to_string_lossy().into_owned();
        let root_string = root.to_string_lossy().into_owned();

        let result =
            execute_prepared_install_graph(&mut conn, &db_path_string, &root_string, prepared);
        let error = match result {
            Ok(_) => panic!("restore pre-install should fail on dangling current"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("dangling"), "{error:#}");
    }

    #[test]
    fn restore_pre_install_refuses_no_generation_before_lifecycle() {
        let _mount_skip = crate::commands::composefs_ops::test_mount_skip_clear_guard();
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let db_path = temp.path().join("conary.db");
        std::fs::create_dir_all(&root).unwrap();
        conary_core::db::init(&db_path).unwrap();
        let mut conn = conary_core::db::open(&db_path).unwrap();
        let prepared = prepared_restore_fixture();
        let db_path_string = db_path.to_string_lossy().into_owned();
        let root_string = root.to_string_lossy().into_owned();

        let result =
            execute_prepared_install_graph(&mut conn, &db_path_string, &root_string, prepared);
        let error = match result {
            Ok(_) => panic!("restore pre-install should refuse no-generation install"),
            Err(error) => error,
        };

        assert!(
            error
                .to_string()
                .contains("state restore installs require an active Conary generation"),
            "{error:#}"
        );
    }
}
