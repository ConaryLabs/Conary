// conary-core/src/ccs/budget/tests.rs

use super::*;
use crate::ccs::manifest::FileCapability;
use crate::ccs::v3::schema::{
    ComponentAuthorityV3, ConfigSemanticsV3, ConflictPolicyV3, FileContentLayoutV3,
    LifecycleScriptExecutionV3, LifecycleScriptV3, PackageDataV3,
};
use crate::payload::{PayloadContentAuthority, PayloadIdentity, PayloadNode, PayloadTimestamp};
use std::collections::BTreeMap;

fn package_data(authority: &mut AuthorityDocumentV3) -> &mut PackageDataV3 {
    let PackageKindV3::Package(data) = &mut authority.kind else {
        panic!("fixture must be a package");
    };
    data
}

fn file_at(path: &str) -> FileAuthorityV3 {
    FileAuthorityV3 {
        path: path.to_string(),
        node: PayloadNode::regular(0o644),
        content: Some(PayloadContentAuthority {
            sha256: crate::hash::sha256(path.as_bytes()),
            size: 4096,
        }),
        content_layout: FileContentLayoutV3::WholeObject,
        component: "main".to_string(),
        config: None,
        conflict: ConflictPolicyV3::Error,
    }
}

fn with_files(name: &str, count: usize, path_of: impl Fn(usize) -> String) -> AuthorityDocumentV3 {
    let mut authority = AuthorityDocumentV3::empty_package_for_tests(name);
    authority.components.insert(
        "main".to_string(),
        ComponentAuthorityV3 {
            name: "main".to_string(),
            default: true,
            file_count: count as u32,
            total_size: 0,
        },
    );
    let files = (0..count).map(|index| file_at(&path_of(index))).collect();
    package_data(&mut authority).files = files;
    authority
}

#[test]
fn a_package_far_past_the_retired_sixteen_mebibyte_ceiling_is_admissible() {
    // The retired fixed ceiling refused an ordinary package at roughly 52,000
    // payload nodes. Structural budgeting admits it, and the encoded document
    // fits the ceiling its own census derives.
    let authority = with_files("large", 80_000, |index| {
        format!(
            "/usr/lib/python3.14/site-packages/large_collection/plugins/modules/module_{index:06}.py"
        )
    });

    let census = CCS_BUDGET.admit_authority(&authority).unwrap();
    let encoded = authority.to_cbor().unwrap();

    assert_eq!(census.files, 80_000);
    assert!(
        encoded.len() as u64 > 16 * 1024 * 1024,
        "fixture must exercise the retired ceiling: {} bytes",
        encoded.len()
    );
    CCS_BUDGET
        .admit_encoded_authority(&census, encoded.len() as u64)
        .unwrap();
}

#[test]
fn file_authority_fixed_bytes_is_an_upper_bound() {
    // Every node variant, with its variable-length authority accounted for
    // separately, must fit the constant the derived ceiling multiplies.
    let variants = [
        crate::payload::PayloadNodeKind::Regular {
            hardlink_identity: None,
        },
        crate::payload::PayloadNodeKind::Directory,
        crate::payload::PayloadNodeKind::Symlink {
            target: String::new(),
        },
        crate::payload::PayloadNodeKind::Hardlink {
            target: String::new(),
            identity: String::new(),
        },
        crate::payload::PayloadNodeKind::BlockDevice {
            major: u64::MAX,
            minor: u64::MAX,
        },
        crate::payload::PayloadNodeKind::CharacterDevice {
            major: u64::MAX,
            minor: u64::MAX,
        },
        crate::payload::PayloadNodeKind::Fifo,
        crate::payload::PayloadNodeKind::Socket,
    ];

    for kind in variants {
        let file = FileAuthorityV3 {
            path: String::new(),
            node: PayloadNode {
                kind,
                mode: u32::MAX,
                user: PayloadIdentity::Numeric { id: u64::MAX },
                group: PayloadIdentity::Numeric { id: u64::MAX },
                mtime: PayloadTimestamp {
                    seconds: i64::MIN,
                    nanoseconds: u32::MAX,
                },
                xattrs: BTreeMap::new(),
            },
            content: Some(PayloadContentAuthority {
                sha256: "f".repeat(64),
                size: u64::MAX,
            }),
            content_layout: FileContentLayoutV3::WholeObject,
            component: String::new(),
            config: Some(ConfigSemanticsV3 {
                noreplace: true,
                ghost: true,
                remove_on_upgrade: true,
            }),
            conflict: ConflictPolicyV3::Replace,
        };
        let encoded = encoded_len(&file, "file").unwrap();
        assert!(
            encoded <= FILE_AUTHORITY_FIXED_BYTES,
            "{encoded} exceeds the fixed per-file allowance"
        );
    }
}

