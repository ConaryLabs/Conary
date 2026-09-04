// conary-core/src/repository/static_repo/format/tests.rs

use super::{PackageKeysFile, RepoIdentity, SCHEMA_VERSION, StaticIndex};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};

const VALID_ROOT_KEY_ID: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
const VALID_SHA256: &str = "30e14955ebf1352266dc2ff8067e68104607e750abb9d3b36582b8af909fcb58";

#[test]
fn repo_identity_rejects_bad_name() {
    let input = format!(
        r#"
schema = {SCHEMA_VERSION}
[repo]
name = "Bad_Name"
description = "bad"
[trust]
root_key_ids = ["{VALID_ROOT_KEY_ID}"]
"#
    );
    assert!(RepoIdentity::parse(&input).is_err());
}

#[test]
fn repo_identity_rejects_bad_root_key_id() {
    let input = format!(
        r#"
schema = {SCHEMA_VERSION}
[repo]
name = "good-name"
[trust]
root_key_ids = ["not-a-key"]
"#
    );
    assert!(RepoIdentity::parse(&input).is_err());
}

#[test]
fn repo_identity_rejects_unknown_schema() {
    // Both directions: the retired major and any future major.
    for schema in [SCHEMA_VERSION - 1, SCHEMA_VERSION + 1] {
        let input = format!(
            r#"
schema = {schema}
[repo]
name = "good-name"
[trust]
root_key_ids = ["{VALID_ROOT_KEY_ID}"]
"#
        );
        let error = RepoIdentity::parse(&input).unwrap_err();
        assert!(
            error.to_string().contains(&format!(
                "repo identity schema {schema} is unsupported; expected {SCHEMA_VERSION}"
            )),
            "{error}"
        );
    }
}

#[test]
fn static_index_rejects_unknown_schema() {
    for schema in [SCHEMA_VERSION - 1, SCHEMA_VERSION + 1] {
        let input = valid_index_json(
            schema,
            "packages/acme-widget/acme-widget-1.4.2-1-x86_64.ccs",
            1048576,
        );
        let error = StaticIndex::parse(&input).unwrap_err();
        assert!(
            error.to_string().contains(&format!(
                "static index schema {schema} is unsupported; expected {SCHEMA_VERSION}"
            )),
            "{error}"
        );
    }
}

/// A retired-major index carrying the retired provenance shape must be
/// refused by the version gate, not deserialized into the current shape
/// with its duplicated path silently dropped.
#[test]
fn a_retired_major_index_carrying_a_duplicated_provenance_path_is_refused() {
    // Major 1 is named absolutely, not as `SCHEMA_VERSION - 1`: it is the
    // exact published major whose provenance carried a duplicated path.
    // A relative bound would follow the constant back down and prove
    // nothing about whether the major was actually raised for that change.
    const RETIRED_DUPLICATED_PATH_MAJOR: u64 = 1;

    let input = valid_index_json(
        RETIRED_DUPLICATED_PATH_MAJOR,
        "packages/acme-widget/acme-widget-1.4.2-1-x86_64.ccs",
        1048576,
    )
    .replace(
        r#"{ "role": "exact-identity" }"#,
        r#"{ "role": "source-derived-file", "format": "ccs", "source_path": "/usr/bin/widget" }"#,
    );
    let error = StaticIndex::parse(&input).unwrap_err();
    assert!(
            error.to_string().contains(&format!(
                "static index schema {RETIRED_DUPLICATED_PATH_MAJOR} is unsupported; expected {SCHEMA_VERSION}"
            )),
            "{error}"
        );

    // The same retired major is refused for the identity and key documents,
    // so no static document can be read under the old provenance shape.
    let identity = format!(
        r#"
schema = {RETIRED_DUPLICATED_PATH_MAJOR}
[repo]
name = "acme-tools"
[trust]
root_key_ids = ["{VALID_ROOT_KEY_ID}"]
"#
    );
    assert!(RepoIdentity::parse(&identity).is_err());
    assert!(
        PackageKeysFile::parse(&valid_package_keys_json(
            RETIRED_DUPLICATED_PATH_MAJOR,
            &BASE64.encode([1_u8; 32])
        ))
        .is_err()
    );
}

/// The publisher stamps the constant, so a published document and the
/// parser that admits it cannot drift apart.
#[test]
fn every_published_document_stamps_the_current_major() {
    let index = StaticIndex::parse(&valid_index_json(
        SCHEMA_VERSION,
        "packages/acme-widget/acme-widget-1.4.2-1-x86_64.ccs",
        1048576,
    ))
    .unwrap();
    assert_eq!(index.schema, SCHEMA_VERSION);

    let keys = PackageKeysFile::parse(&valid_package_keys_json(
        SCHEMA_VERSION,
        &BASE64.encode([1_u8; 32]),
    ))
    .unwrap();
    assert_eq!(keys.schema, SCHEMA_VERSION);
}

#[test]
fn static_index_rejects_package_filename_mismatch() {
    let input = valid_index_json(
        SCHEMA_VERSION,
        "packages/acme-widget/wrong-name-1.4.2-1-x86_64.ccs",
        1048576,
    );
    assert!(StaticIndex::parse(&input).is_err());
}

