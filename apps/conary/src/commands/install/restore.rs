// apps/conary/src/commands/install/restore.rs
//! Shared install preparation/execution helpers for state restore.

mod transaction;

pub(crate) use transaction::execute_state_restore_transaction;

use super::acquire::install_provenance_from_resolved;
use super::ccs_removal_hooks::CcsRemovalHookPlan;
use super::prepare::{UpgradeCheck, check_upgrade_status, parse_package};
use super::resolve::{
    PackageResolutionRequest, PolicyOptions, ResolutionOutcome, ResolvedSourceType,
    resolve_package_path_with_policy,
};
use super::{
    CcsEnvelopeAuthority, ExtractionResult, InstallIntent, InstallOptions, InstallPhase,
    InstallProgress, InstallSemantics, NativeLifecycleInstallState, build_resolution_policy,
    extract_and_classify_files, resolve_canonical_name, verify_ccs_package_authority_into_cas,
};
use anyhow::{Context, Result};
use conary_core::ccs::CcsPackage;
use conary_core::db::models::{ProvideEntry, StateMember, Trove, TroveType};
use conary_core::packages::PackageFormat;
use conary_core::repository::dependency_model::{DebianMultiArch, RepositoryRequirementKind};
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
    ccs_removal_hook_plan: CcsRemovalHookPlan,
    repository_provenance: Option<super::RepositoryInstallProvenance>,
    ccs_contract: Option<PreparedCcsInstallContract>,
}

struct PreparedCcsInstallContract {
    capabilities: Option<conary_core::capability::CapabilityDeclaration>,
    file_capabilities: Vec<conary_core::ccs::manifest::FileCapability>,
    hooks: conary_core::ccs::manifest::Hooks,
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
                    kind: provide.kind,
                    name: provide.capability,
                    version: provide.version,
                    version_relation: provide.version_relation,
                    version_scheme: provide.version_scheme,
                    architecture_qualifier: provide.architecture_qualifier,
                    provenance: provide.provenance,
                });
            }
        }
        self.packages.push(package_identity(TargetPackageIdentity {
            name: trove.name.clone(),
            version: trove.version.clone(),
            package_release: trove.package_release.clone(),
            architecture: trove.architecture.clone(),
            debian_multi_arch: trove.debian_multi_arch,
            version_scheme,
            provided_capabilities,
            installed_trove_id: trove.id,
        }));
        Ok(())
    }

    fn add_prepared_install(&mut self, prepared: &PreparedInstall) -> Result<()> {
        let version_scheme = prepared.pkg.version_scheme();
        let provided_capabilities = prepared
            .pkg
            .resolution_capabilities()?
            .into_iter()
            .map(|provide| ProvidedCapability {
                kind: provide.kind,
                name: provide.name,
                version: provide.version,
                version_relation: provide.version_relation,
                version_scheme: provide.version_scheme,
                architecture_qualifier: provide.architecture_qualifier,
                provenance: provide.provenance,
            })
            .collect::<Vec<_>>();
        self.packages.push(package_identity(TargetPackageIdentity {
            name: prepared.pkg.name().to_string(),
            version: prepared.pkg.version().to_string(),
            package_release: prepared.pkg.package_release().map(str::to_string),
            architecture: prepared.pkg.architecture().map(str::to_string),
            debian_multi_arch: prepared.pkg.debian_multi_arch(),
            version_scheme,
            provided_capabilities,
            installed_trove_id: None,
        }));
        Ok(())
    }
}

struct TargetPackageIdentity {
    name: String,
    version: String,
    package_release: Option<String>,
    architecture: Option<String>,
    debian_multi_arch: Option<DebianMultiArch>,
    version_scheme: conary_core::repository::versioning::VersionScheme,
    provided_capabilities: Vec<ProvidedCapability>,
    installed_trove_id: Option<i64>,
}

