// crates/conary-core/src/db/models/converted/repository.rs

//! Exact immutable profile-revision identity and repository serving lookups.

use super::{
    CONVERSION_VERSION, ConvertedArtifactKind, ConvertedPackage, RepositoryConvertedArtifact,
};
use crate::error::Result;
use rusqlite::{Connection, TransactionBehavior, params};

impl ConvertedPackage {
    /// Check if a package needs re-conversion (algorithm upgraded)
    pub fn needs_reconversion(&self) -> bool {
        self.conversion_version != CONVERSION_VERSION
    }

    /// Check whether this repository artifact carries the requested exact
    /// immutable profile revision and current conversion algorithm.
    pub fn repository_conversion_is_current_for_revision(
        &self,
        profile_revision_sha256: &str,
    ) -> Result<bool> {
        super::validation::validate_sha256(profile_revision_sha256, "profile revision SHA-256")?;
        Ok(self.artifact_kind == ConvertedArtifactKind::Repository
            && !self.needs_reconversion()
            && self.profile_revision_sha256.as_deref() == Some(profile_revision_sha256))
    }

    /// Check whether this repository artifact is internally current.
    pub fn repository_conversion_is_current(&self) -> Result<bool> {
        let profile_revision_sha256 = self.profile_revision_sha256.as_deref().ok_or_else(|| {
            crate::Error::InternalError(format!(
                "repository converted package {} is missing profile_revision_sha256",
                self.record_identity()
            ))
        })?;
        self.repository_conversion_is_current_for_revision(profile_revision_sha256)
    }

    /// Remove repository conversion rows for one exact profile revision whose
    /// conversion algorithm is no longer current. No mutable source metadata
    /// participates in this cleanup.
    pub fn reconcile_repository_conversions(
        conn: &Connection,
        profile_revision_sha256: &str,
    ) -> Result<usize> {
        super::validation::validate_sha256(profile_revision_sha256, "profile revision SHA-256")?;
        let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
        let mut statement = tx.prepare(
            "SELECT id FROM converted_packages
             WHERE artifact_kind = 'repository'
               AND profile_revision_sha256 = ?1
               AND conversion_version != ?2
             ORDER BY id",
        )?;
        let ids = statement
            .query_map(
                params![profile_revision_sha256, CONVERSION_VERSION],
                |row| row.get::<_, i64>(0),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);

        let mut deleted = 0;
        for id in ids {
            if Self::delete_with_conversion_pin_in_transaction(&tx, id)? {
                deleted += 1;
            }
        }
        tx.commit()?;
        Ok(deleted)
    }

    /// Return the exact installed trove identity for an installed conversion.
    pub fn installed_trove_id(&self) -> Result<i64> {
        if self.artifact_kind != ConvertedArtifactKind::Installed {
            return Err(crate::Error::InternalError(format!(
                "converted package {} is a repository artifact, not an installed conversion",
                self.record_identity()
            )));
        }
        self.trove_id.ok_or_else(|| {
            crate::Error::InternalError(format!(
                "installed converted package {} has no trove identity",
                self.record_identity()
            ))
        })
    }

    /// Return a validated repository-serving artifact.
    ///
    /// Missing identity or storage fields and malformed chunk JSON are
    /// persisted-state corruption. Serving callers must propagate this error;
    /// they must not synthesize names, versions, sizes, hashes, or paths.
    pub fn repository_artifact(&self) -> Result<RepositoryConvertedArtifact<'_>> {
        if self.artifact_kind != ConvertedArtifactKind::Repository {
            return Err(crate::Error::InternalError(format!(
                "converted package {} is an installed conversion, not a repository artifact",
                self.record_identity()
            )));
        }

