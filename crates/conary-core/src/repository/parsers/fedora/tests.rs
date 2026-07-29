// conary-core/src/repository/parsers/fedora/tests.rs

use super::*;
use crate::repository::dependency_model::{ConditionalRequirementBehavior, ProvideVersionRelation};

fn parser() -> FedoraParser {
    let trust = PreparedOpenPgpTrust::for_test(RepositoryTrustPolicy::Rpm {
        metadata: RpmMetadataAuthority::Metalink {
            url: "https://mirrors.example.test/metalink".to_string(),
        },
        package_keys: vec![crate::repository::OpenPgpTrustRoot {
            url: "https://keys.example.test/rpm.gpg".to_string(),
            fingerprint: "A".repeat(40),
        }],
    });
    FedoraParser::new("x86_64".to_string(), trust).unwrap()
}

fn valid_builder() -> PackageBuilder {
    let mut builder = PackageBuilder::new();
    builder.name = Some("test-package".to_string());
    builder.epoch = Some("1".to_string());
    builder.ver = Some("2.3.4".to_string());
    builder.rel = Some("5.fc44".to_string());
    builder.arch = Some("x86_64".to_string());
    builder.checksum = Some("a".repeat(64));
    builder.checksum_type = Some("sha256".to_string());
    builder.size = Some("1024".to_string());
    builder.location = Some("Packages/t/test-package-2.3.4-5.fc44.x86_64.rpm".to_string());
    builder
}

fn primary_package_with_format(format_body: &str) -> String {
    format!(
        r#"
<metadata xmlns:rpm="http://linux.duke.edu/metadata/rpm">
  <package type="rpm">
    <name>bash</name>
    <arch>x86_64</arch>
    <version epoch="0" ver="5.3.3" rel="2.fc44"/>
    <checksum type="sha256">aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa</checksum>
    <summary>GNU Bourne Again shell</summary>
    <description>GNU Bourne Again shell</description>
    <size package="1024"/>
    <location href="Packages/b/bash-5.3.3-2.fc44.x86_64.rpm"/>
    <format>
      {format_body}
    </format>
  </package>
</metadata>
"#
    )
}

#[test]
fn test_package_builder() {
    let builder = valid_builder();
    let pkg = builder.build("https://example.com").unwrap();
    assert_eq!(pkg.name, "test-package");
    assert_eq!(pkg.version, "1:2.3.4-5.fc44");
    assert_eq!(pkg.size, 1024);
}

#[test]
fn test_package_builder_rejects_path_traversal() {
    let mut builder = valid_builder();
    builder.location = Some("../../../etc/passwd".to_string());
    let result = builder.build("https://example.com");
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("path traversal"));
}

#[test]
fn test_package_builder_rejects_absolute_path() {
    let mut builder = valid_builder();
    builder.location = Some("/etc/passwd".to_string());
    let result = builder.build("https://example.com");
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not relative"));
}

#[test]
fn test_package_builder_rejects_url_scheme() {
    let mut builder = valid_builder();
    builder.location = Some("https://evil.com/malware.rpm".to_string());
    let result = builder.build("https://example.com");
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not relative"));
}

#[test]
fn test_package_builder_rejects_oversized() {
    let mut builder = valid_builder();
    builder.size = Some("10000000000000".to_string()); // 10TB
    let result = builder.build("https://example.com");
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("exceeds maximum"));
}

