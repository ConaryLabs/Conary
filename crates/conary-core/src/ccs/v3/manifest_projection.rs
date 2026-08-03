// conary-core/src/ccs/v3/manifest_projection.rs

//! Shared projection of signed v3 identity into the compatibility manifest.

use super::AuthorityDocumentV3;
use crate::ccs::manifest::{CcsManifest, Platform};

pub(crate) fn project_manifest_identity(
    authority: &AuthorityDocumentV3,
    description: String,
) -> CcsManifest {
    let mut manifest =
        CcsManifest::new_minimal(&authority.identity.name, &authority.identity.version);
    manifest.package.version_scheme = authority.identity.version_scheme;
    manifest.package.release = authority.identity.release.clone();
    manifest.package.kind = authority.identity.kind;
    manifest.package.debian_multi_arch = authority.identity.debian_multi_arch;
    manifest.package.platform =
        if authority.identity.platform.is_some() || authority.identity.architecture.is_some() {
            Some(Platform {
                os: authority.identity.platform.clone().unwrap_or_default(),
                arch: authority.identity.architecture.clone(),
                ..Platform::default()
            })
        } else {
            None
        };
    manifest.package.description = description;
    manifest
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::dependency_model::DebianMultiArch;
    use crate::repository::versioning::VersionScheme;

    #[test]
    fn exact_v3_identity_overwrites_native_authoring_defaults() {
        let mut authority = AuthorityDocumentV3::package_for_tests("identity-projection");
        authority.identity.version_scheme = VersionScheme::Debian;
        authority.identity.release = "7".to_string();
        authority.identity.architecture = Some("all".to_string());
        authority.identity.debian_multi_arch = Some(DebianMultiArch::Foreign);
        authority.identity.platform = None;

        let manifest = project_manifest_identity(&authority, "untrusted inspection".to_string());

        assert_eq!(manifest.package.version_scheme, VersionScheme::Debian);
        assert_eq!(manifest.package.release, "7");
        assert_eq!(manifest.package.kind, authority.identity.kind);
        assert_eq!(
            manifest.package.debian_multi_arch,
            Some(DebianMultiArch::Foreign)
        );
        let platform = manifest.package.platform.as_ref().unwrap();
        assert_eq!(platform.os, "");
        assert_eq!(platform.arch.as_deref(), Some("all"));
        assert_eq!(manifest.package.description, "untrusted inspection");
    }
}