#[test]
fn every_structural_dimension_fails_one_over_its_own_limit() {
    let budget = CCS_BUDGET;

    // File count.
    let authority = with_files("files", (budget.max_files + 1) as usize, |index| {
        format!("/f{index}")
    });
    // Building a real 1M-file fixture is wasteful; assert against the
    // dimension directly for the count-only limits.
    drop(authority);
    assert_eq!(
        admit(
            BudgetDimension::FileCount,
            "kind.package.files",
            budget.max_files + 1,
            budget.max_files
        )
        .unwrap_err()
        .dimension,
        BudgetDimension::FileCount
    );
    assert_eq!(
        admit(
            BudgetDimension::PayloadReferenceCount,
            "kind.package.files.content_layout",
            budget.max_payload_references() + 1,
            budget.max_payload_references(),
        )
        .unwrap_err()
        .dimension,
        BudgetDimension::PayloadReferenceCount
    );

    // Path length.
    let mut authority = with_files("path", 1, |_| {
        format!("/{}", "a".repeat(CCS_BUDGET.max_path_bytes as usize))
    });
    assert_eq!(
        CCS_BUDGET
            .admit_authority(&authority)
            .unwrap_err()
            .dimension,
        BudgetDimension::PathBytes
    );

    // Path depth.
    authority = with_files("depth", 1, |_| {
        "/a".repeat(CCS_BUDGET.max_path_components as usize + 1)
    });
    assert_eq!(
        CCS_BUDGET
            .admit_authority(&authority)
            .unwrap_err()
            .dimension,
        BudgetDimension::PathDepth
    );

    // Component name length.
    authority = with_files("component", 1, |index| format!("/f{index}"));
    package_data(&mut authority).files[0].component =
        "c".repeat(CCS_BUDGET.max_name_bytes as usize + 1);
    assert_eq!(
        CCS_BUDGET
            .admit_authority(&authority)
            .unwrap_err()
            .dimension,
        BudgetDimension::NameBytes
    );

    // Symlink target length.
    authority = with_files("link", 1, |index| format!("/f{index}"));
    package_data(&mut authority).files[0].node.kind = crate::payload::PayloadNodeKind::Symlink {
        target: "t".repeat(CCS_BUDGET.max_link_target_bytes as usize + 1),
    };
    assert_eq!(
        CCS_BUDGET
            .admit_authority(&authority)
            .unwrap_err()
            .dimension,
        BudgetDimension::LinkTargetBytes
    );

    // Xattr count, name, and value.
    authority = with_files("xattr", 1, |index| format!("/f{index}"));
    let xattrs = &mut package_data(&mut authority).files[0].node.xattrs;
    for index in 0..=CCS_BUDGET.max_xattrs_per_node {
        xattrs.insert(format!("user.x{index}"), vec![0_u8; 4]);
    }
    assert_eq!(
        CCS_BUDGET
            .admit_authority(&authority)
            .unwrap_err()
            .dimension,
        BudgetDimension::XattrCount
    );

    authority = with_files("xattr-name", 1, |index| format!("/f{index}"));
    package_data(&mut authority).files[0].node.xattrs.insert(
        "n".repeat(CCS_BUDGET.max_xattr_name_bytes as usize + 1),
        vec![0_u8; 4],
    );
    assert_eq!(
        CCS_BUDGET
            .admit_authority(&authority)
            .unwrap_err()
            .dimension,
        BudgetDimension::XattrNameBytes
    );

    authority = with_files("xattr-value", 1, |index| format!("/f{index}"));
    package_data(&mut authority).files[0].node.xattrs.insert(
        "user.big".to_string(),
        vec![0_u8; CCS_BUDGET.max_xattr_value_bytes as usize + 1],
    );
    assert_eq!(
        CCS_BUDGET
            .admit_authority(&authority)
            .unwrap_err()
            .dimension,
        BudgetDimension::XattrValueBytes
    );

    // Component count.
    authority = with_files("components", 1, |index| format!("/f{index}"));
    for index in 0..=CCS_BUDGET.max_components {
        authority.components.insert(
            format!("c{index}"),
            ComponentAuthorityV3 {
                name: format!("c{index}"),
                default: false,
                file_count: 0,
                total_size: 0,
            },
        );
    }
    assert_eq!(
        CCS_BUDGET
            .admit_authority(&authority)
            .unwrap_err()
            .dimension,
        BudgetDimension::ComponentCount
    );

    // Payload object size.
    authority = with_files("object", 1, |index| format!("/f{index}"));
    package_data(&mut authority).files[0]
        .content
        .as_mut()
        .unwrap()
        .size = CCS_BUDGET.max_payload_object_bytes + 1;
    assert_eq!(
        CCS_BUDGET
            .admit_authority(&authority)
            .unwrap_err()
            .dimension,
        BudgetDimension::PayloadObjectBytes
    );

    // Lifecycle script body.
    authority = with_files("script", 1, |index| format!("/f{index}"));
    authority.lifecycle.post_install = Some(LifecycleScriptV3 {
        interpreter: "/bin/sh".to_string(),
        body: "x".repeat(CCS_BUDGET.max_script_body_bytes as usize + 1),
        capabilities: Vec::new(),
        reversible: None,
        execution: LifecycleScriptExecutionV3::SandboxedTargetRoot,
    });
    assert_eq!(
        CCS_BUDGET
            .admit_authority(&authority)
            .unwrap_err()
            .dimension,
        BudgetDimension::ScriptBodyBytes
    );
}