#[test]
fn test_parse_primary_xml_captures_namespaced_requires_and_provides() {
    let parser = parser();
    let xml = r#"
<metadata xmlns:rpm="http://linux.duke.edu/metadata/rpm">
  <package type="rpm">
    <name>kernel-core</name>
    <arch>x86_64</arch>
    <version epoch="0" ver="6.19.6" rel="200.fc44"/>
    <checksum type="sha256">dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd</checksum>
    <summary>kernel core</summary>
    <description>kernel core</description>
    <size package="123"/>
    <location href="Packages/k/kernel-core-6.19.6-200.fc44.x86_64.rpm"/>
    <format>
      <rpm:provides>
        <rpm:entry name="kernel-core-uname-r" flags="EQ" epoch="2" ver="6.19.6" rel="200.fc44"/>
        <rpm:entry name="kernel-abi" flags="GE" epoch="1" ver="6.19" rel="4"/>
      </rpm:provides>
      <rpm:requires>
        <rpm:entry name="systemd" flags="GE" epoch="1" ver="255" rel="3"/>
        <rpm:entry name="rpmlib(CompressedFileNames)" flags="LE" epoch="0" ver="3.0.4" rel="1"/>
      </rpm:requires>
    </format>
  </package>
</metadata>
"#;

    let packages = parser
        .parse_primary_xml(xml, "https://example.com")
        .unwrap();
    let pkg = packages.first().unwrap();
    let metadata = pkg.extra_metadata.as_object().unwrap();
    let provides = metadata
        .get("rpm_provides")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|value| value.as_str())
        .collect::<Vec<_>>();
    assert!(provides.contains(&"kernel-core-uname-r = 2:6.19.6-200.fc44"));
    assert!(provides.contains(&"kernel-abi >= 1:6.19-4"));
    let ranged_provide = pkg
        .provides
        .iter()
        .find(|provide| provide.name == "kernel-abi")
        .expect("ranged RPM provide");
    assert_eq!(ranged_provide.version.as_deref(), Some("1:6.19-4"));
    assert_eq!(
        ranged_provide.version_relation,
        Some(ProvideVersionRelation::GreaterOrEqual)
    );
    assert!(
        pkg.requirements
            .iter()
            .any(|requirement| requirement.native_text.as_deref() == Some("systemd >= 1:255-3"))
    );
    assert!(
        pkg.requirements
            .iter()
            .flat_map(|requirement| requirement.expression.atoms())
            .all(|clause| !clause.name.starts_with("rpmlib("))
    );
    assert!(metadata.get("rpm_requires").is_none());
}

#[test]
fn primary_xml_projects_every_generator_selected_file_as_a_typed_provide() {
    // Structure derived from createrepo_c 5cf41fe5d703901d78078ed18c67ab667e446c1a
    // tests/testdata/repodata_snippets/primary_snippet_02.xml. The repository
    // generator owns selection; the parser must preserve every signed record.
    let xml = primary_package_with_format(
        r#"
      <rpm:provides>
        <rpm:entry name="bash" flags="EQ" epoch="0" ver="5.3.3" rel="2.fc44"/>
      </rpm:provides>
      <file>/usr/bin/bash</file>
      <file type="ghost">/usr/bin/sh</file>
      <file type="dir">/usr/share/bash-completion</file>
      <file>/opt/provider&amp;selected</file>
      <file>/usr<![CDATA[/local/bin/cdata]]></file>
      <file>/usr/bin/trailing-space </file>
"#,
    );

    let packages = parser()
        .parse_primary_xml(&xml, "https://example.com")
        .unwrap();
    let files = packages[0]
        .provides
        .iter()
        .filter(|provide| provide.kind == RepositoryCapabilityKind::File)
        .collect::<Vec<_>>();

    assert_eq!(
        files
            .iter()
            .map(|provide| provide.name.as_str())
            .collect::<Vec<_>>(),
        vec![
            "/usr/bin/bash",
            "/usr/bin/sh",
            "/usr/share/bash-completion",
            "/opt/provider&selected",
            "/usr/local/bin/cdata",
            "/usr/bin/trailing-space ",
        ]
    );
    assert!(files.iter().all(|provide| provide.version.is_none()
        && provide.version_relation.is_none()
        && provide.architecture_qualifier
            == crate::repository::dependency_model::ProvideArchitectureQualifier::Implicit));
}

#[test]
fn primary_xml_rejects_ambiguous_file_record_structure() {
    for file_record in [
        "<file>/usr<unexpected/>/bin/bash</file>",
        "<file>/usr<file>/bin/bash</file></file>",
    ] {
        let xml = primary_package_with_format(file_record);
        let error = parser()
            .parse_primary_xml(&xml, "https://example.com")
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("file record cannot contain nested elements"),
            "{file_record}: {error}"
        );
    }
}

#[test]
fn primary_xml_rejects_file_records_without_an_absolute_path() {
    for file_record in ["<file/>", "<file></file>", "<file>usr/bin/bash</file>"] {
        let xml = primary_package_with_format(file_record);
        let error = parser()
            .parse_primary_xml(&xml, "https://example.com")
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("file record must contain an absolute path"),
            "{file_record}: {error}"
        );
    }
}

