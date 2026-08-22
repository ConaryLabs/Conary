// conary-core/src/repository/parsers/eopkg.rs

//! Authenticated Solus eopkg repository-index parser.

use super::{
    AuthenticatedMetadataObjectRole, AuthenticatedRepositoryMetadata,
    AuthenticatedSnapshotIdentity, ChecksumType, PackageMetadata, RepositoryParser,
    authenticated_metadata_object,
};
use crate::error::{Error, Result};
use crate::packages::eopkg::xml;
use crate::repository::RepositoryClient;
use crate::repository::dependency_model::{
    CapabilityProvenance, ProvideArchitectureQualifier, RepositoryDependencyFlavor,
    RepositoryProvide, RepositoryRequirementKind,
};
use crate::repository::trust::RepositoryTrustPolicy;
use crate::repository::trust::openpgp::PreparedOpenPgpTrust;
use crate::repository::versioning::VersionScheme;
use quick_xml::events::{BytesEnd, BytesStart, Event};
use quick_xml::{Reader, Writer};

pub struct EopkgParser {
    architecture: String,
    trust: PreparedOpenPgpTrust,
}

impl EopkgParser {
    pub fn new(architecture: String, trust: PreparedOpenPgpTrust) -> Result<Self> {
        if !matches!(trust.policy(), RepositoryTrustPolicy::Eopkg { .. }) {
            return Err(Error::ConfigError(
                "eopkg parser requires an eopkg repository trust policy".to_string(),
            ));
        }
        Ok(Self {
            architecture,
            trust,
        })
    }
}

impl RepositoryParser for EopkgParser {
    async fn sync_metadata(&self, repo_url: &str) -> Result<AuthenticatedRepositoryMetadata> {
        let RepositoryTrustPolicy::Eopkg { origin } = self.trust.policy() else {
            unreachable!("constructor admitted eopkg policy")
        };
        let normalized = format!("{}/", repo_url.trim_end_matches('/'));
        if &normalized != origin {
            return Err(Error::TrustError(format!(
                "eopkg repository URL {normalized} disagrees with enrolled origin {origin}"
            )));
        }
        let index_url = format!("{origin}eopkg-index.xml.xz");
        let digest_url = format!("{index_url}.sha256sum");
        let client = RepositoryClient::new()?;
        let digest_bytes = client.download_to_bytes(&digest_url).await?;
        let enrolled_digest = parse_sha256_sidecar(&digest_bytes)?;
        let compressed = client.download_to_bytes(&index_url).await?;
        let actual = crate::hash::sha256(&compressed);
        if actual != enrolled_digest {
            return Err(Error::TrustError(format!(
                "eopkg compressed index SHA-256 mismatch: authenticated sidecar {enrolled_digest}, received {actual}"
            )));
        }
        let snapshot = AuthenticatedSnapshotIdentity::for_bytes(&compressed);
        let xml_bytes =
            crate::compression::decompress_metadata_auto(&compressed, "authenticated eopkg index")?;
        let packages = parse_index(&xml_bytes, origin, &self.architecture)?;
        Ok(AuthenticatedRepositoryMetadata {
            packages,
            snapshot,
            authenticated_objects: vec![authenticated_metadata_object(
                AuthenticatedMetadataObjectRole::EopkgIndex,
                "eopkg-index.xml.xz",
                &compressed,
            )],
        })
    }
}

fn parse_sha256_sidecar(bytes: &[u8]) -> Result<String> {
    if bytes.len() != 64
        || !bytes
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        return Err(Error::TrustError(
            "eopkg index SHA-256 sidecar must be exactly 64 lowercase hexadecimal bytes"
                .to_string(),
        ));
    }
    String::from_utf8(bytes.to_vec())
        .map_err(|error| Error::TrustError(format!("invalid eopkg SHA-256 sidecar: {error}")))
}

