// apps/remi/src/server/conversion/benchmark.rs
//! Conversion benchmark evidence.

use super::{
    CONVERSION_BENCHMARK_SCHEMA_V1, ConversionBenchmarkEnvironment, ConversionBenchmarkEvidence,
    ConversionBenchmarkSample, ConversionBenchmarkSampleClass, ConversionService,
};
use anyhow::{Context, Result, anyhow, ensure};
use conary_core::db::models::{ConvertedPackage, RepositoryPackage};
use std::collections::HashSet;

use crate::server::profile_catalog::ProfileCatalog;

const SIZE_CLASS_COUNT: usize = 3;

impl ConversionService {
    pub async fn benchmark_size_class_samples(
        &self,
        distro: &str,
    ) -> Result<Vec<ConversionBenchmarkSample>> {
        let db_path = self.db_path.clone();
        let catalog_authority = self.catalog_authority.clone().ok_or_else(|| {
            anyhow!("conversion benchmark requires an immutable profile catalog authority")
        })?;
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
            let pinned = catalog_authority
                .open_active_profile(profile.id())
                .with_context(|| {
                    format!(
                        "open active immutable catalog for benchmark profile '{}'",
                        profile.id()
                    )
                })?;
            let mut converted_inputs = HashSet::new();
            for converted in ConvertedPackage::find_current_conversions(
                &conn,
                pinned.profile_revision_sha256(),
                None,
            )? {
                let converted_id = converted
                    .id
                    .context("persisted repository conversion has no row identity")?;
                ConvertedPackage::require_conversion_pin(&conn, converted_id)?;
                let artifact = converted.repository_artifact()?;
                converted.scriptlet_summary()?;
                converted_inputs.insert((
                    artifact.package_name.to_string(),
                    artifact.package_version.to_string(),
                    artifact.package_architecture.to_string(),
                    converted.original_checksum,
                ));
            }
            let catalog_packages =
                ProfileCatalog::new(&pinned).downloadable_package_records(1)?;
            for package in &catalog_packages {
                ensure!(
                    package.source_profile == profile.id(),
                    "immutable catalog for '{}' contains package '{}' from profile '{}'",
                    profile.id(),
                    package.name,
                    package.source_profile
                );
            }
            let mut candidates = catalog_packages
                .into_iter()
                .filter(|package| {
                    package.architecture.is_some()
                        && !converted_inputs.contains(&(
                            package.name.clone(),
                            package.version.clone(),
                            package.architecture.clone().unwrap_or_default(),
                            package.checksum.clone(),
                        ))
                })
                .map(|package| ConversionBenchmarkSample {
                    class: ConversionBenchmarkSampleClass::Explicit,
                    package: package.name,
                    version: package.version,
                    architecture: package.architecture,
                    source_checksum: package.checksum,
                    source_size_bytes: package.size,
                })
                .collect::<Vec<_>>();
            candidates.sort_by(|left, right| {
                (
                    left.source_size_bytes,
                    &left.package,
                    &left.version,
                    &left.architecture,
                    &left.source_checksum,
                )
                    .cmp(&(
                        right.source_size_bytes,
                        &right.package,
                        &right.version,
                        &right.architecture,
                        &right.source_checksum,
                    ))
            });
            let count = candidates.len();
            ensure!(
                count >= SIZE_CLASS_COUNT,
                "benchmark route '{}' has only {count} unconverted positive-size package(s); three distinct size classes are required",
                profile.id()
            );

            let offsets = [0, count / 2, count - 1];
            let classes = [
                ConversionBenchmarkSampleClass::Small,
                ConversionBenchmarkSampleClass::Median,
                ConversionBenchmarkSampleClass::Large,
            ];
            Ok(offsets
                .into_iter()
                .zip(classes)
                .map(|(offset, class)| {
                    let mut sample = candidates[offset].clone();
                    sample.class = class;
                    sample
                })
                .collect())
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
        benchmark_sample_from_package(package.repo_pkg, ConversionBenchmarkSampleClass::Explicit)
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
    use super::super::test_support::create_test_db;
    use super::*;
    use crate::server::catalog_authority::test_support::{ActiveCatalogFixture, package};
    use std::path::PathBuf;

    fn catalog_service(fixture: &ActiveCatalogFixture) -> ConversionService {
        ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            fixture.db_path().to_path_buf(),
            None,
        )
        .with_catalog_authority(fixture.authority().clone())
    }

    #[tokio::test]
    async fn benchmark_size_class_samples_return_exact_unconverted_subjects() {
        let fixture = ActiveCatalogFixture::new();
        fixture.activate(
            "fedora-44",
            1,
            vec![
                package(
                    "fedora-44",
                    "small",
                    "1.0",
                    "1",
                    Some("x86_64"),
                    10,
                    "small-source",
                ),
                package(
                    "fedora-44",
                    "large",
                    "1.0",
                    "1",
                    Some("x86_64"),
                    200,
                    "large-source",
                ),
                package(
                    "fedora-44",
                    "medium",
                    "1.0",
                    "1",
                    Some("x86_64"),
                    100,
                    "medium-source",
                ),
            ],
        );
        let service = catalog_service(&fixture);

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
        let fixture = ActiveCatalogFixture::new();
        let already_converted = package(
            "fedora-44",
            "already-converted",
            "1.0",
            "1",
            Some("x86_64"),
            300,
            "already-converted-source",
        );
        let already_converted_checksum = already_converted.checksum.clone();
        let revision = fixture.activate(
            "fedora-44",
            1,
            vec![
                package(
                    "fedora-44",
                    "small",
                    "1.0",
                    "1",
                    Some("x86_64"),
                    10,
                    "small-source",
                ),
                package(
                    "fedora-44",
                    "medium",
                    "1.0",
                    "1",
                    Some("x86_64"),
                    100,
                    "medium-source",
                ),
                package(
                    "fedora-44",
                    "large",
                    "1.0",
                    "1",
                    Some("x86_64"),
                    200,
                    "large-source",
                ),
                already_converted,
            ],
        );
        let conn = fixture.connection();
        let transport = super::super::test_support::test_transport(&[]);
        let mut converted = ConvertedPackage::new_repository(
            "fedora-44".to_string(),
            revision,
            "already-converted".to_string(),
            "1.0".to_string(),
            "x86_64".to_string(),
            "rpm".to_string(),
            already_converted_checksum,
            &transport,
            1,
            "sha256:converted".to_string(),
            "/tmp/converted.ccs".to_string(),
            conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
        );
        converted.insert_with_conversion_pin(&conn, 1).unwrap();
        let service = catalog_service(&fixture);

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
        assert!(
            evidence
                .error
                .unwrap()
                .contains("immutable profile catalog authority")
        );
    }
}