#[test]
fn primary_xml_rejects_nameless_dependency_entries() {
    let parser = parser();
    let xml = r#"
<metadata xmlns:rpm="http://linux.duke.edu/metadata/rpm">
  <package type="rpm">
    <name>nameless-requirement</name>
    <arch>x86_64</arch>
    <version epoch="0" ver="1" rel="1"/>
    <checksum type="sha256">dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd</checksum>
    <size package="1"/>
    <location href="Packages/n/nameless-requirement.rpm"/>
    <format>
      <rpm:requires>
        <rpm:entry flags="GE" epoch="0" ver="1" rel="1"/>
      </rpm:requires>
    </format>
  </package>
</metadata>
"#;

    let error = parser
        .parse_primary_xml(xml, "https://example.com")
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("requires entry is missing its required name attribute"),
        "{error}"
    );
}

#[test]
fn primary_xml_rejects_unimplemented_rpm_runtime_features() {
    let parser = parser();
    let xml = r#"
<metadata xmlns:rpm="http://linux.duke.edu/metadata/rpm">
  <package type="rpm">
    <name>unsupported-runtime-feature</name>
    <arch>x86_64</arch>
    <version epoch="0" ver="1" rel="1"/>
    <checksum type="sha256">dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd</checksum>
    <size package="1"/>
    <location href="Packages/u/unsupported-runtime-feature.rpm"/>
    <format>
      <rpm:requires>
        <rpm:entry name="rpmlib(ConcurrentAccess)" flags="LE" epoch="0" ver="4.1" rel="1"/>
      </rpm:requires>
    </format>
  </package>
</metadata>
"#;

    let error = parser
        .parse_primary_xml(xml, "https://example.com")
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("does not implement RPM runtime capability"),
        "{error}"
    );
}

#[test]
fn primary_xml_runtime_true_short_circuits_mixed_rich_or_requirement() {
    let parser = parser();
    let xml = r#"
<metadata xmlns:rpm="http://linux.duke.edu/metadata/rpm">
  <package type="rpm">
    <name>mixed-runtime-feature</name>
    <arch>x86_64</arch>
    <version epoch="0" ver="1" rel="1"/>
    <checksum type="sha256">dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd</checksum>
    <size package="1"/>
    <location href="Packages/m/mixed-runtime-feature.rpm"/>
    <format>
      <rpm:requires>
        <rpm:entry name="(missing-provider or rpmlib(CompressedFileNames) &lt;= 3.0.4-1)"/>
      </rpm:requires>
    </format>
  </package>
</metadata>
"#;

    let packages = parser
        .parse_primary_xml(xml, "https://example.com")
        .unwrap();

    assert!(packages[0].requirements.is_empty());
}

#[test]
fn primary_xml_package_cannot_forge_package_manager_runtime_provides() {
    let parser = parser();
    let xml = r#"
<metadata xmlns:rpm="http://linux.duke.edu/metadata/rpm">
  <package type="rpm">
    <name>runtime-forger</name>
    <arch>x86_64</arch>
    <version epoch="0" ver="1" rel="1"/>
    <checksum type="sha256">dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd</checksum>
    <size package="1"/>
    <location href="Packages/r/runtime-forger.rpm"/>
    <format>
      <rpm:provides>
        <rpm:entry name="rpmlib(CompressedFileNames)" flags="EQ" ver="3.0.4" rel="1"/>
      </rpm:provides>
    </format>
  </package>
</metadata>
"#;

    let error = parser
        .parse_primary_xml(xml, "https://example.com")
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("typed runtime ledger is the sole authority"),
        "{error}"
    );
}

#[test]
fn primary_xml_rejects_malformed_version_attribute_encoding() {
    let parser = parser();
    let xml = r#"
<metadata>
  <package type="rpm">
    <name>broken-attribute</name>
    <arch>x86_64</arch>
    <version epoch="0" ver="1&unknown;" rel="1"/>
    <checksum type="sha256">dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd</checksum>
    <size package="1"/>
    <location href="Packages/b/broken-attribute.rpm"/>
  </package>
</metadata>
"#;

    assert!(
        parser
            .parse_primary_xml(xml, "https://example.com")
            .is_err()
    );
}