#[test]
fn near_limit_values_on_each_dimension_stay_admissible() {
    let mut authority = with_files("near-limit", 4, |index| format!("/near/{index}"));
    package_data(&mut authority).files[0].path =
        format!("/{}", "a".repeat(CCS_BUDGET.max_path_bytes as usize - 1));
    package_data(&mut authority).files[1].node.kind = crate::payload::PayloadNodeKind::Symlink {
        target: "t".repeat(CCS_BUDGET.max_link_target_bytes as usize),
    };
    package_data(&mut authority).files[1].content = None;
    let widest_component = "c".repeat(CCS_BUDGET.max_name_bytes as usize);
    package_data(&mut authority).files[2].component = widest_component.clone();
    authority.components.insert(
        widest_component.clone(),
        ComponentAuthorityV3 {
            name: widest_component,
            default: false,
            file_count: 1,
            total_size: 4096,
        },
    );
    let xattrs = &mut package_data(&mut authority).files[3].node.xattrs;
    for index in 0..CCS_BUDGET.max_xattrs_per_node {
        xattrs.insert(format!("user.x{index}"), vec![0_u8; 8]);
    }
    package_data(&mut authority).config = vec![
        crate::packages::config_authority::SourceConfigDeclaration::Ccs(
            crate::packages::config_authority::CcsConfigDeclaration {
                path: "/etc/near-limit.conf".to_string(),
                noreplace: true,
                payload: crate::packages::config_authority::ConfigPayloadAssociation::Matched,
            },
        ),
    ];

    let census = CCS_BUDGET.admit_authority(&authority).unwrap();

    assert_eq!(census.files, 4);
    assert_eq!(census.config_entries, 1);
    CCS_BUDGET
        .admit_encoded_authority(&census, authority.to_cbor().unwrap().len() as u64)
        .unwrap();
}