fn package_identity(identity: TargetPackageIdentity) -> PackageIdentity {
    let TargetPackageIdentity {
        name,
        version,
        package_release,
        architecture,
        debian_multi_arch,
        version_scheme,
        provided_capabilities,
        installed_trove_id,
    } = identity;
    PackageIdentity {
        repo_package_id: None,
        name,
        version,
        package_release,
        architecture,
        debian_multi_arch,
        version_scheme,
        repository_id: None,
        repository_name: String::new(),
        repository_profile: None,
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
        let matches = Trove::find_by_name(conn, &member.trove_name)?
            .into_iter()
            .filter(|trove| {
                trove.version == member.trove_version
                    && trove.package_release == member.package_release
                    && member.architecture == trove.architecture
                    && trove.trove_type == TroveType::Package
            })
            .collect::<Vec<_>>();

        match matches.as_slice() {
            [] => {}
            [trove] => target_state.add_installed_trove(conn, trove)?,
            _ => {
                anyhow::bail!(
                    "Restore target state contains ambiguous installed identity '{}' version '{}'{}",
                    member.trove_name,
                    member.package_release.as_ref().map_or_else(
                        || member.trove_version.clone(),
                        |release| format!("{}-{release}", member.trove_version),
                    ),
                    member
                        .architecture
                        .as_deref()
                        .map(|architecture| format!(" [{architecture}]"))
                        .unwrap_or_default(),
                );
            }
        }
    }

    Ok(target_state)
}

pub(crate) fn add_prepared_install_to_target_state(
    target_state: &mut TargetStateView,
    prepared: &PreparedInstall,
) -> Result<()> {
    target_state.add_prepared_install(prepared)
}