#[test]
fn static_index_rejects_package_path_outside_name_directory() {
    let input = valid_index_json(
        SCHEMA_VERSION,
        "packages/other/acme-widget-1.4.2-1-x86_64.ccs",
        1048576,
    );
    assert!(StaticIndex::parse(&input).is_err());
}

#[test]
fn static_index_rejects_package_size_above_i64_max() {
    let input = valid_index_json(
        SCHEMA_VERSION,
        "packages/acme-widget/acme-widget-1.4.2-1-x86_64.ccs",
        i64::MAX as u64 + 1,
    );
    assert!(StaticIndex::parse(&input).is_err());
}

#[test]
fn static_index_rejects_duplicate_package_identity() {
    let input = format!(
        r#"{{
  "schema": {SCHEMA_VERSION},
  "name": "acme-tools",
  "index_version": 7,
  "generated": "2026-06-10T18:00:00Z",
  "packages": [
    {{
      "name": "acme-widget",
      "version": "1.4.2",
      "version_scheme": "conary",
      "release": "1",
      "arch": "x86_64",
      "path": "packages/acme-widget/acme-widget-1.4.2-1-x86_64.ccs",
      "sha256": "{VALID_SHA256}",
      "size": 1048576,
      "provides": [{{
        "name": "acme-widget",
        "kind": "PackageName",
        "version": "1.4.2",
        "version_relation": "equal",
        "architecture_qualifier": {{ "kind": "implicit" }},
        "native_text": null,
        "provenance": {{ "role": "exact-identity" }}
      }}],
      "requirements": [],

    "relations": []
    }},
    {{
      "name": "acme-widget",
      "version": "1.4.2",
      "version_scheme": "conary",
      "release": "1",
      "arch": "x86_64",
      "path": "packages/acme-widget/acme-widget-1.4.2-1-x86_64.ccs",
      "sha256": "{VALID_SHA256}",
      "size": 1048576,
      "provides": [{{
        "name": "acme-widget",
        "kind": "PackageName",
        "version": "1.4.2",
        "version_relation": "equal",
        "architecture_qualifier": {{ "kind": "implicit" }},
        "native_text": null,
        "provenance": {{ "role": "exact-identity" }}
      }}],
      "requirements": [],

    "relations": []
    }}
  ]
}}"#
    );
    assert!(StaticIndex::parse(&input).is_err());
}

#[test]
fn static_index_rejects_string_dependency_contract() {
    let mut value: serde_json::Value = serde_json::from_str(&valid_index_json(
        SCHEMA_VERSION,
        "packages/acme-widget/acme-widget-1.4.2-1-x86_64.ccs",
        1048576,
    ))
    .unwrap();
    let package = value["packages"][0].as_object_mut().unwrap();
    package.remove("provides");
    package.remove("requirements");
    package.insert(
        "dependencies".to_string(),
        serde_json::json!(["libfoo >= 2.0"]),
    );

    let error = StaticIndex::parse(&serde_json::to_string(&value).unwrap()).unwrap_err();
    assert!(error.to_string().contains("provides"), "{error}");
}

#[test]
fn static_index_preserves_same_name_compatibility_capability() {
    let mut value: serde_json::Value = serde_json::from_str(&valid_index_json(
        SCHEMA_VERSION,
        "packages/acme-widget/acme-widget-1.4.2-1-x86_64.ccs",
        1048576,
    ))
    .unwrap();
    value["packages"][0]["version_scheme"] = serde_json::json!("arch");
    value["packages"][0]["provides"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "name": "acme-widget",
            "kind": "PackageName",
            "version": "1.4",
            "version_relation": "equal",
            "architecture_qualifier": { "kind": "implicit" },
            "native_text": "acme-widget=1.4",
            "provenance": {
                "role": "source-declared",
                "format": "alpm",
                "record_index": 0
            }
        }));

    let index = StaticIndex::parse(&serde_json::to_string(&value).unwrap()).unwrap();

    assert_eq!(index.packages[0].provides.len(), 2);
    assert_eq!(
        index.packages[0].provides[1].version.as_deref(),
        Some("1.4")
    );
}

#[test]
fn static_index_rejects_negative_requirement_groups() {
    let mut value: serde_json::Value = serde_json::from_str(&valid_index_json(
        SCHEMA_VERSION,
        "packages/acme-widget/acme-widget-1.4.2-1-x86_64.ccs",
        1048576,
    ))
    .unwrap();
    value["packages"][0]["requirements"] = serde_json::json!([{
        "kind": "Conflict",
        "behavior": "Hard",
        "expression": {
            "operator": "atom",
            "operands": {
                "name": "incompatible-widget",
                "capability_kind": null,
                "version_constraint": null,
                "native_text": null
            }
        },
        "alternatives": [{
            "name": "incompatible-widget",
            "capability_kind": null,
            "version_constraint": null,
            "native_text": null
        }],
        "description": null,
        "native_text": "incompatible-widget"
    }]);

    let error = StaticIndex::parse(&serde_json::to_string(&value).unwrap()).unwrap_err();
    assert!(
        error.to_string().contains("unsupported negative relation"),
        "{error}"
    );
}

