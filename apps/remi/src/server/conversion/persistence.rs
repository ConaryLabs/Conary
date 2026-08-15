// apps/remi/src/server/conversion/persistence.rs
//! Converted-package persistence and cache-hit reconstruction.

use super::metadata::repository_package_conversion_inputs_match;
use super::{ConversionService, ScriptletPackageMetadata, ServerConversionResult};
use anyhow::{Context, Result, anyhow, ensure};
use conary_core::ccs::convert::ConversionResult;
use conary_core::ccs::convert::ForeignConversionInput;
use conary_core::db::models::{ConvertedPackage, RepositoryPackage, RepositoryProvide};
use rusqlite::{Connection, TransactionBehavior};
use std::path::PathBuf;
use tracing::info;

pub(super) struct PersistConversionInput {
    pub(super) source_profile: String,
    pub(super) metadata: ForeignConversionInput,
    pub(super) format: &'static str,
    pub(super) source_checksum: String,
    pub(super) conversion_result: ConversionResult,
    pub(super) repo_pkg: RepositoryPackage,
    pub(super) repository_provides_digest: String,
    pub(super) transport: conary_core::ccs::CcsTransportEnvelopeV1,
}

enum CacheInspection {
    Current(Box<ServerConversionResult>),
    Missing,
    Stale,
}

enum PersistOutcome {
    Existing(Box<ServerConversionResult>),
    Inserted,
}

impl ConversionService {
    fn inspect_cached_conversion(
        &self,
        conn: &Connection,
        source_profile: &str,
        repo_pkg: &RepositoryPackage,
        source_checksum: &str,
        repository_provides_digest: &str,
    ) -> Result<CacheInspection> {
        let repository_package_id = repo_pkg
            .id
            .context("repository conversion package has no persisted identity")?;
        let current_package = RepositoryPackage::find_by_id(conn, repository_package_id)?
            .context("repository package metadata changed during conversion cache lookup")?;
        ensure!(
            repository_package_conversion_inputs_match(repo_pkg, &current_package),
            "repository package identity changed during conversion cache lookup"
        );
        let Some(existing) =
            ConvertedPackage::find_repository_by_checksum(conn, source_profile, source_checksum)?
        else {
            return Ok(CacheInspection::Missing);
        };

        let artifact = existing.repository_artifact()?;
        let expected_architecture = repo_pkg
            .architecture
            .as_deref()
            .context("repository conversion package has no exact architecture")?;
        let artifact_matches_package = artifact.source_profile == source_profile
            && artifact.package_name == repo_pkg.name
            && artifact.package_version == repo_pkg.version
            && artifact.package_architecture == expected_architecture;
        let metadata_is_current = existing.repository_metadata_is_current(conn)?;
        let ccs_path = PathBuf::from(artifact.ccs_path);
        if metadata_is_current {
            ensure!(
                artifact_matches_package,
                "current conversion checksum maps to a conflicting repository package identity"
            );
        }
        if metadata_is_current
            && artifact.repository_provides_digest == repository_provides_digest
            && ccs_path.exists()
        {
            return Ok(CacheInspection::Current(Box::new(
                self.build_result_from_existing(&existing)?,
            )));
        }

        Ok(CacheInspection::Stale)
    }

    pub(super) async fn cached_conversion_result_async(
        &self,
        source_profile: &str,
        repo_pkg: &RepositoryPackage,
        source_checksum: &str,
        repository_provides_digest: &str,
    ) -> Result<Option<ServerConversionResult>> {
        let service = self.clone();
        let source_profile = source_profile.to_string();
        let repo_pkg = repo_pkg.clone();
        let source_checksum = source_checksum.to_string();
        let repository_provides_digest = repository_provides_digest.to_string();

        tokio::task::spawn_blocking(move || {
            let mut conn = crate::server::open_runtime_db(&service.db_path)?;
            let tx = conn.transaction()?;
            let inspection = service.inspect_cached_conversion(
                &tx,
                &source_profile,
                &repo_pkg,
                &source_checksum,
                &repository_provides_digest,
            )?;
            tx.commit()?;
            match inspection {
                CacheInspection::Current(result) => {
                    info!("Package already converted (checksum: {})", source_checksum);
                    return Ok(Some(*result));
                }
                CacheInspection::Missing => return Ok(None),
                CacheInspection::Stale => {}
            }

            let writer = service.database_writer.clone();
            writer.execute(|| {
                let mut conn = crate::server::open_runtime_db(&service.db_path)?;
                let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let inspection = service.inspect_cached_conversion(
                    &tx,
                    &source_profile,
                    &repo_pkg,
                    &source_checksum,
                    &repository_provides_digest,
                )?;
                let result = match inspection {
                    CacheInspection::Current(result) => Some(*result),
                    CacheInspection::Missing => None,
                    CacheInspection::Stale => {
                        info!(
                            "Stale conversion record (CCS file missing or conversion input changed), re-converting"
                        );
                        ConvertedPackage::delete_repository_by_checksum(
                            &tx,
                            &source_profile,
                            &source_checksum,
                        )?;
                        None
                    }
                };
                tx.commit()?;
                Ok(result)
            })
        })
        .await
        .map_err(|e| anyhow!("conversion cache lookup task panicked: {e}"))?
    }