pub(crate) fn validate_prepared_install_dependencies(
    prepared: &PreparedInstall,
    target_state: &TargetStateView,
) -> Result<()> {
    let native_architecture = conary_core::repository::registry::detect_system_arch();
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
        let depending_architecture = prepared
            .pkg
            .architecture()
            .filter(|architecture| !architecture.is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "prepared package '{}' has no architecture authority",
                    prepared.pkg.name()
                )
            })?;
        if !conary_core::resolver::requirement_expression_satisfied(
            &requirement.expression,
            prepared.pkg.version_scheme(),
            depending_architecture,
            &native_architecture,
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
        package_release,
        repo,
        architecture,
        selection_reason,
        allow_downgrade,
        from_source,
        ..
    } = opts;

    let effective_source_policy = conary_core::repository::load_effective_policy(
        conn,
        conary_core::repository::resolution_policy::RequestScope::Any,
    )?;
    let policy = build_resolution_policy(
        conn,
        effective_source_policy.resolution,
        from_source.as_deref(),
        repo.as_deref(),
    )?;
    let resolved_name = resolve_canonical_name(conn, package, from_source.as_deref(), &policy)?;
    let package_name = resolved_name.unwrap_or_else(|| package.to_string());

    let progress = InstallProgress::single("Restoring");
    progress.set_phase(&package_name, InstallPhase::Downloading);
    let policy_opts = PolicyOptions {
        policy: Some(policy),
        is_root: true,
    };

    let resolved = match resolve_package_path_with_policy(
        PackageResolutionRequest {
            package: &package_name,
            db_path,
            version: version.as_deref(),
            package_release: package_release.as_deref(),
            repository: repo.as_deref(),
            architecture: architecture.as_deref(),
        },
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

    let repository_provenance = install_provenance_from_resolved(&resolved);
    let has_ccs_contract =
        conary_core::ccs::archive_reader::has_current_ccs_archive_contract(&resolved.path)
            .context("Failed to inspect package archive contract")?;
    let (pkg, semantics, native_lifecycle_bundle, ccs_remove_hook, ccs_contract) =
        if resolved.source_type == ResolvedSourceType::Ccs || has_ccs_contract {
            let envelope_authority = if repository_provenance.is_some() {
                CcsEnvelopeAuthority::Repository
            } else if resolved.source_type == ResolvedSourceType::LocalFile {
                CcsEnvelopeAuthority::LocalDev
            } else {
                anyhow::bail!(
                    "Repository-resolved CCS restore package is missing exact repository provenance"
                );
            };
            let runtime_root = conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(db_path);
            let cas = conary_core::filesystem::CasStore::new(runtime_root.objects_dir())?;
            let verified = verify_ccs_package_authority_into_cas(
                db_path,
                &resolved.path,
                &envelope_authority,
                repository_provenance.as_ref(),
                &cas,
            )?;
            let ccs_pkg = CcsPackage::from_verified_archive(path_str, &verified)
                .context("Failed to construct verified CCS package")?;
            super::ccs_transaction::enforce_ccs_scriptlet_capability_gate(&ccs_pkg)?;
            let native_lifecycle_bundle = ccs_pkg.manifest().native_lifecycle.clone();
            let ccs_remove_hook = ccs_pkg.manifest().hooks.pre_remove.clone();
            let semantics =
                super::ccs_transaction::install_semantics_for_ccs_manifest(ccs_pkg.manifest())?;
            let ccs_contract = PreparedCcsInstallContract {
                capabilities: ccs_pkg.manifest().capabilities.clone(),
                file_capabilities: ccs_pkg.manifest().file_capabilities.clone(),
                hooks: ccs_pkg.manifest().hooks.clone(),
            };
            (
                Box::new(ccs_pkg) as Box<dyn PackageFormat>,
                semantics,
                native_lifecycle_bundle,
                ccs_remove_hook,
                Some(ccs_contract),
            )
        } else {
            let format = super::detect_package_format(path_str)
                .with_context(|| format!("Failed to detect package format for '{}'", path_str))?;
            (
                parse_package(&resolved.path, format)?,
                InstallSemantics::native_package(format),
                None,
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
        InstallIntent::PackageChange,
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
        UpgradeCheck::Upgrade(trove)
        | UpgradeCheck::Downgrade(trove)
        | UpgradeCheck::Replatform(trove) => Some(*trove),
    };
    let native_lifecycle_state =
        NativeLifecycleInstallState::from_bundle(native_lifecycle_bundle.as_ref())?;
    let ccs_removal_hook_plan =
        CcsRemovalHookPlan::prepare(conn, old_trove_to_upgrade.as_ref(), std::iter::empty())?;

    Ok(PreparedInstall {
        pkg,
        extraction,
        selection_reason: selection_reason.map(str::to_string),
        old_trove_to_upgrade,
        semantics,
        _temp_dir: resolved._temp_dir,
        native_lifecycle_state,
        ccs_removal_hook_plan,
        repository_provenance,
        ccs_contract,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::install::{InstallSemantics, PackageFormatType};
    use conary_core::components::ComponentType;
    use conary_core::db::models::{StateMember, Trove, TroveType};
    use conary_core::packages::traits::{PackageFile, PackageFormat};

    struct FakePackage {
        requirements: Vec<conary_core::repository::dependency_model::RepositoryRequirementGroup>,
        provides: Vec<conary_core::repository::dependency_model::ProvidedCapability>,
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

        fn resolution_capabilities(
            &self,
        ) -> conary_core::Result<Vec<conary_core::repository::dependency_model::ProvidedCapability>>
        {
            Ok(self.provides.clone())
        }

        fn package_payload(&self) -> conary_core::Result<conary_core::packages::PackagePayload> {
            Ok(conary_core::packages::PackagePayload::default())
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
            pkg: Box::new(FakePackage {
                requirements,
                provides: vec![crate::commands::test_helpers::exact_package_self_provider(
                    "restore-fixture",
                    "1.0.0",
                    conary_core::repository::versioning::VersionScheme::Rpm,
                )],
            }),
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
            ccs_removal_hook_plan: CcsRemovalHookPlan::default(),
            repository_provenance: None,
            ccs_contract: None,
        }
    }

    #[test]
    fn prepared_restore_carries_native_lifecycle_state() {
        let prepared = prepared_restore_fixture();

        assert!(prepared.native_lifecycle_state.bundle_to_persist.is_none());
    }

    #[test]
    fn target_state_view_requires_exact_optional_architecture() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("conary.db");
        conary_core::db::init(&db_path).unwrap();
        let conn = conary_core::db::open(&db_path).unwrap();

        let mut installed = Trove::new(
            "glibc".to_string(),
            "2.40".to_string(),
            TroveType::Package,
            conary_core::repository::versioning::VersionScheme::Rpm,
        );
        installed.package_release = Some("2".to_string());
        installed.architecture = Some("x86_64".to_string());
        installed.insert(&conn).unwrap();

        let architecture_independent = StateMember::new(1, "glibc".to_string(), "2.40".to_string());
        let unmatched =
            build_target_state_view(&conn, std::slice::from_ref(&architecture_independent))
                .unwrap();
        assert!(
            unmatched.packages.is_empty(),
            "absent architecture must not wildcard-match a concrete variant"
        );

        let mut exact = architecture_independent;
        exact.architecture = Some("x86_64".to_string());
        let release_unmatched =
            build_target_state_view(&conn, std::slice::from_ref(&exact)).unwrap();
        assert!(
            release_unmatched.packages.is_empty(),
            "absent release must not wildcard-match an exact signed CCS release"
        );
        exact.package_release = Some("2".to_string());
        let matched = build_target_state_view(&conn, &[exact]).unwrap();
        assert_eq!(matched.packages.len(), 1);
        assert_eq!(matched.packages[0].package_release.as_deref(), Some("2"));
        assert_eq!(matched.packages[0].architecture.as_deref(), Some("x86_64"));
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
            kind: conary_core::repository::dependency_model::RepositoryCapabilityKind::Generic,
            name: name.to_string(),
            version: None,
            version_relation: None,
            version_scheme: conary_core::repository::versioning::VersionScheme::Rpm,
            architecture_qualifier: Default::default(),
            provenance:
                conary_core::repository::dependency_model::CapabilityProvenance::AuthorDeclared,
        };
        let split = TargetStateView {
            packages: vec![
                package_identity(TargetPackageIdentity {
                    name: "provider-a".to_string(),
                    version: "1".to_string(),
                    package_release: None,
                    architecture: Some("x86_64".to_string()),
                    debian_multi_arch: None,
                    version_scheme: conary_core::repository::versioning::VersionScheme::Rpm,
                    provided_capabilities: vec![capability("feature-a")],
                    installed_trove_id: None,
                }),
                package_identity(TargetPackageIdentity {
                    name: "provider-b".to_string(),
                    version: "1".to_string(),
                    package_release: None,
                    architecture: Some("x86_64".to_string()),
                    debian_multi_arch: None,
                    version_scheme: conary_core::repository::versioning::VersionScheme::Rpm,
                    provided_capabilities: vec![capability("feature-b")],
                    installed_trove_id: None,
                }),
            ],
        };
        assert!(validate_prepared_install_dependencies(&prepared, &split).is_err());

        let combined = TargetStateView {
            packages: vec![package_identity(TargetPackageIdentity {
                name: "provider-both".to_string(),
                version: "1".to_string(),
                package_release: None,
                architecture: Some("x86_64".to_string()),
                debian_multi_arch: None,
                version_scheme: conary_core::repository::versioning::VersionScheme::Rpm,
                provided_capabilities: vec![capability("feature-a"), capability("feature-b")],
                installed_trove_id: None,
            })],
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

        let result = execute_state_restore_transaction(
            &mut conn,
            &db_path_string,
            &root_string,
            1,
            0,
            &[],
            vec![prepared],
        );
        let error = match result {
            Ok(_) => panic!("restore pre-install should fail on dangling current"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("dangling"), "{error:#}");
    }

    #[test]
    fn restore_without_generation_commits_one_wrapper_and_retryable_publication_debt() {
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

        execute_state_restore_transaction(
            &mut conn,
            &db_path_string,
            &root_string,
            1,
            0,
            &[],
            vec![prepared],
        )
        .unwrap();

        let descriptions = conn
            .prepare("SELECT description FROM changesets ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(descriptions, vec!["Restore state 1 -> 0"]);
        assert_eq!(
            Trove::find_one_by_name(&conn, "restore-fixture")
                .unwrap()
                .unwrap()
                .installed_by_changeset_id,
            Some(1)
        );
        assert_eq!(
            conary_core::db::models::GenerationPublication::pending_recoverable(&conn)
                .unwrap()
                .len(),
            1
        );
    }
}