#[test]
fn static_index_round_trips_typed_relation_authority() {
    let mut value: serde_json::Value = serde_json::from_str(&valid_index_json(
        SCHEMA_VERSION,
        "packages/acme-widget/acme-widget-1.4.2-1-x86_64.ccs",
        1048576,
    ))
    .unwrap();
    let relation = crate::repository::package_relation::parse_native_relation(
        crate::repository::dependency_model::RepositoryRequirementKind::Conflict,
        crate::repository::versioning::VersionScheme::Conary,
        "incompatible-widget",
    )
    .unwrap();
    value["packages"][0]["relations"] = serde_json::to_value([&relation]).unwrap();

    let index = StaticIndex::parse(&serde_json::to_string(&value).unwrap()).unwrap();

    assert_eq!(index.packages[0].relations, vec![relation]);
}

#[test]
fn static_index_rejects_malformed_typed_relation_constraint() {
    let mut value: serde_json::Value = serde_json::from_str(&valid_index_json(
        SCHEMA_VERSION,
        "packages/acme-widget/acme-widget-1.4.2-1-x86_64.ccs",
        1048576,
    ))
    .unwrap();
    let mut relation = crate::repository::package_relation::parse_native_relation(
        crate::repository::dependency_model::RepositoryRequirementKind::Replace,
        crate::repository::versioning::VersionScheme::Conary,
        "old-widget<2.0.0",
    )
    .unwrap();
    relation.alternatives[0].version_constraint = Some(">=".to_string());
    if let crate::repository::dependency_model::RepositoryRequirementExpression::Atom(clause) =
        &mut relation.expression
    {
        clause.version_constraint = Some(">=".to_string());
    }
    value["packages"][0]["relations"] = serde_json::to_value([relation]).unwrap();

    let error = StaticIndex::parse(&serde_json::to_string(&value).unwrap()).unwrap_err();

    assert!(error.to_string().contains("invalid"), "{error}");
}

#[test]
fn package_keys_reject_unknown_schema() {
    let input = valid_package_keys_json(SCHEMA_VERSION + 1, &BASE64.encode([0_u8; 32]));
    assert!(PackageKeysFile::parse(&input).is_err());
}

#[test]
fn package_keys_reject_malformed_public_key() {
    let input = valid_package_keys_json(SCHEMA_VERSION, "not base64!");
    assert!(PackageKeysFile::parse(&input).is_err());
}

#[test]
fn package_keys_reject_wrong_public_key_length() {
    let input = valid_package_keys_json(SCHEMA_VERSION, &BASE64.encode([0_u8; 31]));
    assert!(PackageKeysFile::parse(&input).is_err());
}

#[test]
fn package_keys_reject_invalid_ed25519_public_key_bytes() {
    let input = valid_package_keys_json(
        1,
        &BASE64.encode([
            0x43, 0xc3, 0x14, 0x30, 0xfa, 0xa7, 0x7c, 0xac, 0x28, 0x60, 0x59, 0x5d, 0x4c, 0xf0,
            0x25, 0x69, 0x2d, 0x65, 0x12, 0x36, 0xec, 0xaf, 0xce, 0xf2, 0xcc, 0xe9, 0x1c, 0xd8,
            0x6e, 0xcf, 0x7d, 0xae,
        ]),
    );
    assert!(PackageKeysFile::parse(&input).is_err());
}

#[test]
fn package_keys_reject_empty_keys_for_non_empty_index() {
    let index = StaticIndex::parse(&valid_index_json(
        SCHEMA_VERSION,
        "packages/acme-widget/acme-widget-1.4.2-1-x86_64.ccs",
        1048576,
    ))
    .unwrap();
    let keys =
        PackageKeysFile::parse(&format!(r#"{{"schema":{SCHEMA_VERSION},"keys":[]}}"#)).unwrap();
    assert!(keys.validate_for_index(&index).is_err());
}

fn valid_index_json(schema: u64, path: &str, size: u64) -> String {
    format!(
        r#"{{
  "schema": {schema},
  "name": "acme-tools",
  "index_version": 7,
  "generated": "2026-06-10T18:00:00Z",
  "packages": [
    {{
      "name": "acme-widget",
      "version": "1.4.2",
      "version_scheme": "conary",
      "release": "1",
      "arch": "x86_64",
      "path": "{path}",
      "sha256": "{VALID_SHA256}",
      "size": {size},
      "provides": [{{
        "name": "acme-widget",
        "kind": "PackageName",
        "version": "1.4.2",
        "version_relation": "equal",
        "architecture_qualifier": {{ "kind": "implicit" }},
        "native_text": null,
        "provenance": {{ "role": "exact-identity" }}
      }}],
      "requirements": [],

    "relations": []
    }}
  ]
}}"#
    )
}

fn valid_package_keys_json(schema: u64, public_key: &str) -> String {
    format!(
        r#"{{
  "schema": {schema},
  "keys": [
    {{
      "algorithm": "ed25519",
      "public_key": "{public_key}",
      "key_id": "publish",
      "status": "active"
    }}
  ]
}}"#
    )
}
