// conary-core/src/repository/parsers/fedora/metadata.rs

//! Authenticated Fedora repository metadata acquisition.

use std::path::{Path, PathBuf};

use tracing::debug;

use super::FedoraParser;
use super::metalink::parse_metalink_repomd_identity;
use super::repomd::{self, RepoMdIndex, RepoMdRecord};
use crate::error::{Error, Result};
use crate::repository::catalog::{CatalogMetadataObjectScratchV1, CatalogMetadataScratchV1};
use crate::repository::client::{DownloadedFileIdentity, RepositoryClient};
use crate::repository::parsers::{
    AuthenticatedMetadataObject, AuthenticatedMetadataObjectRole, AuthenticatedSnapshotIdentity,
};
use crate::repository::trust::{RepositoryTrustPolicy, RpmMetadataAuthority, TrustRole};

pub(super) fn authenticated_metadata_scratch(
    repomd: &RepoMdIndex,
) -> Result<CatalogMetadataScratchV1> {
    let mut objects = vec![CatalogMetadataObjectScratchV1 {
        role: AuthenticatedMetadataObjectRole::RpmPrimary,
        source_path: repomd.primary.href.clone(),
        size: repomd.primary.size,
    }];
    if let Some(filelists) = &repomd.filelists {
        objects.push(CatalogMetadataObjectScratchV1 {
            role: AuthenticatedMetadataObjectRole::RpmFilelists,
            source_path: filelists.href.clone(),
            size: filelists.size,
        });
    }
    CatalogMetadataScratchV1::from_signed_objects(objects)
}

impl FedoraParser {
    /// Download repomd.xml and admit the metadata records Conary reads.
    pub(super) async fn fetch_repomd_index(
        &self,
        repo_url: &str,
    ) -> Result<(RepoMdIndex, AuthenticatedSnapshotIdentity)> {
        let repomd_url = format!("{}/repodata/repomd.xml", repo_url.trim_end_matches('/'));
        debug!("Downloading repomd.xml from: {}", repomd_url);

        let client = RepositoryClient::new()?;
        let xml_bytes = client.download_to_bytes(&repomd_url).await?;
        let RepositoryTrustPolicy::Rpm { metadata, .. } = self.trust.policy() else {
            return Err(Error::ConfigError(
                "RPM parser lost its RPM trust policy".to_string(),
            ));
        };
        match metadata {
            RpmMetadataAuthority::OpenPgp { .. } => {
                let signature_url = format!("{repomd_url}.asc");
                let signature =
                    client
                        .download_to_bytes(&signature_url)
                        .await
                        .map_err(|error| {
                            Error::GpgVerificationFailed(format!(
                                "RPM repository metadata signature {signature_url} is required: \
                             {error}"
                            ))
                        })?;
                self.trust
                    .verify_detached(TrustRole::RpmMetadata, &xml_bytes, &signature)?;
            }
            RpmMetadataAuthority::Metalink { url } => {
                let metalink = client.download_to_bytes(url).await?;
                let identity = parse_metalink_repomd_identity(&metalink)?;
                identity.verify(&xml_bytes)?;
            }
        }
        let snapshot = AuthenticatedSnapshotIdentity::for_bytes(&xml_bytes);
        let xml_content = String::from_utf8(xml_bytes)
            .map_err(|error| Error::ParseError(format!("Invalid UTF-8 in repomd.xml: {error}")))?;
        Ok((repomd::parse_repomd(&xml_content)?, snapshot))
    }

    /// Download one metadata document and verify it against its signed record.
    ///
    /// The served compressed bytes are what signed repomd.xml records, so
    /// identity is established before a decoder sees them.
    pub(super) async fn download_verified_document(
        &self,
        repo_url: &str,
        record: &RepoMdRecord,
        work_directory: &Path,
        file_name: &str,
    ) -> Result<(String, PathBuf, DownloadedFileIdentity)> {
        let document_url = format!("{}/{}", repo_url.trim_end_matches('/'), record.href);
        debug!(
            "Downloading {} metadata from: {}",
            record.document.label(),
            document_url
        );

        let client = RepositoryClient::new()?;
        let path = work_directory.join(file_name);
        let identity = client
            .download_file_with_identity_limit(&document_url, &path, record.size)
            .await?;
        record.verify_served_download(&identity)?;
        Ok((document_url, path, identity))
    }

    /// Download and verify primary.xml without materializing its contents.
    pub(super) async fn download_primary_xml(
        &self,
        repo_url: &str,
        record: &RepoMdRecord,
        work_directory: &Path,
    ) -> Result<(String, PathBuf, AuthenticatedMetadataObject)> {
        let (primary_url, path, identity) = self
            .download_verified_document(repo_url, record, work_directory, "rpm-primary")
            .await?;
        let authenticated_object = AuthenticatedMetadataObject {
            role: AuthenticatedMetadataObjectRole::RpmPrimary,
            source_path: record.href.clone(),
            sha256: identity.sha256,
            size: identity.size,
        };
        Ok((primary_url, path, authenticated_object))
    }
}
