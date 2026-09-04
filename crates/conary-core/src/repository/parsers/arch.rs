// crates/conary-core/src/repository/parsers/arch.rs

//! Arch Linux repository metadata parser
//!
//! Parses Arch Linux .db.tar.gz files which contain package metadata
//! in a custom text format with %FIELD% markers.

use super::{
    ArchPackageFragmentKind, AuthenticatedMetadataObject, AuthenticatedMetadataObjectRole,
    AuthenticatedProjectionInputV1, AuthenticatedSnapshotIdentity, ChecksumType, PackageMetadata,
    RepositoryParser, RepositorySnapshotSink, SourceCandidatePreflightOutcome,
};
use crate::error::{Error, Result};
use crate::repository::catalog::{CatalogMetadataStreamAdmission, CatalogMetadataStreamScratchV1};
use crate::repository::dependency_model::{
    RepositoryDependencyFlavor, RepositoryProvide, RepositoryRequirementGroup,
    RepositoryRequirementKind,
};
use crate::repository::package_relation::{parse_arch_provide, parse_native_relation};
use crate::repository::trust::openpgp::PreparedOpenPgpTrust;
use crate::repository::trust::{ArchSignatureRequirement, RepositoryTrustPolicy, TrustRole};
use crate::repository::versioning::VersionScheme;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use tar::Archive;
use tracing::{debug, info};

use super::common::{self, MAX_PACKAGE_SIZE};

mod preflight;

/// Arch Linux repository parser
pub struct ArchParser {
    /// Repository name (e.g., "core", "extra", "community")
    repo_name: String,
    trust: PreparedOpenPgpTrust,
}

impl ArchParser {
    /// Create a new Arch Linux parser for a specific repository
    pub fn new(repo_name: String, trust: PreparedOpenPgpTrust) -> Result<Self> {
        if !matches!(trust.policy(), RepositoryTrustPolicy::Arch { .. }) {
            return Err(Error::ConfigError(
                "Arch parser requires an Arch repository trust policy".to_string(),
            ));
        }
        Ok(Self { repo_name, trust })
    }

    /// Download and authenticate the repository database into run-local disk.
    async fn download_database(
        &self,
        repo_url: &str,
        work_directory: &std::path::Path,
        scratch_admission: &dyn CatalogMetadataStreamAdmission,
    ) -> Result<(
        std::path::PathBuf,
        AuthenticatedSnapshotIdentity,
        AuthenticatedMetadataObject,
    )> {
        let db_url = format!("{}/{}.db", repo_url.trim_end_matches('/'), self.repo_name);
        debug!("Downloading Arch database from: {}", db_url);

        let client = self.trust.repository_client()?;
        let database_file = work_directory.join("arch-database");
        let download = client
            .download_file_with_identity_admission(&db_url, &database_file, scratch_admission)
            .await?;
        let signature_url = format!("{db_url}.sig");
        let requirement = match self.trust.policy() {
            RepositoryTrustPolicy::Arch { sig_level, .. } => sig_level.database,
            _ => unreachable!("constructor validates Arch trust"),
        };
        match client.download_to_bytes(&signature_url).await {
            Ok(signature) => {
                self.trust.verify_detached_file(
                    TrustRole::ArchDatabase,
                    &database_file,
                    &signature,
                )?;
            }
            Err(Error::HttpStatus {
                status: 403 | 404, ..
            }) if requirement == ArchSignatureRequirement::Optional => {}
            Err(Error::HttpStatus {
                status: 403 | 404, ..
            }) => {
                return Err(Error::GpgVerificationFailed(format!(
                    "Arch SigLevel requires repository database signature {signature_url}"
                )));
            }
            Err(error) => return Err(error),
        }
        let snapshot = AuthenticatedSnapshotIdentity::from_download(&download)?;
        let database_path = format!("{}.db", self.repo_name);
        let database_object = AuthenticatedMetadataObject {
            role: AuthenticatedMetadataObjectRole::ArchDatabase,
            source_path: database_path,
            sha256: download.sha256,
            size: download.size,
        };
        Ok((database_file, snapshot, database_object))
    }

