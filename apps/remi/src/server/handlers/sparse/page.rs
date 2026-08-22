// apps/remi/src/server/handlers/sparse/page.rs

//! Stable sparse-index page selection from one pinned immutable catalog.

use conary_core::repository::remi_metadata::{
    REMI_SPARSE_MIN_PACKAGE_SIZE, RemiSparsePackageList, RemiSparsePackagePage,
    RemiSparseResolutionEntry, validate_remi_public_name,
};

use crate::server::catalog_authority::CatalogAuthority;
use crate::server::profile_catalog::ProfileCatalog;

/// Build a paginated list of unique package names from one exact profile
/// revision. The pinned reader remains alive through count and page selection.
pub(super) fn build_package_list(
    catalog_authority: &CatalogAuthority,
    distro: &str,
    page: usize,
    per_page: usize,
) -> Result<RemiSparsePackageList, anyhow::Error> {
    let source_profile = source_profile_for_route(distro)?;
    let pinned = catalog_authority.open_active_profile(source_profile)?;
    let catalog = ProfileCatalog::new(&pinned);
    let selection = select_sparse_name_page(&catalog, page, per_page)?;

    Ok(RemiSparsePackageList {
        distro: distro.to_string(),
        source_profile: source_profile.to_string(),
        packages: selection.names,
        total: selection.total,
        page,
        per_page,
    })
}

/// Build one bounded resolution page from one exact profile revision.
pub(super) fn build_package_page(
    catalog_authority: &CatalogAuthority,
    distro: &str,
    page: usize,
    per_page: usize,
) -> Result<RemiSparsePackagePage, anyhow::Error> {
    let source_profile = source_profile_for_route(distro)?;
    let pinned = catalog_authority.open_active_profile(source_profile)?;
    let catalog = ProfileCatalog::new(&pinned);
    let selection = select_sparse_name_page(&catalog, page, per_page)?;
    let revision = catalog.revision()?;
    let minimum_size = sparse_minimum_package_size()?;

    let mut entries = Vec::with_capacity(selection.names.len());
    for name in selection.names {
        let versions = catalog.find_downloadable_packages_by_name(&name, minimum_size)?;
        if versions.is_empty() {
            anyhow::bail!(
                "immutable catalog name page listed package '{name}' without a package record"
            );
        }
        entries.push(RemiSparseResolutionEntry {
            name,
            distro: distro.to_string(),
            versions,
        });
    }

    Ok(RemiSparsePackagePage {
        distro: distro.to_string(),
        source_profile: source_profile.to_string(),
        revision,
        packages: entries,
        total: selection.total,
        page,
        per_page,
    })
}

struct SparseNamePageSelection {
    names: Vec<String>,
    total: usize,
}

fn select_sparse_name_page(
    catalog: &ProfileCatalog<'_>,
    page: usize,
    per_page: usize,
) -> Result<SparseNamePageSelection, anyhow::Error> {
    let offset = page
        .checked_sub(1)
        .and_then(|zero_based| zero_based.checked_mul(per_page))
        .ok_or_else(|| anyhow::anyhow!("sparse page offset overflow"))?;
    let minimum_size = sparse_minimum_package_size()?;
    let page = catalog.package_name_page(offset, per_page, minimum_size)?;
    for package in &page.names {
        validate_remi_public_name(package).map_err(|reason| {
            anyhow::anyhow!(
                "immutable catalog contains invalid sparse package name {package:?}: {reason}"
            )
        })?;
    }

    Ok(SparseNamePageSelection {
        names: page.names,
        total: page.total,
    })
}

fn sparse_minimum_package_size() -> Result<u64, anyhow::Error> {
    u64::try_from(REMI_SPARSE_MIN_PACKAGE_SIZE)
        .map_err(|_| anyhow::anyhow!("sparse minimum package size is negative"))
}

fn source_profile_for_route(distro: &str) -> Result<&'static str, anyhow::Error> {
    conary_core::repository::supported_profiles::profile_for_remi_route(distro)
        .map(|profile| profile.id())
        .ok_or_else(|| anyhow::anyhow!("unsupported public route '{distro}'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_mapping_requires_one_exact_supported_profile() {
        assert_eq!(source_profile_for_route("fedora").unwrap(), "fedora-44");
        assert!(source_profile_for_route("fedora44").is_err());
        assert!(source_profile_for_route("made-up").is_err());
    }
}
