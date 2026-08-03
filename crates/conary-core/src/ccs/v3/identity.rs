// conary-core/src/ccs/v3/identity.rs

use super::schema::*;
use crate::repository::dependency_model::RepositoryRequirementGroup;
use anyhow::Result;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct ContentIdentityProjectionV3<'a> {
    pub identity: &'a PackageIdentityV3,
    pub kind: &'a PackageKindV3,
    pub provided_capabilities: &'a [ProvidedCapabilityV3],
    pub requirements: &'a [RepositoryRequirementGroup],
    pub relations: &'a [RepositoryRequirementGroup],
    #[serde(skip_serializing_if = "referenced_option_is_none")]
    pub execution_capabilities: &'a Option<crate::capability::CapabilityDeclaration>,
    #[serde(skip_serializing_if = "referenced_slice_is_empty")]
    pub file_capabilities: &'a [crate::ccs::manifest::FileCapability],
    pub components: &'a std::collections::BTreeMap<String, ComponentAuthorityV3>,
    pub lifecycle: &'a LifecycleAuthorityV3,
    pub provenance: ProvenanceAuthorityV3,
}

fn referenced_option_is_none<T>(value: &&Option<T>) -> bool {
    value.is_none()
}

fn referenced_slice_is_empty<T>(value: &&[T]) -> bool {
    value.is_empty()
}

pub fn compute_v3_content_identity(authority: &AuthorityDocumentV3) -> Result<String> {
    let mut provenance = authority.provenance.clone();
    provenance.foreign_conversion_boundary_hash = None;
    let projection = ContentIdentityProjectionV3 {
        identity: &authority.identity,
        kind: &authority.kind,
        provided_capabilities: &authority.provided_capabilities,
        requirements: &authority.requirements,
        relations: &authority.relations,
        execution_capabilities: &authority.execution_capabilities,
        file_capabilities: &authority.file_capabilities,
        components: &authority.components,
        lifecycle: &authority.lifecycle,
        provenance,
    };
    let bytes = crate::ccs::attestation::canonical_json_bytes(&projection)?;
    Ok(crate::hash::sha256_prefixed(&bytes))
}

pub fn compute_v3_file_merkle_root(authority: &AuthorityDocumentV3) -> Result<String> {
    let PackageKindV3::Package(data) = &authority.kind else {
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
        let authority = crate::ccs::v3::test_support::package_authority_with_one_file("id");
        let first = compute_v3_content_identity(&authority).unwrap();
        let second = compute_v3_content_identity(&authority).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn authority_changes_change_identity() {
        let mut authority = crate::ccs::v3::test_support::package_authority_with_one_file("id");
        let first = compute_v3_content_identity(&authority).unwrap();
        authority.requirements.push(
            crate::repository::dependency_model::RepositoryRequirementGroup::simple(
                crate::repository::dependency_model::RepositoryRequirementKind::Depends,
                crate::repository::dependency_model::RepositoryRequirementClause::name_only(
                    "openssl".to_string(),
                ),
            ),
        );
        let second = compute_v3_content_identity(&authority).unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn relation_changes_change_identity() {
        let mut authority = crate::ccs::v3::test_support::package_authority_with_one_file("id");
        let first = compute_v3_content_identity(&authority).unwrap();
        authority.relations.push(
            crate::repository::dependency_model::RepositoryRequirementGroup::simple(
                crate::repository::dependency_model::RepositoryRequirementKind::Conflict,
                crate::repository::dependency_model::RepositoryRequirementClause::name_only(
                    "incompatible-package".to_string(),
                ),
            ),
        );

        let second = compute_v3_content_identity(&authority).unwrap();

        assert_ne!(first, second);
    }

    #[test]
    fn package_capability_changes_change_identity() {
        let mut authority = crate::ccs::v3::test_support::package_authority_with_one_file("id");
        let first = compute_v3_content_identity(&authority).unwrap();
        authority.execution_capabilities =
            Some(crate::capability::CapabilityDeclaration::default());

        let second = compute_v3_content_identity(&authority).unwrap();

        assert_ne!(first, second);
    }

    #[test]
    fn file_capability_changes_change_identity() {
        let mut authority = crate::ccs::v3::test_support::package_authority_with_one_file("id");
        let first = compute_v3_content_identity(&authority).unwrap();
        authority.file_capabilities = vec![crate::ccs::manifest::FileCapability {
            path: "/usr/bin/hello".to_string(),
            capabilities: vec!["cap_net_bind_service".to_string()],
            permitted: true,
            effective: true,
            inheritable: false,
        }];

        let second = compute_v3_content_identity(&authority).unwrap();

        assert_ne!(first, second);
    }
}