#[test]
fn primary_xml_rejects_malformed_text_entity() {
    let parser = parser();
    let xml = r#"
<metadata>
  <package type="rpm">
    <name>broken&unknown;</name>
    <arch>x86_64</arch>
    <version epoch="0" ver="1" rel="1"/>
    <checksum type="sha256">dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd</checksum>
    <size package="1"/>
    <location href="Packages/b/broken.rpm"/>
  </package>
</metadata>
"#;

    assert!(
        parser
            .parse_primary_xml(xml, "https://example.com")
            .is_err()
    );
}

#[test]
fn primary_xml_preserves_rpm_conflicts_and_obsoletes() {
    let parser = parser();
    let xml = r#"
<metadata xmlns:rpm="http://linux.duke.edu/metadata/rpm">
  <package type="rpm">
    <name>newpkg</name>
    <arch>x86_64</arch>
    <version epoch="0" ver="2" rel="1.fc44"/>
    <checksum type="sha256">dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd</checksum>
    <summary>new package</summary>
    <description>new package</description>
    <size package="123"/>
    <location href="Packages/n/newpkg-2-1.fc44.x86_64.rpm"/>
    <format>
      <rpm:conflicts>
        <rpm:entry name="old-conflict" flags="LT" ver="2"/>
      </rpm:conflicts>
      <rpm:obsoletes>
        <rpm:entry name="old-owner" flags="LE" ver="1"/>
      </rpm:obsoletes>
    </format>
  </package>
</metadata>
"#;

    let packages = parser
        .parse_primary_xml(xml, "https://example.com")
        .unwrap();

    assert_eq!(
        packages[0]
            .requirements
            .iter()
            .map(|relation| relation.kind)
            .collect::<Vec<_>>(),
        vec![
            RepositoryRequirementKind::Conflict,
            RepositoryRequirementKind::Obsolete,
        ]
    );
    assert_eq!(
        packages[0]
            .requirements
            .iter()
            .map(|relation| relation.native_text.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("old-conflict < 2"), Some("old-owner <= 1")]
    );
}

#[test]
fn primary_xml_rejects_malformed_rpm_relation_constraint() {
    let parser = parser();
    let xml = r#"
<metadata xmlns:rpm="http://linux.duke.edu/metadata/rpm">
  <package type="rpm">
    <name>newpkg</name>
    <arch>x86_64</arch>
    <version epoch="0" ver="2" rel="1.fc44"/>
    <checksum type="sha256">dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd</checksum>
    <summary>new package</summary>
    <description>new package</description>
    <size package="123"/>
    <location href="Packages/n/newpkg-2-1.fc44.x86_64.rpm"/>
    <format>
      <rpm:conflicts>
        <rpm:entry name="oldpkg" flags="GE"/>
      </rpm:conflicts>
    </format>
  </package>
</metadata>
"#;

    let error = parser
        .parse_primary_xml(xml, "https://example.com")
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("comparison flags and version must appear together")
    );
}

#[test]
fn test_builder_sets_source_distro_and_version_scheme() {
    let builder = valid_builder();
    let pkg = builder.build("https://example.com").unwrap();
    assert_eq!(pkg.dependency_flavor, RepositoryDependencyFlavor::Rpm);
    assert_eq!(pkg.version_scheme, VersionScheme::Rpm);
}

#[test]
fn test_structured_kernel_uname_r_provide() {
    let parser = parser();
    let xml = r#"
<metadata xmlns:rpm="http://linux.duke.edu/metadata/rpm">
  <package type="rpm">
    <name>kernel-core</name>
    <arch>x86_64</arch>
    <version epoch="0" ver="6.19.6" rel="200.fc44"/>
    <checksum type="sha256">dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd</checksum>
    <summary>kernel core</summary>
    <description>kernel core</description>
    <size package="123"/>
    <location href="Packages/k/kernel-core-6.19.6-200.fc44.x86_64.rpm"/>
    <format>
      <rpm:provides>
        <rpm:entry name="kernel-core-uname-r" flags="EQ" ver="6.19.6-200.fc44.x86_64"/>
      </rpm:provides>
    </format>
  </package>
</metadata>
"#;

    let packages = parser
        .parse_primary_xml(xml, "https://example.com")
        .unwrap();
    let pkg = &packages[0];

    // Should have self-provide + kernel-core-uname-r
    assert!(pkg.provides.len() >= 2);

    let uname_provide = pkg
        .provides
        .iter()
        .find(|p| p.name == "kernel-core-uname-r")
        .expect("kernel-core-uname-r provide not found");
    assert_eq!(uname_provide.kind, RepositoryCapabilityKind::Generic);
    assert_eq!(
        uname_provide.version.as_deref(),
        Some("6.19.6-200.fc44.x86_64")
    );
    assert_eq!(
        uname_provide.native_text.as_deref(),
        Some("kernel-core-uname-r = 6.19.6-200.fc44.x86_64")
    );
}

