// conary-core/src/ccs/v2/identity.rs

use super::schema::*;
use crate::repository::dependency_model::RepositoryRequirementGroup;
use anyhow::Result;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct ContentIdentityProjectionV2<'a> {
    pub identity: &'a PackageIdentityV2,
    pub kind: &'a PackageKindV2,
    pub provides: &'a [ProvidedCapabilityV2],
    pub requirements: &'a [RepositoryRequirementGroup],
    pub relations: &'a [RepositoryRequirementGroup],
    #[serde(skip_serializing_if = "referenced_option_is_none")]
    pub capabilities: &'a Option<crate::capability::CapabilityDeclaration>,
    #[serde(skip_serializing_if = "referenced_slice_is_empty")]
    pub file_capabilities: &'a [crate::ccs::manifest::FileCapability],
    pub components: &'a std::collections::BTreeMap<String, ComponentAuthorityV2>,
    pub lifecycle: &'a LifecycleAuthorityV2,
    pub provenance: ProvenanceAuthorityV2,
}

fn referenced_option_is_none<T>(value: &&Option<T>) -> bool {
    value.is_none()
}

fn referenced_slice_is_empty<T>(value: &&[T]) -> bool {
    value.is_empty()
}

pub fn compute_v2_content_identity(authority: &AuthorityDocumentV2) -> Result<String> {
    let mut provenance = authority.provenance.clone();
    provenance.foreign_conversion_boundary_hash = None;
    let projection = ContentIdentityProjectionV2 {
        identity: &authority.identity,
        kind: &authority.kind,
        provides: &authority.provides,
        requirements: &authority.requirements,
        relations: &authority.relations,
        capabilities: &authority.capabilities,
        file_capabilities: &authority.file_capabilities,
        components: &authority.components,
        lifecycle: &authority.lifecycle,
        provenance,
    };
    let bytes = crate::ccs::attestation::canonical_json_bytes(&projection)?;
    Ok(crate::hash::sha256_prefixed(&bytes))
}

pub fn compute_v2_file_merkle_root(authority: &AuthorityDocumentV2) -> Result<String> {
    let PackageKindV2::Package(data) = &authority.kind else {
        return Ok(crate::hash::sha256_prefixed(
            &crate::ccs::attestation::canonical_json_bytes(&authority.kind)?,
        ));
    };
    let bytes =
        crate::ccs::attestation::canonical_json_bytes(&(&authority.components, &data.files))?;
    Ok(crate::hash::sha256_prefixed(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resigning_does_not_change_identity() {
        let authority = crate::ccs::v2::test_support::package_authority_with_one_file("id");
        let first = compute_v2_content_identity(&authority).unwrap();
        let second = compute_v2_content_identity(&authority).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn absent_capability_authority_preserves_the_prior_identity_projection() {
        #[derive(serde::Serialize)]
        struct PriorProjection<'a> {
            identity: &'a PackageIdentityV2,
            kind: &'a PackageKindV2,
            provides: &'a [ProvidedCapabilityV2],
            requirements: &'a [RepositoryRequirementGroup],
            relations: &'a [RepositoryRequirementGroup],
            components: &'a std::collections::BTreeMap<String, ComponentAuthorityV2>,
            lifecycle: &'a LifecycleAuthorityV2,
            provenance: ProvenanceAuthorityV2,
        }

        let authority = crate::ccs::v2::test_support::package_authority_with_one_file("id");
        assert!(authority.capabilities.is_none());
        assert!(authority.file_capabilities.is_empty());
        let mut provenance = authority.provenance.clone();
        provenance.foreign_conversion_boundary_hash = None;
        let prior = PriorProjection {
            identity: &authority.identity,
            kind: &authority.kind,
            provides: &authority.provides,
            requirements: &authority.requirements,
            relations: &authority.relations,
            components: &authority.components,
            lifecycle: &authority.lifecycle,
            provenance,
        };
        let prior_bytes = crate::ccs::attestation::canonical_json_bytes(&prior).unwrap();
        let prior_identity = crate::hash::sha256_prefixed(&prior_bytes);

        assert_eq!(
            compute_v2_content_identity(&authority).unwrap(),
            prior_identity
        );
    }

    #[test]
    fn authority_changes_change_identity() {
        let mut authority = crate::ccs::v2::test_support::package_authority_with_one_file("id");
        let first = compute_v2_content_identity(&authority).unwrap();
        authority.requirements.push(
            crate::repository::dependency_model::RepositoryRequirementGroup::simple(
                crate::repository::dependency_model::RepositoryRequirementKind::Depends,
                crate::repository::dependency_model::RepositoryRequirementClause::name_only(
                    "openssl".to_string(),
                ),
            ),
        );
        let second = compute_v2_content_identity(&authority).unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn relation_changes_change_identity() {
        let mut authority = crate::ccs::v2::test_support::package_authority_with_one_file("id");
        let first = compute_v2_content_identity(&authority).unwrap();
        authority.relations.push(
            crate::repository::dependency_model::RepositoryRequirementGroup::simple(
                crate::repository::dependency_model::RepositoryRequirementKind::Conflict,
                crate::repository::dependency_model::RepositoryRequirementClause::name_only(
                    "incompatible-package".to_string(),
                ),
            ),
        );

        let second = compute_v2_content_identity(&authority).unwrap();

        assert_ne!(first, second);
    }

    #[test]
    fn package_capability_changes_change_identity() {
        let mut authority = crate::ccs::v2::test_support::package_authority_with_one_file("id");
        let first = compute_v2_content_identity(&authority).unwrap();
        authority.capabilities = Some(crate::capability::CapabilityDeclaration::default());

        let second = compute_v2_content_identity(&authority).unwrap();

        assert_ne!(first, second);
    }

    #[test]
    fn file_capability_changes_change_identity() {
        let mut authority = crate::ccs::v2::test_support::package_authority_with_one_file("id");
        let first = compute_v2_content_identity(&authority).unwrap();
        authority.file_capabilities = vec![crate::ccs::manifest::FileCapability {
            path: "/usr/bin/hello".to_string(),
            capabilities: vec!["cap_net_bind_service".to_string()],
            permitted: true,
            effective: true,
            inheritable: false,
        }];

        let second = compute_v2_content_identity(&authority).unwrap();

        assert_ne!(first, second);
    }
}
