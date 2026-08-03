// conary-core/src/ccs/v3/component_view.rs

//! Component and file views derived from signed CCS v3 authority.
//!
//! The archive used to carry a `components/<name>.json` copy of every file
//! record. That projection duplicated the signed file array in a second
//! encoding, roughly doubling archive metadata, and it could add no authority
//! because verification had to prove it matched the MANIFEST exactly. The
//! signed authority is now the sole owner, and every component view is derived
//! from it here.

use super::schema::{AuthorityDocumentV3, PackageKindV3};
use crate::ccs::builder::{ComponentData, FileEntry};
use std::collections::HashMap;

/// Project every signed payload node into the shared builder file view.
pub fn file_entries(authority: &AuthorityDocumentV3) -> Vec<FileEntry> {
    let PackageKindV3::Package(data) = &authority.kind else {
        return Vec::new();
    };
    data.files
        .iter()
        .map(|file| FileEntry {
            path: file.path.clone(),
            node: file.node.clone(),
            content: file.content.clone(),
            component: file.component.clone(),
            chunks: None,
        })
        .collect()
}

/// Derive the component view from signed authority.
///
/// Every component named in signed authority appears, including one that owns
/// no payload node, and each file lands in exactly the component its signed
/// record names.
pub fn components(authority: &AuthorityDocumentV3) -> HashMap<String, ComponentData> {
    let mut components = authority
        .components
        .keys()
        .map(|name| {
            (
                name.clone(),
                ComponentData {
                    name: name.clone(),
                    files: Vec::new(),
                    hash: String::new(),
                    size: 0,
                },
            )
        })
        .collect::<HashMap<String, ComponentData>>();

    for file in file_entries(authority) {
        let component = components
            .entry(file.component.clone())
            .or_insert_with(|| ComponentData {
                name: file.component.clone(),
                files: Vec::new(),
                hash: String::new(),
                size: 0,
            });
        component.size += file.content.as_ref().map_or(0, |content| content.size);
        component.files.push(file);
    }

    for component in components.values_mut() {
        component.hash = component_hash(&component.files);
    }
    components
}

/// Stable content hash of one component's derived file view.
///
/// This is the same canonical-JSON digest the deleted `components/<name>.json`
/// projection carried, so inspection output did not change when the duplicated
/// projection was removed from the archive grammar.
pub fn component_hash(files: &[FileEntry]) -> String {
    crate::hash::sha256_prefixed(
        &crate::ccs::attestation::canonical_json_bytes(&files.to_vec())
            .expect("FileEntry carries only serializable authority"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::v3::schema::ComponentAuthorityV3;

    #[test]
    fn derived_components_cover_signed_names_including_empty_ones() {
        let mut authority = AuthorityDocumentV3::package_for_tests("derived");
        authority.components.insert(
            "docs".to_string(),
            ComponentAuthorityV3 {
                name: "docs".to_string(),
                default: false,
                file_count: 0,
                total_size: 0,
            },
        );

        let components = components(&authority);

        assert_eq!(components.len(), 2);
        assert!(components["docs"].files.is_empty());
        assert_eq!(components["main"].files.len(), 1);
        assert_eq!(components["main"].size, 12);
        assert!(components["main"].hash.starts_with("sha256:"));
        assert!(components["docs"].hash.starts_with("sha256:"));
    }

    #[test]
    fn derived_file_view_preserves_exact_signed_payload_authority() {
        let authority = AuthorityDocumentV3::package_for_tests("derived-files");
        let PackageKindV3::Package(data) = &authority.kind else {
            panic!("fixture should be a package");
        };

        let files = file_entries(&authority);

        assert_eq!(files.len(), data.files.len());
        assert_eq!(files[0].path, data.files[0].path);
        assert_eq!(files[0].node, data.files[0].node);
        assert_eq!(files[0].content, data.files[0].content);
        assert_eq!(files[0].component, data.files[0].component);
    }
}