    /// Parse a desc file from the tarball
    fn parse_desc_file(&self, content: &str) -> Result<HashMap<String, Vec<String>>> {
        let mut fields = HashMap::new();
        let mut current_field: Option<String> = None;
        let mut values: Vec<String> = Vec::new();

        for line in content.lines() {
            let trimmed = line.trim();

            if trimmed.starts_with('%') && trimmed.ends_with('%') {
                // Save previous field
                if let Some(field) = current_field.take()
                    && fields
                        .insert(field.clone(), std::mem::take(&mut values))
                        .is_some()
                {
                    return Err(Error::ParseError(format!(
                        "Arch repository metadata repeats %{field}%"
                    )));
                }

                // Start new field
                current_field = Some(trimmed[1..trimmed.len() - 1].to_string());
            } else if !trimmed.is_empty() {
                if current_field.is_none() {
                    return Err(Error::ParseError(format!(
                        "Arch repository metadata has value {trimmed:?} outside a %FIELD% block"
                    )));
                }
                // Add value to current field
                values.push(trimmed.to_string());
            }
        }

        // Save last field
        if let Some(field) = current_field
            && fields.insert(field.clone(), values).is_some()
        {
            return Err(Error::ParseError(format!(
                "Arch repository metadata repeats %{field}%"
            )));
        }

        Ok(fields)
    }

    /// Build structured requirement groups from a depends file.
    fn parse_structured_depends(&self, content: &str) -> Result<Vec<RepositoryRequirementGroup>> {
        let fields = self.parse_desc_file(content)?;
        let mut groups = Vec::new();

        if let Some(deps) = fields.get("DEPENDS") {
            for dep in deps {
                groups.push(
                    crate::repository::requirement::parse_native_requirement(
                        RepositoryRequirementKind::Depends,
                        VersionScheme::Arch,
                        dep,
                    )
                    .map_err(|error| {
                        Error::ParseError(format!("invalid Arch %DEPENDS% entry '{dep}': {error}"))
                    })?,
                );
            }
        }

        if let Some(opts) = fields.get("OPTDEPENDS") {
            for opt in opts {
                // libalpm's alpm_dep_from_string() recognizes only ": " as
                // the description boundary so an epoch colon remains part of
                // the version.
                let (native_requirement, description) =
                    if let Some((package, description)) = opt.split_once(": ") {
                        (
                            package.trim(),
                            Some(description.trim().to_string()).filter(|value| !value.is_empty()),
                        )
                    } else {
                        (opt.as_str(), None)
                    };
                let mut group = crate::repository::requirement::parse_native_requirement(
                    RepositoryRequirementKind::Optional,
                    VersionScheme::Arch,
                    native_requirement,
                )
                .map_err(|error| {
                    Error::ParseError(format!("invalid Arch %OPTDEPENDS% entry '{opt}': {error}"))
                })?;
                group.description = description;
                group.native_text = Some(opt.clone());
                groups.push(group);
            }
        }

        Ok(groups)
    }

    fn parse_relation_fields(
        &self,
        field_sets: &[&HashMap<String, Vec<String>>],
    ) -> Result<Vec<RepositoryRequirementGroup>> {
        let mut seen = HashSet::new();
        let mut relations = Vec::new();
        for fields in field_sets {
            for (field, kind) in [
                ("CONFLICTS", RepositoryRequirementKind::Conflict),
                ("REPLACES", RepositoryRequirementKind::Replace),
            ] {
                if let Some(entries) = fields.get(field) {
                    for entry in entries {
                        if !seen.insert((kind, entry.clone())) {
                            continue;
                        }
                        relations.push(
                            parse_native_relation(kind, VersionScheme::Arch, entry).map_err(
                                |error| {
                                    Error::ParseError(format!(
                                        "invalid Arch %{field}% relation '{entry}': {error}"
                                    ))
                                },
                            )?,
                        );
                    }
                }
            }
        }
        Ok(relations)
    }