fn parse_index(bytes: &[u8], origin: &str, architecture: &str) -> Result<Vec<PackageMetadata>> {
    let mut reader = Reader::from_reader(bytes);
    let mut depth = 0_u64;
    let mut capture: Option<(Writer<Vec<u8>>, u64)> = None;
    let mut packages = Vec::new();
    let mut package_records = 0_u64;
    loop {
        let event = reader
            .read_event()
            .map_err(|error| Error::ParseError(format!("invalid eopkg index XML: {error}")))?;
        if let Some((writer, capture_depth)) = capture.as_mut() {
            writer
                .write_event(event.clone().into_owned())
                .map_err(|error| Error::ParseError(error.to_string()))?;
            match &event {
                Event::Start(_) => *capture_depth += 1,
                Event::End(_) => {
                    *capture_depth -= 1;
                    if *capture_depth == 0 {
                        let (mut writer, _) = capture.take().unwrap();
                        writer
                            .write_event(Event::End(BytesEnd::new("PISI")))
                            .map_err(|error| Error::ParseError(error.to_string()))?;
                        package_records += 1;
                        let metadata = xml::parse_metadata(&writer.into_inner()).map_err(|error| {
                            Error::ParseError(format!(
                                "eopkg index Package record {package_records} is invalid: {error}"
                            ))
                        })?;
                        if metadata.architecture == architecture {
                            packages.push(project(metadata, origin)?);
                        }
                    }
                }
                _ => {}
            }
        } else {
            match &event {
                Event::Start(start) if depth == 1 && start.local_name().as_ref() == b"Package" => {
                    let mut writer = Writer::new(Vec::new());
                    writer
                        .write_event(Event::Start(BytesStart::new("PISI")))
                        .map_err(|error| Error::ParseError(error.to_string()))?;
                    writer
                        .write_event(event.clone().into_owned())
                        .map_err(|error| Error::ParseError(error.to_string()))?;
                    capture = Some((writer, 1));
                }
                Event::Start(_) => depth += 1,
                Event::End(_) => depth = depth.saturating_sub(1),
                Event::Eof => break,
                _ => {}
            }
        }
    }
    Ok(packages)
}

fn project(metadata: xml::Metadata, origin: &str) -> Result<PackageMetadata> {
    let size = metadata.package_size.ok_or_else(|| {
        Error::ParseError(format!(
            "eopkg index package {} has no PackageSize",
            metadata.name
        ))
    })?;
    let checksum = metadata.package_hash.ok_or_else(|| {
        Error::ParseError(format!(
            "eopkg index package {} has no PackageHash",
            metadata.name
        ))
    })?;
    if checksum.len() != 40
        || !checksum
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::ParseError(format!(
            "eopkg index package {} has invalid SHA-1",
            metadata.name
        )));
    }
    let uri = metadata.package_uri.ok_or_else(|| {
        Error::ParseError(format!(
            "eopkg index package {} has no PackageURI",
            metadata.name
        ))
    })?;
    crate::repository::parsers::common::validate_filename(&uri).map_err(Error::ParseError)?;
    for delta in &metadata.deltas {
        if delta.package_hash.len() != 40
            || !delta
                .package_hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(Error::ParseError(format!(
                "eopkg index package {} has invalid delta SHA-1",
                metadata.name
            )));
        }
        crate::repository::parsers::common::validate_filename(&delta.package_uri)
            .map_err(Error::ParseError)?;
    }
    let version = format!("{}-{}", metadata.version, metadata.release);
    crate::repository::versioning::validate_repo_version(VersionScheme::Eopkg, &version)
        .map_err(|error| Error::ParseError(error.to_string()))?;

    let mut requirements = crate::packages::eopkg::relation_groups(
        metadata.dependencies,
        RepositoryRequirementKind::Depends,
    )?;
    requirements.extend(crate::packages::eopkg::relation_groups(
        metadata
            .conflicts
            .into_iter()
            .map(|record| vec![record])
            .collect(),
        RepositoryRequirementKind::Conflict,
    )?);
    requirements.extend(crate::packages::eopkg::relation_groups(
        metadata
            .replaces
            .into_iter()
            .map(|record| vec![record])
            .collect(),
        RepositoryRequirementKind::Replace,
    )?);
    let mut provides = vec![RepositoryProvide::package_name(
        metadata.name.clone(),
        Some(version.clone()),
    )];
    for (kind, names) in [
        (
            crate::repository::dependency_model::RepositoryCapabilityKind::PkgConfig,
            metadata.pkg_config,
        ),
        (
            crate::repository::dependency_model::RepositoryCapabilityKind::PkgConfig32,
            metadata.pkg_config_32,
        ),
        (
            crate::repository::dependency_model::RepositoryCapabilityKind::Comar,
            metadata.comar,
        ),
    ] {
        for name in names {
            provides.push(RepositoryProvide {
                name,
                kind,
                version: None,
                version_relation: None,
                architecture_qualifier: ProvideArchitectureQualifier::Implicit,
                native_text: None,
                provenance: CapabilityProvenance::SourceDeclared {
                    format: crate::repository::dependency_model::SourcePackageFormat::Eopkg,
                    record_index: u32::try_from(provides.len()).map_err(|_| {
                        Error::ParseError("eopkg provide index exceeds u32".to_string())
                    })?,
                },
            });
        }
    }
    Ok(PackageMetadata {
        name: metadata.name,
        version,
        architecture: Some(metadata.architecture),
        debian_multi_arch: None,
        description: metadata.description.or(metadata.summary),
        checksum,
        checksum_type: ChecksumType::Sha1,
        size,
        download_url: format!("{origin}{uri}"),
        extra_metadata: serde_json::json!({
            "distribution": metadata.distribution,
            "distribution_release": metadata.distribution_release,
            "package_format": metadata.package_format,
            "delta_packages": metadata.deltas,
        }),
        dependency_flavor: RepositoryDependencyFlavor::Eopkg,
        version_scheme: VersionScheme::Eopkg,
        requirements,
        provides,
    })
}

