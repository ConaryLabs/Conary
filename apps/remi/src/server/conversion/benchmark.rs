// apps/remi/src/server/conversion/benchmark.rs
//! Conversion benchmark and scan-only scriptlet corpus evidence.

use super::lookup::PackageDownloadRefresh;
use super::{ConversionBenchmarkEvidence, ConversionService};
use anyhow::{Context, Result, anyhow};
use tempfile::TempDir;

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
            let distro_filter = match distro.as_str() {
                "fedora" => "fedora",
                "ubuntu" => "ubuntu",
                "debian" => "debian",
                "arch" => "arch",
                _ => return Err(anyhow!("Unknown distribution: {}", distro)),
            };
            let mut stmt = conn.prepare(
                "SELECT DISTINCT rp.name
                 FROM repository_packages rp
                 JOIN repositories r ON rp.repository_id = r.id
                 WHERE COALESCE(r.default_strategy_distro, rp.distro, r.name) LIKE ?1
                 AND rp.size > 0
                 ORDER BY rp.size DESC
                 LIMIT ?2",
            )?;
            let pattern = format!("{distro_filter}%");
            let names = stmt
                .query_map(rusqlite::params![pattern, limit as i64], |row| row.get(0))?
                .collect::<Result<Vec<String>, _>>()?;
            Ok(names)
        })
        .await
        .map_err(|e| anyhow!("benchmark package sample task panicked: {e}"))?
    }

    pub async fn scan_package_scriptlets(
        &self,
        distro: &str,
        package_name: &str,
        version: Option<&str>,
        architecture: Option<&str>,
    ) -> Result<ConversionBenchmarkEvidence> {
        // Goal 0 accepts full package downloads for scriptlet-only scanning so the
        // evidence path reuses existing parsers. Before production-scale corpus
        // scans, optimize this with ranged reads for RPM headers and DEB control
        // archives.
        let repo_pkg = self
            .find_package_for_conversion_async(distro, package_name, version, architecture)
            .await?;
        let cache_dir = self
            .cache_dir
            .canonicalize()
            .unwrap_or_else(|_| self.cache_dir.clone());
        let temp_dir = TempDir::new_in(&cache_dir).context("Failed to create temp directory")?;
        let (repo_pkg, pkg_path) = self
            .download_package_with_refresh_async(PackageDownloadRefresh {
                distro,
                package_name,
                version,
                architecture,
                repo_pkg,
                dest_dir: temp_dir.path(),
            })
            .await?;
        let package_version = repo_pkg.version.clone();
        let service = self.clone();
        let distro_owned = distro.to_string();
        let package_owned = package_name.to_string();
        let summary_result = tokio::task::spawn_blocking(
            move || -> Result<crate::server::scriptlet_corpus::ScriptletCorpusSummary> {
                let (mut metadata, _files, _format) =
                    service.parse_package(&pkg_path, &distro_owned)?;
                Self::apply_repository_identity(&mut metadata, &repo_pkg);
                Ok(
                    crate::server::scriptlet_corpus::ScriptletCorpusSummary::from_scriptlets(
                        &distro_owned,
                        &package_owned,
                        &metadata.scriptlets,
                    ),
                )
            },
        )
        .await
        .map_err(|e| anyhow!("scriptlet scan task panicked: {e}"))?;
        let summary = summary_result?;

        Ok(ConversionBenchmarkEvidence {
            distro: distro.to_string(),
            package: package_name.to_string(),
            version: Some(package_version),
            scan_only: true,
            cache_state: "scan-only".to_string(),
            r2_configured: self.r2_store.is_some(),
            timing: None,
            scriptlet_summary: Some(summary),
            converted: false,
            error: None,
        })
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
                let result = outcome.into_result();
                Ok(ConversionBenchmarkEvidence {
                    distro: distro.to_string(),
                    package: package_name.to_string(),
                    version: Some(result.version),
                    scan_only: false,
                    cache_state: result.cache_state,
                    r2_configured: self.r2_store.is_some(),
                    timing: result.timing,
                    scriptlet_summary: None,
                    converted: true,
                    error: None,
                })
            }
            Err(err) => Ok(ConversionBenchmarkEvidence {
                distro: distro.to_string(),
                package: package_name.to_string(),
                version: version.map(ToString::to_string),
                scan_only: false,
                cache_state: "error".to_string(),
                r2_configured: self.r2_store.is_some(),
                timing: None,
                scriptlet_summary: None,
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