    /// Build structured provides from desc fields.
    fn build_structured_provides(
        &self,
        name: &str,
        version: &str,
        desc_fields: &HashMap<String, Vec<String>>,
    ) -> Result<Vec<RepositoryProvide>> {
        let mut provides = vec![RepositoryProvide::package_name(
            name.to_string(),
            Some(version.to_string()),
        )];

        if let Some(prov_list) = desc_fields.get("PROVIDES") {
            for (record_index, prov) in prov_list.iter().enumerate() {
                let parsed = parse_arch_provide(prov, name).map_err(|error| {
                    Error::ParseError(format!("invalid Arch %PROVIDES% entry '{prov}': {error}"))
                })?;

                provides.push(RepositoryProvide {
                    name: parsed.name,
                    kind: parsed.kind,
                    version_relation: parsed.version.as_ref().map(|_| {
                        crate::repository::dependency_model::ProvideVersionRelation::Equal
                    }),
                    version: parsed.version,
                    architecture_qualifier:
                        crate::repository::dependency_model::ProvideArchitectureQualifier::Implicit,
                    native_text: Some(prov.clone()),
                    provenance:
                        crate::repository::dependency_model::CapabilityProvenance::SourceDeclared {
                            format: crate::repository::dependency_model::SourcePackageFormat::Alpm,
                            record_index: u32::try_from(record_index).map_err(|_| {
                                Error::ParseError(
                                    "Arch repository provide index exceeds u32".to_string(),
                                )
                            })?,
                        },
                });
            }
        }

        Ok(provides)
    }

    fn package_from_fields(
        &self,
        repo_url: &str,
        desc_fields: &HashMap<String, Vec<String>>,
        depends_content: Option<&str>,
    ) -> Result<PackageMetadata> {
        let name = desc_fields
            .get("NAME")
            .and_then(|v| v.first())
            .ok_or_else(|| Error::ParseError("Missing %NAME% field".to_string()))?
            .clone();

        let version = desc_fields
            .get("VERSION")
            .and_then(|v| v.first())
            .ok_or_else(|| Error::ParseError("Missing %VERSION% field".to_string()))?
            .clone();

        let filename = desc_fields
            .get("FILENAME")
            .and_then(|v| v.first())
            .ok_or_else(|| Error::ParseError("Missing %FILENAME% field".to_string()))?
            .clone();

        let checksum = desc_fields
            .get("SHA256SUM")
            .and_then(|v| v.first())
            .ok_or_else(|| Error::ParseError("Missing %SHA256SUM% field".to_string()))?
            .clone();

        let size: u64 = desc_fields
            .get("CSIZE")
            .and_then(|v| v.first())
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| Error::ParseError("Missing or invalid %CSIZE% field".to_string()))?;

        if size > MAX_PACKAGE_SIZE {
            return Err(Error::ParseError(format!(
                "Package {} size {} exceeds maximum allowed (5GB)",
                name, size
            )));
        }

        let architecture = desc_fields.get("ARCH").and_then(|v| v.first()).cloned();
        let description = desc_fields.get("DESC").and_then(|v| v.first()).cloned();

        if let Err(msg) = common::validate_filename(&filename) {
            return Err(Error::ParseError(msg));
        }

        let download_url = common::join_repo_url(repo_url, &filename);

        let mut extra = serde_json::Map::new();
        if let Some(url) = desc_fields.get("URL").and_then(|v| v.first()) {
            extra.insert(
                "homepage".to_string(),
                serde_json::Value::String(url.clone()),
            );
        }
        if let Some(license) = desc_fields.get("LICENSE").and_then(|v| v.first()) {
            extra.insert(
                "license".to_string(),
                serde_json::Value::String(license.clone()),
            );
        }
        if let Some(builddate) = desc_fields.get("BUILDDATE").and_then(|v| v.first()) {
            extra.insert(
                "builddate".to_string(),
                serde_json::Value::String(builddate.clone()),
            );
        }
        if let Some(installed_size_str) = desc_fields.get("ISIZE").and_then(|v| v.first()) {
            extra.insert(
                "installed_size".to_string(),
                serde_json::Value::String(installed_size_str.clone()),
            );
        }
        if let Some(provides) = desc_fields.get("PROVIDES") {
            extra.insert(
                "arch_provides".to_string(),
                serde_json::Value::Array(
                    provides
                        .iter()
                        .cloned()
                        .map(serde_json::Value::String)
                        .collect(),
                ),
            );
        }
        extra.insert(
            "format".to_string(),
            serde_json::Value::String("arch".to_string()),
        );

        let mut requirements = depends_content
            .map(|content| self.parse_structured_depends(content))
            .transpose()?
            .unwrap_or_default();
        let depends_fields = depends_content
            .map(|content| self.parse_desc_file(content))
            .transpose()?;
        let relation_field_sets = depends_fields
            .as_ref()
            .map_or_else(|| vec![desc_fields], |fields| vec![desc_fields, fields]);
        requirements.extend(self.parse_relation_fields(&relation_field_sets)?);

        let structured_provides = self.build_structured_provides(&name, &version, desc_fields)?;