#[test]
fn signed_file_capabilities_are_structurally_budgeted() {
    let mut authority = AuthorityDocumentV3::package_for_tests("file-capable-budget");
    authority.file_capabilities = vec![FileCapability {
        path: "/usr/bin/hello".to_string(),
        capabilities: vec!["cap_net_bind_service".to_string()],
        permitted: true,
        effective: true,
        inheritable: false,
    }];

    let census = CCS_BUDGET.admit_authority(&authority).unwrap();

    assert_eq!(census.file_capabilities, 1);
    CCS_BUDGET
        .admit_encoded_authority(&census, authority.to_cbor().unwrap().len() as u64)
        .unwrap();

    let count_budget = CcsStructuralBudget {
        max_file_capabilities: 0,
        ..CCS_BUDGET
    };
    assert_eq!(
        count_budget
            .admit_authority(&authority)
            .unwrap_err()
            .dimension,
        BudgetDimension::FileCapabilityCount
    );

    let name_budget = CcsStructuralBudget {
        max_file_capability_names: 0,
        ..CCS_BUDGET
    };
    assert_eq!(
        name_budget
            .admit_authority(&authority)
            .unwrap_err()
            .dimension,
        BudgetDimension::FileCapabilityNameCount
    );
}

#[test]
fn hostile_cbor_depth_and_length_declarations_fail_before_allocation() {
    // A declared nesting depth beyond the bound is refused while decoding the
    // header, not after descending it.
    // Each 0x81 is a one-element array header, so a run of them declares
    // nesting deeper than the bound allows.
    let mut deep = vec![0x81u8; CCS_BUDGET.max_cbor_nesting_depth + 8];
    deep.push(0x00);
    let error = CCS_BUDGET.decode_authority(&deep).unwrap_err();
    assert!(format!("{error:#}").contains("MANIFEST"), "{error:#}");

    // A hostile declared length is refused before any read.
    let error = CCS_BUDGET
        .decode_authority(&[])
        .expect_err("empty authority is not decodable");
    assert!(format!("{error:#}").contains("MANIFEST"), "{error:#}");

    let over = CCS_BUDGET.max_authority_bytes() + 1;
    let error = CCS_BUDGET
        .admit_control_bytes(
            BudgetDimension::AuthorityBytes,
            "MANIFEST",
            over,
            CCS_BUDGET.max_authority_bytes(),
        )
        .unwrap_err();
    assert_eq!(error.observed, over);
}

#[test]
fn padded_authority_is_refused_against_its_own_census_ceiling() {
    // A document may not be larger than the structure it declares, so an
    // attacker cannot pad a ten-file package up to the global ceiling.
    let authority = with_files("padded", 10, |index| format!("/usr/bin/f{index}"));
    let census = CCS_BUDGET.admit_authority(&authority).unwrap();
    let ceiling = CCS_BUDGET.authority_bytes_ceiling(&census).unwrap();

    assert!(ceiling < CCS_BUDGET.max_authority_bytes());
    let error = CCS_BUDGET
        .admit_encoded_authority(&census, ceiling + 1)
        .unwrap_err();
    assert_eq!(error.dimension, BudgetDimension::AuthorityBytes);
}

#[test]
fn arithmetic_overflow_fails_closed() {
    let mut authority = with_files("overflow", 2, |index| format!("/f{index}"));
    let files = &mut package_data(&mut authority).files;
    files[0].content.as_mut().unwrap().size = u64::MAX;
    files[1].content.as_mut().unwrap().size = u64::MAX;

    let error = CCS_BUDGET.admit_authority(&authority).unwrap_err();

    assert_eq!(error.dimension, BudgetDimension::PayloadObjectBytes);
    assert_eq!(
        sum("probe", [u64::MAX, 1]).unwrap_err().dimension,
        BudgetDimension::Arithmetic
    );
    assert_eq!(
        product("probe", u64::MAX, 2).unwrap_err().dimension,
        BudgetDimension::Arithmetic
    );
}