#[cfg(test)]
mod tests {
    use super::{AuthenticatedSnapshotIdentity, parse_index, parse_sha256_sidecar};

    const DIGEST: &str = "efa19936d28e2e84b462cf2e07efb9cf1b3afb983b0c7b5a5f813d2d52a8061f";

    #[test]
    fn exact_lowercase_sha256_sidecar_is_admitted() {
        assert_eq!(parse_sha256_sidecar(DIGEST.as_bytes()).unwrap(), DIGEST);
    }

    #[test]
    fn decorated_or_noncanonical_sha256_sidecars_fail_closed() {
        for invalid in [
            format!("{DIGEST}\n"),
            format!("{DIGEST}  eopkg-index.xml.xz"),
            DIGEST.to_ascii_uppercase(),
            DIGEST[..63].to_string(),
        ] {
            assert!(
                parse_sha256_sidecar(invalid.as_bytes()).is_err(),
                "{invalid:?}"
            );
        }
    }

    #[test]
    fn compressed_index_identity_preserves_exact_served_size() {
        let compressed = b"authenticated eopkg index bytes";
        let identity = AuthenticatedSnapshotIdentity::for_bytes(compressed);
        assert_eq!(identity.size(), Some(compressed.len() as u64));
    }

    fn index_record(package_hash: &str, package_uri: &str, architecture: &str) -> String {
        format!(
            r#"<PISI><Package><Name>jq</Name><Summary>jq</Summary><RuntimeDependencies><Dependency releaseFrom="5">oniguruma</Dependency><Dependency releaseFrom="141">glibc</Dependency></RuntimeDependencies><History><Update release="13"><Version>1.8.2</Version></Update></History><Distribution>Solus</Distribution><DistributionRelease>1</DistributionRelease><Architecture>{architecture}</Architecture><InstalledSize>505000</InstalledSize><PackageSize>212175</PackageSize><PackageHash>{package_hash}</PackageHash><PackageURI>{package_uri}</PackageURI><PackageFormat>1.2</PackageFormat></Package></PISI>"#
        )
    }

