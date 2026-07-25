// apps/remi/src/server/conversion/benchmark.rs
//! Conversion benchmark evidence.

use super::{ConversionBenchmarkEvidence, ConversionService};
use anyhow::{Result, anyhow};

impl ConversionService {
    pub async fn benchmark_package_sample(
        &self,
        distro: &str,
        limit: usize,
    ) -> Result<Vec<String>> {
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
            let mut stmt = conn.prepare(
                "SELECT DISTINCT rp.name
                 FROM repository_packages rp
                 JOIN repositories r ON rp.repository_id = r.id
                 WHERE r.default_strategy_distro = ?1
                 AND rp.size > 0
                 ORDER BY rp.size DESC
                 LIMIT ?2",
            )?;
            let names = stmt
                .query_map(rusqlite::params![profile.id(), limit as i64], |row| {
                    row.get(0)
                })?
                .collect::<Result<Vec<String>, _>>()?;
            Ok(names)
        })
        .await
        .map_err(|e| anyhow!("benchmark package sample task panicked: {e}"))?
    }

    pub async fn benchmark_package_conversion(
        &self,
        distro: &str,
        package_name: &str,
        version: Option<&str>,
        architecture: Option<&str>,
    ) -> Result<ConversionBenchmarkEvidence> {
        match self
            .convert_package_async(distro, package_name, version, architecture)
            .await
        {
            Ok(outcome) => {
                let result = outcome;
                Ok(ConversionBenchmarkEvidence {
                    distro: distro.to_string(),
                    package: package_name.to_string(),
                    version: Some(result.version),
                    cache_state: result.cache_state,
                    r2_configured: self.r2_store.is_some(),
                    timing: result.timing,
                    converted: true,
                    error: None,
                })
            }
            Err(err) => Ok(ConversionBenchmarkEvidence {
                distro: distro.to_string(),
                package: package_name.to_string(),
                version: version.map(ToString::to_string),
                cache_state: "error".to_string(),
                r2_configured: self.r2_store.is_some(),
                timing: None,
                converted: false,
                error: Some(err.to_string()),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{create_test_db, insert_package, insert_repo};
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn benchmark_package_sample_returns_largest_repository_packages_for_distro() {
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

        let names = service.benchmark_package_sample("fedora", 2).await.unwrap();
        assert_eq!(names, vec!["large".to_string(), "medium".to_string()]);
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

        let evidence = service
            .benchmark_package_conversion("fedora", "missing-package", None, None)
            .await
            .unwrap();

        assert!(!evidence.converted);
        assert_eq!(evidence.cache_state, "error");
        assert!(evidence.error.unwrap().contains("not found"));
    }
}