    pub(super) fn persist_conversion_result(
        &self,
        input: PersistConversionInput,
    ) -> Result<ServerConversionResult> {
        let PersistConversionInput {
            source_profile,
            metadata,
            format,
            source_checksum,
            conversion_result,
            repo_pkg,
            repository_provides_digest,
            transport,
        } = input;

        let ccs_path = conversion_result
            .package_path
            .as_ref()
            .ok_or_else(|| anyhow!("No CCS package path"))?;

        let content_hash = Self::calculate_sha256(ccs_path)?;
        let content_hash_text = content_hash.to_prefixed_string();
        let total_size = std::fs::metadata(ccs_path)?.len();
        let persisted_total_size =
            i64::try_from(total_size).context("converted CCS size exceeds SQLite INTEGER range")?;

        let package_architecture = repo_pkg
            .architecture
            .clone()
            .or_else(|| metadata.architecture().map(str::to_string))
            .ok_or_else(|| anyhow!("converted package has no exact architecture identity"))?;
        let ccs_filename = format!("{}.ccs", content_hash.as_str());
        let final_ccs_path = self.cache_dir.join("packages").join(&ccs_filename);

        if let Some(parent) = final_ccs_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(ccs_path, &final_ccs_path)?;

        let mut converted = ConvertedPackage::new_repository(
            source_profile.clone(),
            metadata.name().to_string(),
            metadata.version().to_string(),
            package_architecture.clone(),
            format.to_string(),
            source_checksum.clone(),
            &transport,
            persisted_total_size,
            content_hash_text.clone(),
            final_ccs_path.to_string_lossy().to_string(),
            repository_provides_digest,
        );
        converted.set_scriptlet_metadata(&conversion_result.scriptlet_metadata)?;
        let writer = self.database_writer.clone();
        let outcome = writer.execute(|| {
            let mut conn = crate::server::open_runtime_db(&self.db_path)?;
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

            match self.inspect_cached_conversion(
                &tx,
                &source_profile,
                &repo_pkg,
                &source_checksum,
                converted.repository_artifact()?.repository_provides_digest,
            )? {
                CacheInspection::Current(result) => {
                    tx.commit()?;
                    return Ok(PersistOutcome::Existing(result));
                }
                CacheInspection::Stale => {
                    ConvertedPackage::delete_repository_by_checksum(
                        &tx,
                        &source_profile,
                        &source_checksum,
                    )?;
                }
                CacheInspection::Missing => {}
            }

            let repository_package_id = repo_pkg
                .id
                .context("converted repository package has no persisted identity")?;
            let current_package = RepositoryPackage::find_by_id(&tx, repository_package_id)?
                .context("repository package metadata changed while conversion was running")?;
            ensure!(
                repository_package_conversion_inputs_match(&repo_pkg, &current_package),
                "repository package identity changed while conversion was running"
            );
            let current_repository_provides_digest =
                RepositoryProvide::conversion_capabilities_digest(&tx, repository_package_id)?;
            ensure!(
                current_repository_provides_digest
                    == converted.repository_artifact()?.repository_provides_digest,
                "repository provide authority changed while conversion was running"
            );
            ensure!(
                converted.repository_metadata_is_current(&tx)?,
                "repository source metadata changed while conversion was running"
            );
            converted.insert(&tx)?;
            tx.commit()?;
            Ok(PersistOutcome::Inserted)
        })?;
        if let PersistOutcome::Existing(result) = outcome {
            return Ok(*result);
        }

        info!(
            "Recorded conversion in database (source_profile={}, name={}, version={})",
            source_profile,
            metadata.name(),
            metadata.version()
        );

        let scriptlet_summary = converted.scriptlet_summary()?;
        Ok(ServerConversionResult {
            name: metadata.name().to_string(),
            version: metadata.version().to_string(),
            source_profile: Some(source_profile),
            transport,
            total_size,
            content_hash: content_hash_text,
            ccs_path: final_ccs_path,
            cache_state: "cold".to_string(),
            scriptlets: ScriptletPackageMetadata::from(&scriptlet_summary),
            timing: None,
        })
    }