    #[test]
    fn delta_metadata_is_typed_and_retained_without_becoming_full_artifact_authority() {
        let xml = index_record(
            "98591d2acce52d110a1ae62b775079d79193ad01",
            "j/jq/jq-1.8.2-13-1-x86_64.eopkg",
            "x86_64",
        )
        .replace(
            "<PackageFormat>1.2</PackageFormat>",
            "<DeltaPackages><Delta releaseFrom=\"12\" buildFrom=\"1\"><PackageURI>j/jq/jq-12-to-13.delta.eopkg</PackageURI><PackageSize>1234</PackageSize><PackageHash>1111111111111111111111111111111111111111</PackageHash></Delta></DeltaPackages><PackageFormat>1.2</PackageFormat>",
        );
        let packages = parse_index(
            xml.as_bytes(),
            "https://cdn.getsol.us/repo/polaris/",
            "x86_64",
        )
        .unwrap();
        let deltas = packages[0].extra_metadata["delta_packages"]
            .as_array()
            .unwrap();
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0]["release_from"], 12);
        assert_eq!(deltas[0]["build_from"], "1");
        assert_eq!(deltas[0]["package_size"], 1234);
        assert_eq!(
            packages[0].checksum,
            "98591d2acce52d110a1ae62b775079d79193ad01"
        );
    }

    #[test]
    fn malformed_delta_authority_fails_closed() {
        let xml = index_record(
            "98591d2acce52d110a1ae62b775079d79193ad01",
            "j/jq/jq-1.8.2-13-1-x86_64.eopkg",
            "x86_64",
        )
        .replace(
            "<PackageFormat>1.2</PackageFormat>",
            "<DeltaPackages><Delta releaseFrom=\"12\"><PackageURI>../escape.delta.eopkg</PackageURI><PackageSize>1234</PackageSize><PackageHash>NOT-A-SHA1</PackageHash></Delta></DeltaPackages><PackageFormat>1.2</PackageFormat>",
        );
        assert!(
            parse_index(
                xml.as_bytes(),
                "https://cdn.getsol.us/repo/polaris/",
                "x86_64",
            )
            .is_err()
        );
    }

    #[test]
    fn authentic_jq_index_identity_projects_without_loss() {
        let packages = parse_index(
            index_record(
                "98591d2acce52d110a1ae62b775079d79193ad01",
                "j/jq/jq-1.8.2-13-1-x86_64.eopkg",
                "x86_64",
            )
            .as_bytes(),
            "https://cdn.getsol.us/repo/polaris/",
            "x86_64",
        )
        .unwrap();

        assert_eq!(packages.len(), 1);
        let package = &packages[0];
        assert_eq!(package.name, "jq");
        assert_eq!(package.version, "1.8.2-13");
        assert_eq!(package.architecture.as_deref(), Some("x86_64"));
        assert_eq!(package.checksum, "98591d2acce52d110a1ae62b775079d79193ad01");
        assert_eq!(package.requirements.len(), 2);
        assert_eq!(package.size, 212175);
        assert_eq!(
            package.download_url,
            "https://cdn.getsol.us/repo/polaris/j/jq/jq-1.8.2-13-1-x86_64.eopkg"
        );
    }

    #[test]
    fn index_architecture_filter_and_malformed_authority_fail_closed() {
        let filtered = parse_index(
            index_record(
                "98591d2acce52d110a1ae62b775079d79193ad01",
                "j/jq/jq-1.8.2-13-1-x86_64.eopkg",
                "aarch64",
            )
            .as_bytes(),
            "https://cdn.getsol.us/repo/polaris/",
            "x86_64",
        )
        .unwrap();
        assert!(filtered.is_empty());

        for (hash, uri) in [
            (
                "98591D2ACCE52D110A1AE62B775079D79193AD01",
                "j/jq/jq-1.8.2-13-1-x86_64.eopkg",
            ),
            (
                "98591d2acce52d110a1ae62b775079d79193ad01",
                "../jq-1.8.2-13-1-x86_64.eopkg",
            ),
        ] {
            assert!(
                parse_index(
                    index_record(hash, uri, "x86_64").as_bytes(),
                    "https://cdn.getsol.us/repo/polaris/",
                    "x86_64",
                )
                .is_err()
            );
        }
    }
}
