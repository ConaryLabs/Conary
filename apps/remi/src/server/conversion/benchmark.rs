// apps/remi/src/server/conversion/benchmark.rs
//! Conversion benchmark evidence.

use super::{
    CONVERSION_BENCHMARK_SCHEMA_V1, ConversionBenchmarkEnvironment, ConversionBenchmarkEvidence,
    ConversionBenchmarkSample, ConversionBenchmarkSampleClass, ConversionService,
};
use anyhow::{Context, Result, anyhow, ensure};
use conary_core::db::models::{CONVERSION_VERSION, RepositoryPackage};

const SIZE_CLASS_COUNT: i64 = 3;

impl ConversionService {
    pub async fn benchmark_size_class_samples(
        &self,
        distro: &str,
    ) -> Result<Vec<ConversionBenchmarkSample>> {
        let db_path = self.db_path.clone();
        let distro = distro.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = conary_core::db::open(&db_path)?;
            let profile =
                conary_core::repository::supported_profiles::profile_for_remi_route(&distro)
                    .ok_or_else(|| {
                        anyhow!(
                            "benchmark route '{distro}' does not map to exactly one public profile"
                        )
                    })?;
            let candidate_predicate =
                "FROM repository_packages rp
                 JOIN repositories r ON rp.repository_id = r.id
                 WHERE r.source_profile = ?1
                   AND rp.size > 0
                   AND rp.architecture IS NOT NULL
                   AND NOT EXISTS (
                       SELECT 1
                       FROM converted_packages cp
                       WHERE cp.artifact_kind = 'repository'
                         AND cp.source_profile = r.source_profile
                         AND cp.original_checksum = rp.checksum
                         AND cp.conversion_version = ?2
                   )";
            let count = conn.query_row(
                &format!("SELECT COUNT(*) {candidate_predicate}"),
                rusqlite::params![profile.id(), CONVERSION_VERSION],
                |row| row.get::<_, i64>(0),
            )?;
            ensure!(
                count >= SIZE_CLASS_COUNT,
                "benchmark route '{}' has only {count} unconverted positive-size package(s); three distinct size classes are required",
                profile.id()
            );

            let query = format!(
                "SELECT rp.name, rp.version, rp.architecture, rp.checksum, rp.size
                 {candidate_predicate}
                 ORDER BY rp.size, rp.name, rp.version, rp.architecture
                 LIMIT 1 OFFSET ?3"
            );
            let mut stmt = conn.prepare(&query)?;
            let offsets = [0_i64, count / 2, count - 1];
            let classes = [
                ConversionBenchmarkSampleClass::Small,
                ConversionBenchmarkSampleClass::Median,
                ConversionBenchmarkSampleClass::Large,
            ];
            offsets
                .into_iter()
                .zip(classes)
                .map(|(offset, class)| {
                    stmt.query_row(
                        rusqlite::params![profile.id(), CONVERSION_VERSION, offset],
                        |row| {
                            let size = row.get::<_, i64>(4)?;
                            let source_size_bytes = u64::try_from(size).map_err(|error| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    4,
                                    rusqlite::types::Type::Integer,
                                    Box::new(error),
                                )
                            })?;
                            Ok(ConversionBenchmarkSample {
                                class,
                                package: row.get(0)?,
                                version: row.get(1)?,
                                architecture: row.get(2)?,
                                source_checksum: row.get(3)?,
                                source_size_bytes,
                            })
                        },
                    )
                    .map_err(anyhow::Error::from)
                })
                .collect()
        })
        .await
        .map_err(|e| anyhow!("benchmark size-class sample task panicked: {e}"))?
    }

    pub async fn benchmark_explicit_sample(
        &self,
        distro: &str,
        package_name: &str,
    ) -> Result<ConversionBenchmarkSample> {
        let profile = conary_core::repository::supported_profiles::profile_for_remi_route(distro)
            .ok_or_else(|| {
            anyhow!("benchmark route '{distro}' does not map to exactly one public profile")
        })?;
        let package = self
            .find_package_for_conversion_async(profile.id(), package_name, None, None)
            .await?;
        benchmark_sample_from_package(package, ConversionBenchmarkSampleClass::Explicit)
    }

    pub async fn benchmark_package_conversion(
        &self,
        distro: &str,
        sample: &ConversionBenchmarkSample,
        iteration: usize,
        environment: &ConversionBenchmarkEnvironment,
    ) -> Result<ConversionBenchmarkEvidence> {
        match self
            .convert_package_async(
                distro,
                &sample.package,
                Some(&sample.version),
                sample.architecture.as_deref(),
            )
            .await
        {
            Ok(outcome) => {
                let result = outcome;
                let actual_source = result
                    .timing
                    .as_ref()
                    .and_then(|timing| timing.source.as_ref())
                    .context("conversion benchmark result omitted exact source identity")?;
                ensure!(
                    actual_source.version == sample.version
                        && actual_source.architecture == sample.architecture
                        && actual_source.checksum == sample.source_checksum
                        && actual_source.declared_size_bytes == sample.source_size_bytes,
                    "conversion benchmark source changed after sample selection"
                );
                Ok(ConversionBenchmarkEvidence {
                    schema_version: CONVERSION_BENCHMARK_SCHEMA_V1,
                    environment: environment.clone(),
                    sample: sample.clone(),
                    iteration,
                    distro: distro.to_string(),
                    package: sample.package.clone(),
                    version: Some(result.version),
                    cache_state: result.cache_state,
                    r2_configured: self.r2_store.is_some(),
                    timing: result.timing,
                    converted: true,
                    error: None,
                })
            }
            Err(err) => Ok(ConversionBenchmarkEvidence {
                schema_version: CONVERSION_BENCHMARK_SCHEMA_V1,
                environment: environment.clone(),
                sample: sample.clone(),
                iteration,
                distro: distro.to_string(),
                package: sample.package.clone(),
                version: Some(sample.version.clone()),
                cache_state: "error".to_string(),
                r2_configured: self.r2_store.is_some(),
                timing: None,
                converted: false,
                error: Some(err.to_string()),
            }),
        }
    }
}