    /// Build a result from a current conversion record.
    fn build_result_from_existing(
        &self,
        existing: &ConvertedPackage,
    ) -> Result<ServerConversionResult> {
        let artifact = existing.repository_artifact()?;
        let scriptlet_summary = existing.scriptlet_summary()?;
        Ok(ServerConversionResult {
            name: artifact.package_name.to_string(),
            version: artifact.package_version.to_string(),
            source_profile: Some(artifact.source_profile.to_string()),
            transport: artifact.transport,
            total_size: artifact.total_size,
            content_hash: artifact.content_hash.to_string(),
            ccs_path: PathBuf::from(artifact.ccs_path),
            cache_state: "hot".to_string(),
            scriptlets: ScriptletPackageMetadata::from(&scriptlet_summary),
            timing: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{create_test_db, insert_repo};
    use super::*;
    use conary_core::db::models::RepositoryProvide;
    use conary_core::repository::versioning::VersionScheme;

    fn cache_fixture(
        bind_current_digest: bool,
    ) -> (
        tempfile::NamedTempFile,
        tempfile::TempDir,
        ConversionService,
        RepositoryPackage,
        String,
        String,
    ) {
        cache_fixture_for_source(
            bind_current_digest,
            "fedora-base",
            "fedora",
            "rpm",
            VersionScheme::Rpm,
            "sha256:systemd-udev-259.5-1.fc44",
        )
    }

    fn cache_fixture_for_source(
        bind_current_digest: bool,
        repository_name: &str,
        profile: &str,
        format: &str,
        version_scheme: VersionScheme,
        source_checksum: &str,
    ) -> (
        tempfile::NamedTempFile,
        tempfile::TempDir,
        ConversionService,
        RepositoryPackage,
        String,
        String,
    ) {
        let (database, conn) = create_test_db();
        let repository_id = insert_repo(&conn, repository_name, profile);
        let exact_profile =
            conary_core::repository::supported_profiles::profile_by_public_id(profile)
                .or_else(|| {
                    conary_core::repository::supported_profiles::profile_for_remi_route(profile)
                })
                .unwrap()
                .id();
        let mut package = RepositoryPackage::new(
            repository_id,
            "systemd-udev".to_string(),
            "259.5-1.fc44".to_string(),
            version_scheme,
            source_checksum.to_string(),
            42,
            "https://example.com/systemd-udev.pkg".to_string(),
        );
        package.architecture = Some("x86_64".to_string());
        package.source_profile = Some(exact_profile.to_string());
        package.insert(&conn).unwrap();
        let package = RepositoryPackage::find_by_name(&conn, "systemd-udev")
            .unwrap()
            .pop()
            .unwrap();
        let package_id = package.id.unwrap();
        let source_checksum = package.checksum.clone();
        let mut provide = RepositoryProvide::new(
            package_id,
            "/usr/bin/kernel-install".to_string(),
            None,
            "file".to_string(),
            Some("/usr/bin/kernel-install".to_string()),
            version_scheme,
        );
        provide.insert(&conn).unwrap();
        let current_digest =
            RepositoryProvide::conversion_capabilities_digest(&conn, package_id).unwrap();

        let storage = tempfile::tempdir().unwrap();
        let cache_dir = storage.path().join("cache");
        let ccs_path = cache_dir.join("packages/existing.ccs");
        std::fs::create_dir_all(ccs_path.parent().unwrap()).unwrap();
        std::fs::write(&ccs_path, b"existing").unwrap();
        let converted_digest = if bind_current_digest {
            current_digest.clone()
        } else {
            conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string()
        };
        let transport = crate::server::conversion::test_support::test_transport(&[]);
        let mut converted = ConvertedPackage::new_repository(
            exact_profile.to_string(),
            "systemd-udev".to_string(),
            "259.5-1.fc44".to_string(),
            "x86_64".to_string(),
            format.to_string(),
            source_checksum.clone(),
            &transport,
            8,
            "sha256:content".to_string(),
            ccs_path.to_string_lossy().to_string(),
            converted_digest,
        );
        converted.insert(&conn).unwrap();
        drop(conn);

        let service = ConversionService::new(
            storage.path().join("chunks"),
            cache_dir,
            database.path().to_path_buf(),
            None,
        );
        (
            database,
            storage,
            service,
            package,
            source_checksum,
            current_digest,
        )
    }

    #[tokio::test]
    async fn cache_hit_requires_the_current_repository_provide_digest() {
        let (_database, _storage, service, package, source_checksum, current_digest) =
            cache_fixture(false);

        let result = service
            .cached_conversion_result_async(
                "fedora-44",
                &package,
                &source_checksum,
                &current_digest,
            )
            .await
            .unwrap();

        assert!(result.is_none());
        let conn = conary_core::db::open(&service.db_path).unwrap();
        assert!(
            ConvertedPackage::find_repository_by_checksum(&conn, "fedora-44", &source_checksum)
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn cache_miss_remains_read_only_while_another_writer_is_active() {
        let (_database, _storage, service, package, source_checksum, current_digest) =
            cache_fixture(true);
        let conn = conary_core::db::open(&service.db_path).unwrap();
        ConvertedPackage::delete_repository_by_checksum(&conn, "fedora-44", &source_checksum)
            .unwrap();
        drop(conn);

        let db_path = service.db_path.clone();
        let (locked_tx, locked_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let holder = std::thread::spawn(move || {
            let mut conn = conary_core::db::open(&db_path).unwrap();
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            locked_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            tx.commit().unwrap();
        });
        locked_rx.recv().unwrap();

        let lookup = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            service.cached_conversion_result_async(
                "fedora-44",
                &package,
                &source_checksum,
                &current_digest,
            ),
        )
        .await;
        release_tx.send(()).unwrap();
        holder.join().unwrap();

        assert!(
            lookup
                .expect("cache miss must not wait for the SQLite writer")
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn cache_hit_reuses_an_exact_metadata_bound_artifact() {
        let (_database, _storage, service, package, source_checksum, current_digest) =
            cache_fixture(true);

        let result = service
            .cached_conversion_result_async(
                "fedora-44",
                &package,
                &source_checksum,
                &current_digest,
            )
            .await
            .unwrap()
            .expect("exact conversion cache hit");

        assert_eq!(result.cache_state, "hot");
        assert_eq!(result.name, "systemd-udev");
    }

    #[tokio::test]
    async fn cache_hit_preserves_eopkg_sha1_source_authority() {
        let source_checksum = "sha1:1826421aded2a344b7864ffff2fae2430778b1f0";
        let (_database, _storage, service, package, persisted_checksum, current_digest) =
            cache_fixture_for_source(
                true,
                "solus-polaris",
                "solus",
                "eopkg",
                VersionScheme::Eopkg,
                source_checksum,
            );

        let result = service
            .cached_conversion_result_async("solus", &package, &persisted_checksum, &current_digest)
            .await
            .unwrap()
            .expect("exact SHA-1 conversion cache hit");

        assert_eq!(persisted_checksum, source_checksum);
        assert_eq!(result.source_profile.as_deref(), Some("solus"));
        assert_eq!(result.cache_state, "hot");
    }

    #[tokio::test]
    async fn cache_hit_rejects_a_checksum_alias_with_conflicting_package_identity() {
        let (_database, _storage, service, package, source_checksum, current_digest) =
            cache_fixture(true);
        let conn = conary_core::db::open(&service.db_path).unwrap();
        let mut alias = RepositoryPackage::new(
            package.repository_id,
            "aliased-systemd".to_string(),
            package.version.clone(),
            VersionScheme::Rpm,
            source_checksum.clone(),
            package.size,
            "https://example.com/aliased-systemd.rpm".to_string(),
        );
        alias.architecture = package.architecture.clone();
        alias.insert(&conn).unwrap();
        let alias_id = alias.id.unwrap();
        let mut provide = RepositoryProvide::new(
            alias_id,
            "/usr/bin/kernel-install".to_string(),
            None,
            "file".to_string(),
            Some("/usr/bin/kernel-install".to_string()),
            VersionScheme::Rpm,
        );
        provide.insert(&conn).unwrap();
        assert_eq!(
            RepositoryProvide::conversion_capabilities_digest(&conn, alias_id).unwrap(),
            current_digest
        );
        drop(conn);

        let error = service
            .cached_conversion_result_async("fedora-44", &alias, &source_checksum, &current_digest)
            .await
            .expect_err("one checksum cannot alias two current package identities")
            .to_string();

        assert!(error.contains("conflicting repository package identity"));
        let conn = conary_core::db::open(&service.db_path).unwrap();
        assert!(
            ConvertedPackage::find_repository_by_checksum(&conn, "fedora-44", &source_checksum)
                .unwrap()
                .is_some(),
            "the current artifact for the original identity must not be deleted"
        );
    }
}