#[test]
fn test_structured_versioned_provide_coreutils() {
    let parser = parser();
    let xml = r#"
<metadata xmlns:rpm="http://linux.duke.edu/metadata/rpm">
  <package type="rpm">
    <name>coreutils-common</name>
    <arch>x86_64</arch>
    <version epoch="0" ver="9.7" rel="1.fc44"/>
    <checksum type="sha256">aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa</checksum>
    <summary>Common files for coreutils</summary>
    <description>Common files</description>
    <size package="456"/>
    <location href="Packages/c/coreutils-common-9.7-1.fc44.x86_64.rpm"/>
    <format>
      <rpm:provides>
        <rpm:entry name="coreutils-common" flags="EQ" ver="9.7"/>
      </rpm:provides>
    </format>
  </package>
</metadata>
"#;

    let packages = parser
        .parse_primary_xml(xml, "https://example.com")
        .unwrap();
    let pkg = &packages[0];

    // Self-provide from the explicit entry should be PackageName kind
    let self_provide = pkg
        .provides
        .iter()
        .find(|p| p.name == "coreutils-common" && p.native_text.is_some())
        .expect("explicit self-provide not found");
    assert_eq!(self_provide.kind, RepositoryCapabilityKind::PackageName);
    assert_eq!(self_provide.version.as_deref(), Some("9.7"));
}

#[test]
fn test_structured_rich_conditional_dep() {
    let parser = parser();
    let xml = r#"
<metadata xmlns:rpm="http://linux.duke.edu/metadata/rpm">
  <package type="rpm">
    <name>test-pkg</name>
    <arch>x86_64</arch>
    <version epoch="0" ver="1.0" rel="1.fc44"/>
    <checksum type="sha256">bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb</checksum>
    <summary>Test</summary>
    <description>Test</description>
    <size package="100"/>
    <location href="Packages/t/test-pkg-1.0-1.fc44.x86_64.rpm"/>
    <format>
      <rpm:requires>
        <rpm:entry name="(foo if bar)"/>
        <rpm:entry name="systemd" flags="GE" ver="255"/>
      </rpm:requires>
    </format>
  </package>
</metadata>
"#;

    let packages = parser
        .parse_primary_xml(xml, "https://example.com")
        .unwrap();
    let pkg = &packages[0];

    assert_eq!(pkg.requirements.len(), 2);

    // Rich dep should be conditional
    let rich = &pkg.requirements[0];
    assert_eq!(rich.behavior, ConditionalRequirementBehavior::Conditional);
    assert_eq!(
        rich.alternatives
            .iter()
            .map(|clause| clause.name.as_str())
            .collect::<Vec<_>>(),
        vec!["foo", "bar"],
    );
    assert!(matches!(
        rich.expression,
        crate::repository::dependency_model::RepositoryRequirementExpression::If { .. }
    ));

    // Normal dep should be hard
    let normal = &pkg.requirements[1];
    assert_eq!(normal.behavior, ConditionalRequirementBehavior::Hard);
    assert_eq!(normal.alternatives[0].name, "systemd");
    assert_eq!(
        normal.alternatives[0].version_constraint.as_deref(),
        Some(">= 255")
    );
}

#[test]
fn test_structured_soname_provide() {
    let parser = parser();
    let xml = r#"
<metadata xmlns:rpm="http://linux.duke.edu/metadata/rpm">
  <package type="rpm">
    <name>glibc</name>
    <arch>x86_64</arch>
    <version epoch="0" ver="2.40" rel="1.fc44"/>
    <checksum type="sha256">cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc</checksum>
    <summary>glibc</summary>
    <description>glibc</description>
    <size package="200"/>
    <location href="Packages/g/glibc-2.40-1.fc44.x86_64.rpm"/>
    <format>
      <rpm:provides>
        <rpm:entry name="libc.so.6()(64bit)"/>
      </rpm:provides>
    </format>
  </package>
</metadata>
"#;

    let packages = parser
        .parse_primary_xml(xml, "https://example.com")
        .unwrap();
    let pkg = &packages[0];

    let soname = pkg
        .provides
        .iter()
        .find(|p| p.name == "libc.so.6()(64bit)")
        .expect("soname provide not found");
    assert_eq!(soname.kind, RepositoryCapabilityKind::Generic);
    assert!(soname.version.is_none());
}