        Ok(PackageMetadata {
            name,
            version,
            architecture,
            debian_multi_arch: None,
            description,
            checksum,
            checksum_type: ChecksumType::Sha256,
            size,
            download_url,
            extra_metadata: serde_json::Value::Object(extra),
            dependency_flavor: RepositoryDependencyFlavor::Arch,
            version_scheme: VersionScheme::Arch,
            requirements,
            provides: structured_provides,
        })
    }
}

impl RepositoryParser for ArchParser {
    async fn ingest_snapshot<S: RepositorySnapshotSink + Send>(
        &self,
        repo_url: &str,
        sink: &mut S,
    ) -> Result<AuthenticatedSnapshotIdentity> {
        info!("Syncing Arch Linux repository: {}", self.repo_name);

        let work_directory = sink.work_directory().to_path_buf();
        let database_path = format!("{}.db", self.repo_name);
        let scratch_admission =
            sink.streamed_authenticated_metadata(CatalogMetadataStreamScratchV1::new(
                AuthenticatedMetadataObjectRole::ArchDatabase,
                database_path,
            )?)?;
        let (database_file, snapshot, database_object) = self
            .download_database(repo_url, &work_directory, scratch_admission.as_ref())
            .await?;
        let projection_input =
            AuthenticatedProjectionInputV1::exact_object(database_object.clone());
        if sink.reuse_cached_projection(&snapshot, std::slice::from_ref(&projection_input))? {
            sink.authenticated_object(database_object, &database_file)?;
            info!("Reused cached Arch repository projection");
            return Ok(snapshot);
        }
        let arch_fragments_replayed = if sink.requires_source_candidate_preflight() {
            preflight::preflight_database(self, repo_url, &database_file, sink)?;
            match sink.begin_source_candidate()? {
                SourceCandidatePreflightOutcome::ArchFragmentsReplayed => true,
                SourceCandidatePreflightOutcome::ReplayAuthenticatedMetadata => false,
                SourceCandidatePreflightOutcome::CompleteProjection { .. } => {
                    return Err(Error::InternalError(
                        "Arch parser received a complete non-ALPM preflight replay outcome"
                            .to_string(),
                    ));
                }
            }
        } else {
            false
        };
        if !arch_fragments_replayed {
            let decoder = super::common::open_metadata_decoder(
                &database_file,
                &format!("Arch repository database {}", database_file.display()),
            )?;
            let mut archive = Archive::new(decoder);
            for entry in archive.entries()? {
                let mut entry = entry.map_err(|e| {
                    Error::ParseError(format!("Failed to read tarball entry: {}", e))
                })?;

                let path = entry
                    .path()
                    .map_err(|e| Error::ParseError(format!("Invalid path in tarball: {}", e)))?;

                let path_str = path.to_str().ok_or_else(|| {
                    Error::ParseError("Arch repository entry path is not valid UTF-8".to_string())
                })?;

                if let Some(dir) = path_str.split('/').next().filter(|dir| !dir.is_empty()) {
                    let dir_key = dir.to_string();

                    if path_str.ends_with("/desc") {
                        let mut content = String::new();
                        entry.read_to_string(&mut content).map_err(|e| {
                            Error::ParseError(format!("Failed to read desc file: {}", e))
                        })?;
                        sink.stage_arch_package_fragment(
                            dir_key,
                            ArchPackageFragmentKind::Desc,
                            content,
                        )?;
                    } else if path_str.ends_with("/depends") {
                        let mut content = String::new();
                        entry.read_to_string(&mut content).map_err(|e| {
                            Error::ParseError(format!("Failed to read depends file: {}", e))
                        })?;
                        sink.stage_arch_package_fragment(
                            dir_key,
                            ArchPackageFragmentKind::Depends,
                            content,
                        )?;
                    }
                }
            }
        }

        let mut package_count = 0_u64;
        while let Some(record) = sink.take_arch_package_record()? {
            let desc_fields = self.parse_desc_file(&record.desc)?;
            sink.package(self.package_from_fields(
                repo_url,
                &desc_fields,
                record.depends.as_deref(),
            )?)?;
            package_count = package_count.checked_add(1).ok_or_else(|| {
                Error::ParseError("Arch repository package count exceeds u64".to_string())
            })?;
        }

        sink.authenticated_object(database_object, &database_file)?;
        info!("Parsed {} packages from Arch repository", package_count);
        Ok(snapshot)
    }
}

#[cfg(test)]
mod tests;
