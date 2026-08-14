// apps/conary/src/commands/install/source_policy.rs

use super::RepositoryInstallProvenance;
use anyhow::Result;
use conary_core::repository::resolution_policy::{
    DependencyMixingPolicy, RequestScope, ResolutionPolicy,
};
use tracing::{info, warn};

/// Overlay install-specific request scope from CLI flags onto the effective policy.
///
/// The `--from` flag constrains the root request to an exact source identity;
/// `--repo` constrains to a specific repository. Both apply to the
/// root request only (transitive deps are governed by the mixing policy).
pub(super) fn build_resolution_policy(
    conn: &rusqlite::Connection,
    mut policy: ResolutionPolicy,
    from_source: Option<&str>,
    repo: Option<&str>,
) -> Result<ResolutionPolicy> {
    let scope = if let Some(source_identity) = from_source {
        conary_core::repository::resolution_policy::validate_source_identity(
            source_identity,
            "--from source identity",
        )
        .map_err(anyhow::Error::msg)?;
        policy.set_primary_source_identity(Some(source_identity.to_string()));
        RequestScope::SourceIdentity(source_identity.to_string())
    } else if let Some(r) = repo {
        let repository = conary_core::db::models::Repository::find_by_name(conn, r)?
            .ok_or_else(|| anyhow::anyhow!("repository '{r}' does not exist"))?;
        policy.set_primary_source_identity(
            repository.resolution_source_identity()?.map(str::to_string),
        );
        RequestScope::Repository(r.to_string())
    } else {
        RequestScope::Any
    };

    policy.request_scope = scope;
    Ok(policy)
}

/// Bind dependency resolution to exact selected-root provenance.
///
/// An explicit scope may already supply a primary source identity. Otherwise
/// a repository-backed root establishes it from the
/// selected package/repository contract. Local files do not manufacture that
/// authority; strict dependency solving will require the user to scope
/// the transaction.
pub(super) fn bind_transaction_source_identity(
    mut policy: ResolutionPolicy,
    provenance: Option<&RepositoryInstallProvenance>,
) -> Result<ResolutionPolicy> {
    if let Some(provenance) = provenance {
        let selected = match provenance.source_identity.as_deref() {
            Some(selected) => Some(selected),
            None if provenance.version_scheme
                == conary_core::repository::versioning::VersionScheme::Conary =>
            {
                None
            }
            None => {
                return Err(anyhow::anyhow!(
                    "selected repository {} has no exact source identity",
                    provenance.repository_id
                ));
            }
        };

        if let Some(selected) = selected {
            match policy.primary_source_identity() {
                Some(primary)
                    if policy.mixing == DependencyMixingPolicy::Strict && primary != selected =>
                {
                    anyhow::bail!(
                        "selected root source identity '{}' conflicts with strict transaction source identity '{}'",
                        selected,
                        primary
                    );
                }
                Some(_) => {}
                None => policy.set_primary_source_identity(Some(selected.to_string())),
            }
        }
    }

    policy
        .validate_source_identities()
        .map_err(anyhow::Error::msg)?;
    Ok(policy)
}