        let package_name = self.required_repository_text("package_name", &self.package_name)?;
        let package_version =
            self.required_repository_text("package_version", &self.package_version)?;
        let source_profile =
            self.required_repository_text("source_profile", &self.source_profile)?;
        let profile_revision_sha256 = self
            .required_repository_text("profile_revision_sha256", &self.profile_revision_sha256)?;
        super::validation::validate_sha256(profile_revision_sha256, "profile revision SHA-256")?;
        if crate::repository::supported_profiles::profile_by_public_id(source_profile).is_none() {
            return Err(crate::Error::InternalError(format!(
                "repository converted package {} carries unsupported source profile '{source_profile}'",
                self.record_identity()
            )));
        }
        let package_architecture =
            self.required_repository_text("package_architecture", &self.package_architecture)?;
        let repository_provides_digest = self
            .repository_provides_digest
            .as_deref()
            .map(|digest| {
                crate::hash::Hash::parse_prefixed(digest)
                    .map_err(|error| {
                        crate::Error::InternalError(format!(
                            "repository converted package {} has invalid repository_provides_digest: {error}",
                            self.record_identity()
                        ))
                    })
                    .and_then(|parsed| {
                        if parsed.algorithm != crate::hash::HashAlgorithm::Sha256 {
                            return Err(crate::Error::InternalError(format!(
                                "repository converted package {} repository_provides_digest must use sha256",
                                self.record_identity()
                            )));
                        }
                        Ok(digest)
                    })
            })
            .transpose()?;
        let content_hash = self.required_repository_text("content_hash", &self.content_hash)?;
        let ccs_path = self.required_repository_text("ccs_path", &self.ccs_path)?;
        let transport_json =
            self.required_repository_text("transport_json", &self.transport_json)?;
        let transport =
            serde_json::from_str::<crate::ccs::transport::CcsTransportEnvelopeV1>(transport_json)
                .map_err(|error| {
                crate::Error::InternalError(format!(
                    "repository converted package {} has malformed transport_json: {error}",
                    self.record_identity()
                ))
            })?;
        let total_size = self.total_size.ok_or_else(|| {
            crate::Error::InternalError(format!(
                "repository converted package {} is missing total_size",
                self.record_identity()
            ))
        })?;
        let total_size = u64::try_from(total_size).map_err(|_| {
            crate::Error::InternalError(format!(
                "repository converted package {} has negative total_size {total_size}",
                self.record_identity()
            ))
        })?;

        Ok(RepositoryConvertedArtifact {
            package_name,
            package_version,
            source_profile,
            profile_revision_sha256,
            package_architecture,
            repository_provides_digest,
            transport,
            total_size,
            content_hash,
            ccs_path,
        })
    }

    pub(super) fn required_repository_text<'a>(
        &self,
        field: &str,
        value: &'a Option<String>,
    ) -> Result<&'a str> {
        value
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                crate::Error::InternalError(format!(
                    "repository converted package {} is missing {field}",
                    self.record_identity()
                ))
            })
    }

    pub(super) fn validate_artifact_contract(&self) -> Result<()> {
        match self.artifact_kind {
            ConvertedArtifactKind::Installed => {
                self.installed_trove_id()?;
                if self.package_name.is_some()
                    || self.package_version.is_some()
                    || self.source_profile.is_some()
                    || self.profile_revision_sha256.is_some()
                    || self.package_architecture.is_some()
                    || self.repository_provides_digest.is_some()
                    || self.transport_json.is_some()
                    || self.total_size.is_some()
                    || self.content_hash.is_some()
                {
                    return Err(crate::Error::InternalError(format!(
                        "installed converted package {} carries repository-serving fields",
                        self.record_identity()
                    )));
                }
                if self.ccs_path.as_deref().is_some_and(str::is_empty) {
                    return Err(crate::Error::InternalError(format!(
                        "installed converted package {} carries an empty CCS path",
                        self.record_identity()
                    )));
                }
                Ok(())
            }
            ConvertedArtifactKind::Repository => {
                if self.trove_id.is_some() {
                    return Err(crate::Error::InternalError(format!(
                        "repository converted package {} carries an installed trove identity",
                        self.record_identity()
                    )));
                }
                self.repository_artifact().map(|_| ())
            }
        }
    }

    /// Attach the durable signed CCS output retained for an adopted package.
    pub fn set_installed_ccs_path(&mut self, path: String) -> Result<()> {
        if self.artifact_kind != ConvertedArtifactKind::Installed || path.is_empty() {
            return Err(crate::Error::InternalError(
                "installed CCS path requires a non-empty installed conversion".to_string(),
            ));
        }
        self.ccs_path = Some(path);
        Ok(())
    }
}