#[test]
fn derived_ceilings_are_stated_in_terms_of_structural_dimensions() {
    let budget = CCS_BUDGET;
    let expected = AUTHORITY_ENVELOPE_BYTES
        + budget.max_files * FILE_AUTHORITY_FIXED_BYTES
        + budget.max_payload_references() * PAYLOAD_OBJECT_REFERENCE_BYTES
        + budget.max_total_path_bytes
        + budget.max_total_xattr_bytes
        + budget.max_non_payload_authority_bytes;

    assert_eq!(budget.max_authority_bytes(), expected);
    assert_eq!(
        budget.max_archive_entries(),
        budget.max_payload_objects() + 256 + 1 + 8
    );

    let archive = budget.archive_decode_bounds().unwrap();
    assert_eq!(
        archive.max_payload_object_bytes,
        archive.max_total_payload_bytes
    );
    assert_eq!(archive.max_metadata_bytes, 512 * 1024 * 1024);
    assert_eq!(
        archive.max_decompressed_stream_bytes,
        archive.max_total_payload_bytes
            + archive.max_archive_entries * ARCHIVE_ENTRY_ENVELOPE_BYTES
    );
}

#[test]
fn archive_decode_dimensions_fail_one_over_with_typed_diagnostics() {
    let bounds = CCS_BUDGET.archive_decode_bounds().unwrap();

    let entry_error = bounds
        .admit_archive_entry("test archive entries", bounds.max_archive_entries + 1)
        .unwrap_err();
    assert_eq!(entry_error.dimension, BudgetDimension::ArchiveEntryCount);
    assert_eq!(entry_error.field, "test archive entries");

    let object_error = bounds
        .admit_payload_object("test payload object", bounds.max_payload_object_bytes + 1)
        .unwrap_err();
    assert_eq!(object_error.dimension, BudgetDimension::PayloadObjectBytes);

    let total_error = bounds
        .add_payload_bytes("test cumulative payload", bounds.max_total_payload_bytes, 1)
        .unwrap_err();
    assert_eq!(total_error.dimension, BudgetDimension::TotalPayloadBytes);

    let metadata_error = bounds
        .admit_metadata_bytes("test metadata", bounds.max_metadata_bytes + 1)
        .unwrap_err();
    assert_eq!(metadata_error.dimension, BudgetDimension::MetadataBytes);

    let stream_error = bounds
        .admit_decompressed_stream_bytes(
            "test decompressed stream",
            bounds.max_decompressed_stream_bytes + 1,
        )
        .unwrap_err();
    assert_eq!(
        stream_error.dimension,
        BudgetDimension::DecompressedStreamBytes
    );

    let spool_error = bounds
        .admit_spool_reservation("test payload spool", 1025, 1024)
        .unwrap_err();
    assert_eq!(spool_error.dimension, BudgetDimension::TotalPayloadBytes);
    assert_eq!(spool_error.observed, 1025);
    assert_eq!(spool_error.limit, 1024);
}

#[test]
fn archive_decode_dimensions_admit_their_exact_limits() {
    let bounds = CCS_BUDGET.archive_decode_bounds().unwrap();

    bounds
        .admit_archive_entry("near-limit archive", bounds.max_archive_entries)
        .unwrap();
    assert_eq!(
        bounds
            .add_payload_bytes("near-limit payload", 0, bounds.max_total_payload_bytes,)
            .unwrap(),
        bounds.max_total_payload_bytes
    );
    bounds
        .admit_metadata_bytes("near-limit metadata", bounds.max_metadata_bytes)
        .unwrap();
    bounds
        .admit_decompressed_stream_bytes(
            "near-limit decompressed stream",
            bounds.max_decompressed_stream_bytes,
        )
        .unwrap();
    bounds
        .admit_spool_reservation(
            "near-limit spool",
            bounds.max_total_payload_bytes,
            bounds.max_total_payload_bytes,
        )
        .unwrap();
}

#[test]
fn archive_decode_derivation_overflow_is_typed() {
    let budget = CcsStructuralBudget {
        max_files: u64::MAX,
        ..CCS_BUDGET
    };
    let error = budget.archive_decode_bounds().unwrap_err();
    assert_eq!(error.dimension, BudgetDimension::Arithmetic);
    assert_eq!(error.field, "archive decompressed stream envelope");
}
