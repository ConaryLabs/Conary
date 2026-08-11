// apps/conary/src/commands/install/repository_batch.rs

//! Authenticated preparation of exact repository selections for one batch.

use super::batch::{
    BatchInstallResult, BatchInstaller, PreparedPackage, prepare_ccs_package_for_batch,
    prepare_package_for_batch,
};
use super::conversion::{
    validate_selected_repository_ccs_identity, validate_selected_repository_native_identity,
};
use super::{
    CcsEnvelopeAuthority, InstallIntent, repository_install_provenance_from_package,
    verify_ccs_package_authority,
};
use anyhow::{Context, Result};
use conary_core::ccs::CcsPackage;
use conary_core::db::models::InstallReason;
use conary_core::db::paths::keyring_dir;
use conary_core::repository;
use tempfile::TempDir;

use super::dep_resolution::{self, ResolvedDep};

pub(super) struct RepositoryBatchSelection {
    pub(super) selected: ResolvedDep,
    pub(super) install_reason: InstallReason,
    pub(super) selection_reason: String,
    pub(super) allow_downgrade: bool,
    pub(super) intent: InstallIntent,
}

pub(super) struct PreparedRepositoryBatch {
    packages: Vec<PreparedPackage>,
    _download_root: TempDir,
}

impl PreparedRepositoryBatch {
    pub(super) fn push(&mut self, package: PreparedPackage) {
        self.packages.push(package);
    }

    pub(super) fn install(self, installer: BatchInstaller<'_>) -> Result<()> {
        installer.install_batch(self.packages)
    }

    pub(super) fn validate(self, installer: BatchInstaller<'_>) -> Result<()> {
        installer.validate_batch(self.packages)
    }

    pub(super) fn install_with_result(
        self,
        installer: BatchInstaller<'_>,
    ) -> Result<BatchInstallResult> {
        installer.install_batch_with_result(self.packages)
    }
}

pub(super) async fn prepare_repository_batch(
    db_path: &str,
    selections: Vec<RepositoryBatchSelection>,
) -> Result<PreparedRepositoryBatch> {
    let selected = selections
        .iter()
        .map(|selection| selection.selected.clone())
        .collect::<Vec<_>>();
    let conn = super::super::open_db(db_path)?;
    let to_download = dep_resolution::exact_repository_downloads(&conn, &selected)?;
    drop(conn);

    let download_root = TempDir::new()?;
    let downloaded = repository::download_dependencies(
        &to_download,
        download_root.path(),
        &keyring_dir(db_path),
    )
    .await?;
    if downloaded.len() != selections.len() || to_download.len() != selections.len() {
        anyhow::bail!(
            "repository package-set downloader returned {} artifacts for {} exact selections",
            downloaded.len(),
            selections.len()
        );
    }

    let mut packages = Vec::with_capacity(selections.len());
    for (((expected_name, package_with_repo), (downloaded_name, path)), selection) in
        to_download.iter().zip(&downloaded).zip(&selections)
    {
        if expected_name != downloaded_name || expected_name != &selection.selected.package.name {
            anyhow::bail!(
                "repository package-set download order drifted: expected {}, received {}",
                selection.selected.package.name,
                downloaded_name
            );
        }
        let provenance = repository_install_provenance_from_package(
            &package_with_repo.package,
            &package_with_repo.repository,
        )?;

        if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("ccs"))
        {
            let ccs_path = path
                .to_str()
                .context("Invalid CCS package path (non-UTF8)")?;
            let verified = verify_ccs_package_authority(
                db_path,
                path,
                &CcsEnvelopeAuthority::Repository,
                Some(&provenance),
            )?;
            let package =
                CcsPackage::from_verified_archive(ccs_path, &verified).with_context(|| {
                    format!("Failed to construct verified CCS package {downloaded_name}")
                })?;
            validate_selected_repository_ccs_identity(
                &package,
                &selection.selected.package,
                Some(&provenance),
            )?;
            crate::commands::ccs::validate_ccs_capability_declaration(&package)?;
            packages.push(prepare_ccs_package_for_batch(
                &package,
                db_path,
                selection.install_reason.clone(),
                &selection.selection_reason,
                selection.allow_downgrade,
                selection.intent,
                Some(provenance),
            )?);
        } else {
            let Some(mut package) = prepare_package_for_batch(
                path,
                db_path,
                selection.install_reason.clone(),
                &selection.selection_reason,
                selection.allow_downgrade,
                provenance.source_profile.as_deref(),
            )?
            else {
                anyhow::bail!(
                    "SAT selected package {downloaded_name}, but its exact artifact is already installed"
                );
            };
            validate_selected_repository_native_identity(
                &package.name,
                &package.version,
                package.package_release.as_deref(),
                package.architecture.as_deref(),
                package.semantics.version_scheme,
                &selection.selected.package,
                &provenance,
            )?;
            package.repository_provenance = Some(provenance);
            packages.push(package);
        }
    }

    Ok(PreparedRepositoryBatch {
        packages,
        _download_root: download_root,
    })
}
