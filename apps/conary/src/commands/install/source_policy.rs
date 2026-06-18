// src/commands/install/source_policy.rs

use anyhow::Result;
use conary_core::repository::dependency_model::RepositoryDependencyFlavor;
use conary_core::repository::resolution_policy::{RequestScope, ResolutionPolicy};
use conary_core::repository::supported_profiles::dependency_flavor_for_name;
use tracing::{info, warn};

/// Overlay install-specific request scope from CLI flags onto the effective policy.
///
/// The `--from-distro` flag constrains the root request to a specific distro
/// flavor; `--repo` constrains to a specific repository.  Both apply to the
/// root request only (transitive deps are governed by the mixing policy).
pub(super) fn build_resolution_policy(
    mut policy: ResolutionPolicy,
    from_distro: Option<&str>,
    repo: Option<&str>,
) -> ResolutionPolicy {
    let scope = if let Some(target_distro) = from_distro {
        // Map distro name to the correct flavor for request-scope filtering
        let flavor = distro_name_to_flavor(target_distro);
        if let Some(f) = flavor {
            RequestScope::DistroFlavor(f)
        } else {
            // Unknown flavor -- use repo scope as a fallback
            RequestScope::Repository(target_distro.to_string())
        }
    } else if let Some(r) = repo {
        RequestScope::Repository(r.to_string())
    } else {
        RequestScope::Any
    };

    policy.request_scope = scope;
    policy
}

/// Resolve the canonical name for a package.
///
/// If `--from <distro>` was specified, resolve the canonical name to that
/// distro's package name.  Otherwise, use canonical expansion to find the best
/// implementation for the current system (canonical expansion applies only to
/// root requests, never deps).
pub(super) fn resolve_canonical_name(
    conn: &rusqlite::Connection,
    package: &str,
    from_distro: Option<&str>,
    policy: &ResolutionPolicy,
) -> Result<Option<String>> {
    if let Some(target_distro) = from_distro {
        if let Some(canonical) =
            conary_core::db::models::CanonicalPackage::resolve_name(conn, package)?
        {
            let impls = conary_core::db::models::PackageImplementation::find_by_canonical(
                conn,
                canonical
                    .id
                    .ok_or_else(|| anyhow::anyhow!("Canonical package has no ID"))?,
            )?;
            if let Some(imp) = impls.iter().find(|i| i.distro == target_distro) {
                info!(
                    "Resolved canonical '{}' -> '{}' for {}",
                    package, imp.distro_name, target_distro
                );
                return Ok(Some(imp.distro_name.clone()));
            }
            warn!(
                "No implementation of '{}' found for distro '{}'",
                package, target_distro
            );
        }
        Ok(None)
    } else {
        // No explicit --from-distro: use canonical resolver to expand and rank
        // implementations by pin/affinity/override.  This only applies to root
        // requests -- deps are never canonically expanded.
        use conary_core::resolver::canonical::CanonicalResolver;
        let canonical_resolver = CanonicalResolver::new(conn);
        let candidates = canonical_resolver.expand(package)?;
        if candidates.len() > 1 {
            let ranked = canonical_resolver.rank_candidates_with_policy(&candidates, policy)?;
            info!(
                "Canonical expansion for '{}': {} implementations, best = '{}' ({})",
                package,
                ranked.len(),
                ranked[0].distro_name,
                ranked[0].distro,
            );
            // Use the top-ranked implementation
            Ok(Some(ranked[0].distro_name.clone()))
        } else if candidates.len() == 1 {
            Ok(Some(candidates[0].distro_name.clone()))
        } else {
            // No canonical mapping -- use the name as-is
            Ok(None)
        }
    }
}

/// Map a distro identifier string to its `RepositoryDependencyFlavor`.
///
/// Returns `None` for unrecognised distro names.
fn distro_name_to_flavor(distro: &str) -> Option<RepositoryDependencyFlavor> {
    dependency_flavor_for_name(distro)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distro_name_to_flavor_accepts_public_ids_and_route_slugs() {
        assert_eq!(
            distro_name_to_flavor("fedora-44"),
            Some(RepositoryDependencyFlavor::Rpm)
        );
        assert_eq!(
            distro_name_to_flavor("fedora"),
            Some(RepositoryDependencyFlavor::Rpm)
        );
        assert_eq!(
            distro_name_to_flavor("ubuntu-26.04"),
            Some(RepositoryDependencyFlavor::Deb)
        );
        assert_eq!(
            distro_name_to_flavor("ubuntu"),
            Some(RepositoryDependencyFlavor::Deb)
        );
        assert_eq!(
            distro_name_to_flavor("arch"),
            Some(RepositoryDependencyFlavor::Arch)
        );
    }

    #[test]
    fn distro_name_to_flavor_rejects_unsupported_derivatives() {
        for name in [
            "debian",
            "debian-13",
            "linux-mint",
            "ubuntu-noble",
            "fedora-45",
        ] {
            assert_eq!(distro_name_to_flavor(name), None, "{name}");
        }
    }

    #[test]
    fn distro_name_to_flavor_unknown() {
        assert_eq!(distro_name_to_flavor("nixos"), None);
    }
}
