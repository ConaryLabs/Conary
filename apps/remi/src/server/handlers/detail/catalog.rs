// apps/remi/src/server/handlers/detail/catalog.rs
//! Exact catalog selection helpers shared by package-detail responses.

use crate::server::catalog_authority::{
    CatalogAuthority, PinnedProfileCatalog, ProfileRevisionSelection,
};
use anyhow::Context;
use conary_core::repository::catalog::CatalogPackageRecordV1;
use conary_core::repository::versioning::compare_repo_versions;
use std::cmp::Ordering;
use std::collections::{HashMap, hash_map::Entry};

pub(super) fn source_profile_for_route(route_slug: &str) -> anyhow::Result<&'static str> {
    conary_core::repository::supported_profiles::profile_for_remi_route(route_slug)
        .map(conary_core::repository::supported_profiles::SupportedProfile::id)
        .ok_or_else(|| anyhow::anyhow!("unsupported public route '{route_slug}'"))
}

pub(super) fn latest_catalog_package(
    packages: &[CatalogPackageRecordV1],
) -> anyhow::Result<Option<&CatalogPackageRecordV1>> {
    let Some((first, rest)) = packages.split_first() else {
        return Ok(None);
    };
    let mut latest = first;
    for candidate in rest {
        match compare_repo_versions(latest.version_scheme, &candidate.version, &latest.version)? {
            Ordering::Greater => latest = candidate,
            Ordering::Equal => {
                if (
                    &candidate.package_release,
                    &candidate.architecture,
                    &candidate.package_key_sha256,
                ) > (
                    &latest.package_release,
                    &latest.architecture,
                    &latest.package_key_sha256,
                ) {
                    latest = candidate;
                }
            }
            Ordering::Less => {}
        }
    }
    Ok(Some(latest))
}

pub(super) fn extract_catalog_metadata(
    package: &CatalogPackageRecordV1,
) -> anyhow::Result<(Option<String>, Option<String>)> {
    let Some(metadata) = package.metadata.as_deref() else {
        return Ok((None, None));
    };
    let metadata = serde_json::from_str::<serde_json::Value>(metadata).with_context(|| {
        format!(
            "immutable catalog package '{}' version '{}' has malformed metadata JSON",
            package.name, package.version
        )
    })?;
    let license = metadata
        .get("license")
        .and_then(|value| value.as_str())
        .map(String::from);
    let homepage = metadata
        .get("homepage")
        .or_else(|| metadata.get("url"))
        .and_then(|value| value.as_str())
        .map(String::from);
    Ok((license, homepage))
}

/// Pin each profile used by one response once, keeping every item on the
/// exact revision selected for that response.
pub(super) fn pin_response_catalog<'a>(
    catalog_authority: &CatalogAuthority,
    pinned_profiles: &'a mut HashMap<String, PinnedProfileCatalog>,
    selection: &ProfileRevisionSelection,
) -> anyhow::Result<&'a PinnedProfileCatalog> {
    match pinned_profiles.entry(selection.source_profile.clone()) {
        Entry::Occupied(entry) => Ok(entry.into_mut()),
        Entry::Vacant(entry) => {
            let pinned = catalog_authority.open_selected_profile(selection)?;
            Ok(entry.insert(pinned))
        }
    }
}

pub(super) fn route_for_source_profile(source_profile: &str) -> anyhow::Result<&'static str> {
    conary_core::repository::supported_profiles::profile_by_public_id(source_profile)
        .map(conary_core::repository::supported_profiles::SupportedProfile::remi_route_slug)
        .with_context(|| format!("unsupported persisted source profile '{source_profile}'"))
}