/// Resolve the canonical name for a package.
///
/// If `--from <source>` names a feed preset with canonical mappings, project
/// the canonical name to that preset's package name. Opaque source identities
/// that are not presets remain fully valid and keep the package name unchanged.
/// Otherwise, use canonical expansion to find the uniquely authorized root
/// implementation (canonical expansion never applies to dependencies).
pub(super) fn resolve_canonical_name(
    conn: &rusqlite::Connection,
    package: &str,
    from_source: Option<&str>,
    policy: &ResolutionPolicy,
) -> Result<Option<String>> {
    if let Some(target_source) = from_source {
        conary_core::repository::resolution_policy::validate_source_identity(
            target_source,
            "--from source identity",
        )
        .map_err(anyhow::Error::msg)?;
        let Some(profile_id) = source_profile_projection(target_source) else {
            return Ok(None);
        };
        if let Some(canonical) =
            conary_core::db::models::CanonicalPackage::resolve_name(conn, package)?
        {
            let canonical_id = canonical
                .id
                .ok_or_else(|| anyhow::anyhow!("canonical package has no ID"))?;
            if let Some(implementation) =
                conary_core::db::models::PackageImplementation::find_for_distro(
                    conn,
                    canonical_id,
                    profile_id,
                )?
            {
                info!(
                    "Resolved canonical '{}' -> '{}' for {}",
                    package, implementation.distro_name, profile_id
                );
                return Ok(Some(implementation.distro_name));
            }
            let available = conary_core::db::models::PackageImplementation::find_by_canonical(
                conn,
                canonical_id,
            )?;
            warn!(
                "No implementation of '{}' found for profile '{}' (available: {})",
                package,
                profile_id,
                available
                    .iter()
                    .map(|implementation| implementation.distro.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        Ok(None)
    } else {
        // No explicit --from: expand implementations and select only when
        // exact policy provides a unique authority. This applies only to root
        // requests; dependencies are never canonically expanded.
        use conary_core::resolver::canonical::CanonicalResolver;
        let canonical_resolver = CanonicalResolver::new(conn);
        let candidates = canonical_resolver.expand(package)?;
        if let Some(selected) =
            canonical_resolver.select_candidate_with_policy(&candidates, policy)?
        {
            info!(
                "Canonical expansion for '{}': {} implementations, selected = '{}' ({})",
                package,
                candidates.len(),
                selected.distro_name,
                selected.distro,
            );
            Ok(Some(selected.distro_name))
        } else {
            Ok(None)
        }
    }
}

/// Project an opaque source identity onto a named feed profile only when the
/// exact public ID is catalogued. The source identity remains the policy
/// authority; this optional projection supplies source-format ABI details such
/// as the Arch scriptlet shell.
pub(super) fn source_profile_projection(value: &str) -> Option<&'static str> {
    conary_core::repository::supported_profiles::profile_by_public_id(value)
        .map(conary_core::repository::supported_profiles::SupportedProfile::id)
}

/// Select the optional source-format profile used for lifecycle ABI details.
/// Repository provenance wins because it describes the exact selected
/// artifact. A named `--from` identity supplies the profile for a local
/// artifact; an uncatalogued identity deliberately supplies no projection.
pub(super) fn effective_source_profile<'a>(
    provenance: Option<&'a RepositoryInstallProvenance>,
    requested_source_profile: Option<&'a str>,
) -> Option<&'a str> {
    provenance
        .and_then(|provenance| provenance.source_profile.as_deref())
        .or(requested_source_profile)
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::repository::RepositorySourceKind;
    use conary_core::repository::versioning::VersionScheme;

    fn provenance(profile: Option<&str>) -> RepositoryInstallProvenance {
        RepositoryInstallProvenance {
            repository_id: 7,
            source_identity: profile.map(str::to_string),
            source_profile: profile.map(str::to_string),
            version_scheme: VersionScheme::Rpm,
            source_kind: RepositorySourceKind::Native,
        }
    }

    #[test]
    fn named_feed_ids_enable_canonical_projection() {
        assert_eq!(source_profile_projection("fedora-44"), Some("fedora-44"));
        assert_eq!(
            source_profile_projection("ubuntu-26.04"),
            Some("ubuntu-26.04")
        );
        assert_eq!(source_profile_projection("arch"), Some("arch"));
    }

    #[test]
    fn opaque_source_identities_do_not_imply_named_canonical_projection() {
        for name in [
            "debian",
            "debian-13",
            "fedora",
            "linux-mint",
            "ubuntu",
            "ubuntu-noble",
            "fedora-45",
        ] {
            assert_eq!(source_profile_projection(name), None, "{name}");
        }
    }

    #[test]
    fn unknown_source_identity_has_no_named_canonical_projection() {
        assert_eq!(source_profile_projection("nixos"), None);
    }

    #[test]
    fn lifecycle_profile_uses_selected_artifact_then_named_request_projection() {
        let selected = provenance(Some("fedora-44"));
        assert_eq!(
            effective_source_profile(Some(&selected), Some("arch")),
            Some("fedora-44")
        );
        assert_eq!(effective_source_profile(None, Some("arch")), Some("arch"));
        assert_eq!(effective_source_profile(None, None), None);
    }

    #[test]
    fn selected_root_binds_unscoped_transaction_profile() {
        let policy = bind_transaction_source_identity(
            ResolutionPolicy::new(),
            Some(&provenance(Some("fedora-44"))),
        )
        .unwrap();
        assert_eq!(policy.primary_source_identity(), Some("fedora-44"));
    }

    #[test]
    fn strict_transaction_rejects_selected_root_profile_conflict() {
        let error = bind_transaction_source_identity(
            ResolutionPolicy::new().with_primary_source_identity("ubuntu-26.04"),
            Some(&provenance(Some("fedora-44"))),
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("conflicts with strict"),
            "{error}"
        );
    }

    #[test]
    fn selected_root_accepts_uncatalogued_exact_source_identity() {
        for source_identity in ["linux-mint:22", "opensuse:tumbleweed", "artix:rolling"] {
            let policy = bind_transaction_source_identity(
                ResolutionPolicy::new(),
                Some(&provenance(Some(source_identity))),
            )
            .unwrap();
            assert_eq!(
                policy.primary_source_identity(),
                Some(source_identity),
                "{source_identity}"
            );
        }
    }

    #[test]
    fn selected_native_root_requires_an_exact_source_identity() {
        assert!(
            bind_transaction_source_identity(ResolutionPolicy::new(), Some(&provenance(None)))
                .is_err()
        );
    }

    #[test]
    fn source_native_conary_package_does_not_invent_a_distro_profile() {
        let conary = RepositoryInstallProvenance {
            repository_id: 9,
            source_identity: None,
            source_profile: None,
            version_scheme: VersionScheme::Conary,
            source_kind: RepositorySourceKind::Static,
        };
        let policy =
            bind_transaction_source_identity(ResolutionPolicy::new(), Some(&conary)).unwrap();
        assert_eq!(policy.primary_source_identity(), None);
    }

    #[test]
    fn guarded_transaction_preserves_persisted_primary_profile() {
        let policy = bind_transaction_source_identity(
            ResolutionPolicy::new()
                .with_mixing(DependencyMixingPolicy::Guarded)
                .with_primary_source_identity("ubuntu-26.04"),
            Some(&provenance(Some("fedora-44"))),
        )
        .unwrap();
        assert_eq!(policy.primary_source_identity(), Some("ubuntu-26.04"));
    }
}
