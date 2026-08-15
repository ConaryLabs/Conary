// apps/remi/src/server/conversion/storage.rs
//! Signed CCS object persistence and optional R2 write-through.

use super::ConversionService;
use anyhow::{Context, Result};
use conary_core::ccs::convert::ConversionResult;
use conary_core::db::models::ChunkAccess;
use conary_core::filesystem::{CasStore, object_path};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

pub(super) struct StoredTransport {
    pub(super) transport: conary_core::ccs::CcsTransportEnvelopeV1,
    pub(super) verification_duration: Duration,
    pub(super) cas_duration: Duration,
    pub(super) r2_duration: Option<Duration>,
}

impl ConversionService {
    pub(super) async fn store_transport_with_timing(
        &self,
        result: &ConversionResult,
        source_profile: &str,
    ) -> Result<StoredTransport> {
        let package_path = result
            .package_path
            .as_deref()
            .context("conversion did not emit a CCS artifact")?;
        self.store_signed_ccs_path_with_timing(package_path, source_profile)
            .await
    }

    /// Verify the signed CCS authority, persist exactly its canonical objects,
    /// and publish only absent objects to remote storage.
    pub(super) async fn store_signed_ccs_path_with_timing(
        &self,
        package_path: &Path,
        source_profile: &str,
    ) -> Result<StoredTransport> {
        let objects_dir = self.chunk_dir.join("objects");
        let keys_dir = self.repository_keys_dir.as_ref().context(
            "Remi conversion requires release_publish.repository_keys_dir for CCS transport verification",
        )?;
        let signing_key = crate::server::signing_authority::load_role_key(
            keys_dir,
            source_profile,
            crate::server::signing_authority::RepositorySigningRole::Targets,
        )?;
        let policy = conary_core::ccs::TrustPolicy::strict(vec![signing_key.public_key_base64()]);

        let verify_started = Instant::now();
        let local_artifact = package_path.to_path_buf();
        let verification = tokio::task::spawn_blocking(move || {
            conary_core::ccs::verify::verify_package(&local_artifact, &policy)
        })
        .await
        .context("join emitted CCS transport verification task")??;
        let verification_duration = verify_started.elapsed();
        let transport =
            conary_core::ccs::CcsTransportEnvelopeV1::from_verified_archive(&verification)?;
        let exact_sizes = transport
            .objects
            .iter()
            .map(|object| {
                Ok((
                    object.sha256.clone(),
                    i64::try_from(object.size).context("CCS object size exceeds i64")?,
                ))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;

        let local_objects_dir = objects_dir.clone();
        let cas_started = Instant::now();
        tokio::task::spawn_blocking(move || {
            let cas =
                CasStore::new(local_objects_dir).context("initialize signed CCS object CAS")?;
            conary_core::ccs::transport::persist_verified_archive_objects(&verification, &cas)
                .map(|_| ())
        })
        .await
        .context("join signed CCS object persistence task")??;
        let cas_duration = cas_started.elapsed();

        // The upsert also refreshes the GC grace authority for CAS hits.
        self.persist_chunk_sizes(&exact_sizes).await?;

        let r2_duration = if let Some(r2) = self.r2_store.clone() {
            let started = Instant::now();
            for object in &transport.objects {
                if r2.head_chunk(&object.sha256).await? {
                    continue;
                }
                let path = object_path(&objects_dir, &object.sha256)?;
                let data = tokio::fs::read(&path)
                    .await
                    .with_context(|| format!("read signed CCS object {}", object.sha256))?;
                conary_core::hash::verify_sha256(&data, &object.sha256)?;
                r2.put_chunk(&object.sha256, &data).await?;
            }
            Some(started.elapsed())
        } else {
            None
        };

        Ok(StoredTransport {
            transport,
            verification_duration,
            cas_duration,
            r2_duration,
        })
    }

    async fn persist_chunk_sizes(&self, exact_sizes: &BTreeMap<String, i64>) -> Result<()> {
        if exact_sizes.is_empty() {
            return Ok(());
        }

        let exact_sizes = exact_sizes.clone();
        let db_path = self.db_path.clone();
        let writer = self.database_writer.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            writer.execute(|| {
                let mut conn = crate::server::open_runtime_db(&db_path)?;
                let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                for (hash, size) in exact_sizes {
                    if let Some(existing) = ChunkAccess::find_by_hash(&tx, &hash)?
                        && existing.size_bytes != size
                    {
                        anyhow::bail!(
                            "chunk {hash} size disagrees with persisted CAS authority: {} != {size}",
                            existing.size_bytes
                        );
                    }
                    ChunkAccess::new(hash, size).upsert(&tx)?;
                }
                tx.commit()?;
                Ok(())
            })
        })
        .await
        .context("join exact chunk-size persistence task")??;
        Ok(())
    }

    pub(super) fn calculate_sha256(path: &Path) -> Result<conary_core::hash::Hash> {
        let mut file = std::fs::File::open(path)?;
        Ok(conary_core::hash::hash_reader(
            conary_core::hash::HashAlgorithm::Sha256,
            &mut file,
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calculated_sha256_is_typed_and_serializes_with_algorithm() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.pkg");
        std::fs::write(&file_path, b"hello world").unwrap();

        let checksum = ConversionService::calculate_sha256(&file_path).unwrap();
        assert_eq!(
            checksum.to_prefixed_string(),
            "sha256:b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn calculate_sha256_rejects_a_missing_file() {
        assert!(ConversionService::calculate_sha256(Path::new("/nonexistent/file.pkg")).is_err());
    }
}
