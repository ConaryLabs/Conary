// conary-core/src/ccs/v2/identity.rs

use super::schema::*;
use anyhow::Result;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct ContentIdentityProjectionV2<'a> {
    pub identity: &'a PackageIdentityV2,
    pub kind: &'a PackageKindV2,
    pub provides: &'a [DependencyEntryV2],
    pub requires: &'a [DependencyEntryV2],
    pub components: &'a std::collections::BTreeMap<String, ComponentAuthorityV2>,
    pub lifecycle: &'a LifecycleAuthorityV2,
    pub provenance: &'a ProvenanceAuthorityV2,
}

pub fn compute_v2_content_identity(authority: &AuthorityDocumentV2) -> Result<String> {
    let projection = ContentIdentityProjectionV2 {
        identity: &authority.identity,
        kind: &authority.kind,
        provides: &authority.provides,
        requires: &authority.requires,
        components: &authority.components,
        lifecycle: &authority.lifecycle,
        provenance: &authority.provenance,
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
    fn authority_changes_change_identity() {
        let mut authority = crate::ccs::v2::test_support::package_authority_with_one_file("id");
        let first = compute_v2_content_identity(&authority).unwrap();
        authority
            .requires
            .push(crate::ccs::v2::schema::DependencyEntryV2::package(
                "openssl",
            ));
        let second = compute_v2_content_identity(&authority).unwrap();
        assert_ne!(first, second);
    }
}