#[test]
fn test_implicit_self_provide() {
    let builder = valid_builder();
    let pkg = builder.build("https://example.com").unwrap();

    let self_provide = pkg
        .provides
        .iter()
        .find(|p| p.name == "test-package" && p.kind == RepositoryCapabilityKind::PackageName)
        .expect("implicit self-provide not found");
    assert_eq!(self_provide.version.as_deref(), Some("1:2.3.4-5.fc44"));
}

#[test]
fn test_rich_dep_or_parsed_into_alternatives() {
    let parser = parser();
    let xml = r#"
<metadata xmlns:rpm="http://linux.duke.edu/metadata/rpm">
  <package type="rpm">
    <name>test-pkg</name>
    <arch>x86_64</arch>
    <version epoch="0" ver="1.0" rel="1.fc44"/>
    <checksum type="sha256">bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb</checksum>
    <summary>Test</summary>
    <description>Test</description>
    <size package="100"/>
    <location href="Packages/t/test-pkg-1.0-1.fc44.x86_64.rpm"/>
    <format>
      <rpm:requires>
        <rpm:entry name="(foo or bar)"/>
        <rpm:entry name="(foo if bar)"/>
        <rpm:entry name="(a or b or c)"/>
      </rpm:requires>
    </format>
  </package>
</metadata>
"#;

    let packages = parser
        .parse_primary_xml(xml, "https://example.com")
        .unwrap();
    let pkg = &packages[0];

    assert_eq!(pkg.requirements.len(), 3);

    // `(foo or bar)` should be parsed into an OR-group with two alternatives
    let or_group = &pkg.requirements[0];
    assert_eq!(or_group.behavior, ConditionalRequirementBehavior::Hard);
    assert_eq!(or_group.alternatives.len(), 2);
    assert_eq!(or_group.alternatives[0].name, "foo");
    assert_eq!(or_group.alternatives[1].name, "bar");

    // `(foo if bar)` retains both atoms and exact conditional structure.
    let conditional = &pkg.requirements[1];
    assert_eq!(
        conditional.behavior,
        ConditionalRequirementBehavior::Conditional
    );
    assert_eq!(conditional.alternatives.len(), 2);
    assert_eq!(conditional.alternatives[0].name, "foo");
    assert_eq!(conditional.alternatives[1].name, "bar");
    assert!(matches!(
        conditional.expression,
        crate::repository::dependency_model::RepositoryRequirementExpression::If { .. }
    ));

    // `(a or b or c)` should be parsed into a 3-way OR-group
    let triple = &pkg.requirements[2];
    assert_eq!(triple.behavior, ConditionalRequirementBehavior::Hard);
    assert_eq!(triple.alternatives.len(), 3);
    assert_eq!(triple.alternatives[0].name, "a");
    assert_eq!(triple.alternatives[1].name, "b");
    assert_eq!(triple.alternatives[2].name, "c");
}

#[test]
fn malformed_rich_dependency_rejects_repository_metadata() {
    let parser = parser();
    let xml = r#"
<metadata xmlns:rpm="http://linux.duke.edu/metadata/rpm">
  <package type="rpm">
    <name>test-pkg</name>
    <arch>x86_64</arch>
    <version epoch="0" ver="1.0" rel="1.fc44"/>
    <checksum type="sha256">bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb</checksum>
    <summary>Test</summary>
    <description>Test</description>
    <size package="100"/>
    <location href="Packages/t/test-pkg-1.0-1.fc44.x86_64.rpm"/>
    <format>
      <rpm:requires>
        <rpm:entry name="(foo if bar if baz)"/>
      </rpm:requires>
    </format>
  </package>
</metadata>
"#;

    assert!(
        parser
            .parse_primary_xml(xml, "https://example.com")
            .is_err()
    );
}