fn benchmark_sample_from_package(
    package: RepositoryPackage,
    class: ConversionBenchmarkSampleClass,
) -> Result<ConversionBenchmarkSample> {
    Ok(ConversionBenchmarkSample {
        class,
        package: package.name,
        version: package.version,
        architecture: package.architecture,
        source_checksum: package.checksum,
        source_size_bytes: u64::try_from(package.size)
            .context("repository package size is negative")?,
    })
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{create_test_db, insert_package, insert_repo};
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn benchmark_size_class_samples_return_exact_unconverted_subjects() {
        let (temp_file, conn) = create_test_db();
        let repo_id = insert_repo(&conn, "fedora-base", "fedora");
        insert_package(&conn, repo_id, "small", "1.0", 10);
        insert_package(&conn, repo_id, "large", "1.0", 200);
        insert_package(&conn, repo_id, "medium", "1.0", 100);

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        let samples = service
            .benchmark_size_class_samples("fedora")
            .await
            .unwrap();
        assert_eq!(samples.len(), 3);
        assert_eq!(samples[0].class, ConversionBenchmarkSampleClass::Small);
        assert_eq!(samples[0].package, "small");
        assert_eq!(samples[0].source_size_bytes, 10);
        assert_eq!(samples[1].class, ConversionBenchmarkSampleClass::Median);
        assert_eq!(samples[1].package, "medium");
        assert_eq!(samples[2].class, ConversionBenchmarkSampleClass::Large);
        assert_eq!(samples[2].package, "large");
    }

    #[tokio::test]
    async fn benchmark_size_classes_exclude_current_conversions() {
        let (temp_file, conn) = create_test_db();
        let repo_id = insert_repo(&conn, "fedora-base", "fedora");
        insert_package(&conn, repo_id, "small", "1.0", 10);
        insert_package(&conn, repo_id, "medium", "1.0", 100);
        insert_package(&conn, repo_id, "large", "1.0", 200);
        insert_package(&conn, repo_id, "already-converted", "1.0", 300);
        conn.execute(
            "INSERT INTO converted_packages (
                 artifact_kind, original_format, original_checksum,
                 repository_provides_digest, conversion_version,
                 package_name, package_version, source_profile,
                 transport_json, total_size, content_hash, ccs_path,
                 package_architecture
             ) VALUES (
                 'repository', 'rpm', ?1, ?2, ?3, 'already-converted', '1.0',
                 'fedora-44', '{\"schema_version\":1,\"manifest_base64\":\"\",\"signature_json\":\"{}\",\"objects\":[]}',
                 1, 'sha256:converted', '/tmp/converted.ccs', 'x86_64'
             )",
            rusqlite::params![
                "sha256:already-converted-1.0",
                conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST,
                CONVERSION_VERSION
            ],
        )
        .unwrap();

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        let samples = service
            .benchmark_size_class_samples("fedora")
            .await
            .unwrap();
        assert_eq!(samples[2].package, "large");
    }

    #[tokio::test]
    async fn benchmark_package_conversion_returns_error_evidence_without_network() {
        let (temp_file, _conn) = create_test_db();
        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        let sample = ConversionBenchmarkSample {
            class: ConversionBenchmarkSampleClass::Explicit,
            package: "missing-package".to_string(),
            version: "1.0".to_string(),
            architecture: Some("x86_64".to_string()),
            source_checksum: "sha256:missing".to_string(),
            source_size_bytes: 1,
        };
        let environment = ConversionBenchmarkEnvironment::capture("fixture-runner".to_string());

        let evidence = service
            .benchmark_package_conversion("fedora", &sample, 1, &environment)
            .await
            .unwrap();

        assert!(!evidence.converted);
        assert_eq!(evidence.schema_version, CONVERSION_BENCHMARK_SCHEMA_V1);
        assert_eq!(evidence.sample, sample);
        assert_eq!(evidence.iteration, 1);
        assert_eq!(evidence.cache_state, "error");
        assert!(evidence.error.unwrap().contains("not found"));
    }
}
