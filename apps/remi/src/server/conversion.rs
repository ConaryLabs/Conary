// apps/remi/src/server/conversion.rs
//! Package conversion service for the Remi server
//!
//! Downloads legacy packages from upstream repositories and converts them
//! to CCS format, storing chunks in the CAS.

mod lookup;
mod metadata;
mod persistence;
mod safety;
mod storage;
#[cfg(test)]
mod test_support;
mod types;

use crate::server::R2Store;
use crate::server::conversion_timing::{
    ConversionPhase, ConversionPhaseTiming, ConversionSkippedPhase, ConversionTimingReport,
};
use crate::server::publication::ServerConversionOutcome;
use anyhow::{Context, Result, anyhow};
use conary_core::ccs::convert::{
    ConversionOptions, ConversionResult, LegacyConverter, ScriptletBundleSummary,
};
use conary_core::db::models::RepositoryPackage;
use conary_core::packages::common::PackageMetadata;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tempfile::TempDir;
use tracing::info;

use lookup::PackageDownloadRefresh;
use persistence::PersistConversionInput;

pub use types::{ConversionBenchmarkEvidence, ScriptletPackageMetadata, ServerConversionResult};

/// Conversion service for Remi
#[derive(Clone)]
pub struct ConversionService {
    /// Path to chunk storage
    chunk_dir: PathBuf,
    /// Path to cache/scratch directory
    cache_dir: PathBuf,
    /// Database path
    db_path: PathBuf,
    /// Optional R2 store for write-through
    r2_store: Option<Arc<R2Store>>,
}

struct ParsedConversion {
    metadata: PackageMetadata,
    format: &'static str,
    original_checksum: String,
    conversion_result: ConversionResult,
    repo_pkg: RepositoryPackage,
    phase_timings: Vec<ConversionPhaseTiming>,
    skipped_phases: Vec<ConversionSkippedPhase>,
}

impl ConversionService {
    pub fn new(
        chunk_dir: PathBuf,
        cache_dir: PathBuf,
        db_path: PathBuf,
        r2_store: Option<Arc<R2Store>>,
    ) -> Self {
        Self {
            chunk_dir,
            cache_dir,
            db_path,
            r2_store,
        }
    }

    /// Convert a package from a repository.
    pub async fn convert_package_async(
        &self,
        distro: &str,
        package_name: &str,
        version: Option<&str>,
        architecture: Option<&str>,
    ) -> Result<ServerConversionOutcome> {
        let mut timing = ConversionTimingReport::new(distro, package_name, version);
        let result = self
            .convert_package_async_inner(distro, package_name, version, architecture, &mut timing)
            .await;

        match result {
            Ok(mut result) => {
                timing.finish(true);
                Self::log_conversion_timing(&timing);
                result.result_mut().timing = Some(timing);
                Ok(result)
            }
            Err(err) => {
                timing.finish(false);
                Self::log_conversion_timing(&timing);
                Err(err)
            }
        }
    }

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

    async fn convert_package_async_inner(
        &self,
        distro: &str,
        package_name: &str,
        version: Option<&str>,
        architecture: Option<&str>,
        timing: &mut ConversionTimingReport,
    ) -> Result<ServerConversionOutcome> {
        // Refuse to convert critical system packages
        Self::ensure_package_name_not_critical(package_name)?;

        info!(
            "Converting package: {}:{} (version: {:?})",
            distro, package_name, version
        );

        let started = Instant::now();
        let repo_pkg = self
            .find_package_for_conversion_async(distro, package_name, version, architecture)
            .await?;
        timing.record(ConversionPhase::PackageLookup, started.elapsed());
        if timing.version.is_none() {
            timing.version = Some(repo_pkg.version.clone());
        }
        info!(
            "Found package: {} {} from repo {}",
            repo_pkg.name, repo_pkg.version, repo_pkg.repository_id
        );

        let cache_dir = self
            .cache_dir
            .canonicalize()
            .unwrap_or_else(|_| self.cache_dir.clone());
        let temp_dir = TempDir::new_in(&cache_dir).context("Failed to create temp directory")?;

        let started = Instant::now();
        let (repo_pkg, pkg_path) = self
            .download_package_with_refresh_async(PackageDownloadRefresh {
                distro,
                package_name,
                version,
                architecture,
                repo_pkg,
                dest_dir: temp_dir.path(),
            })
            .await
            .map_err(|e| anyhow!("Failed to download package: {}", e))?;
        timing.record(ConversionPhase::Download, started.elapsed());
        info!("Downloaded to: {:?}", pkg_path);

        let checksum_path = pkg_path.clone();
        let started = Instant::now();
        let original_checksum =
            tokio::task::spawn_blocking(move || Self::calculate_checksum(&checksum_path))
                .await
                .map_err(|e| anyhow!("checksum task panicked: {e}"))??;
        timing.record(ConversionPhase::Checksum, started.elapsed());

        let started = Instant::now();
        if let Some(existing) = self
            .cached_conversion_result_async(distro, &repo_pkg, &original_checksum)
            .await?
        {
            timing.record(ConversionPhase::CacheLookup, started.elapsed());
            Self::record_cache_hit_skips(timing);
            return Ok(existing);
        }
        timing.record(ConversionPhase::CacheLookup, started.elapsed());

        let parse_service = self.clone();
        let distro_owned = distro.to_string();
        let output_dir = temp_dir.path().join("output");
        let parsed = tokio::task::spawn_blocking(move || {
            parse_service.parse_and_convert_package(
                &distro_owned,
                repo_pkg,
                pkg_path,
                output_dir,
                original_checksum,
            )
        })
        .await
        .map_err(|e| anyhow!("conversion task panicked: {e}"))??;
        timing.phases.extend(parsed.phase_timings.clone());
        timing.skipped_phases.extend(parsed.skipped_phases.clone());

        let stored_chunks = self
            .store_chunks_with_timing(&parsed.conversion_result)
            .await?;
        timing.record(ConversionPhase::CasWrite, stored_chunks.cas_duration);
        if let Some(duration) = stored_chunks.r2_duration {
            timing.record(ConversionPhase::R2WriteThrough, duration);
        } else {
            timing.record_skipped(ConversionPhase::R2WriteThrough, "r2 store not configured");
        }
        info!("Stored {} chunks/blobs", stored_chunks.chunk_hashes.len());

        let persist_service = self.clone();
        let distro_owned = distro.to_string();
        let started = Instant::now();
        tokio::task::spawn_blocking(move || {
            persist_service.persist_conversion_result(PersistConversionInput {
                distro: distro_owned,
                metadata: parsed.metadata,
                format: parsed.format,
                original_checksum: parsed.original_checksum,
                conversion_result: parsed.conversion_result,
                repo_pkg: parsed.repo_pkg,
                chunk_hashes: stored_chunks.chunk_hashes,
            })
        })
        .await
        .map_err(|e| anyhow!("conversion persistence task panicked: {e}"))?
        .inspect(|_| timing.record(ConversionPhase::Persistence, started.elapsed()))
    }

    fn record_cache_hit_skips(timing: &mut ConversionTimingReport) {
        for phase in [
            ConversionPhase::ArchiveExtraction,
            ConversionPhase::NativeMetadataExtraction,
            ConversionPhase::Capture,
            ConversionPhase::AdapterDispatch,
            ConversionPhase::Chunking,
            ConversionPhase::CasWrite,
            ConversionPhase::R2WriteThrough,
            ConversionPhase::Persistence,
        ] {
            timing.record_skipped(phase, "cache hit; phase did not run");
        }
    }

    fn log_conversion_timing(timing: &ConversionTimingReport) {
        tracing::info!(
            target: "remi::conversion_timing",
            distro = %timing.distro,
            package = %timing.package,
            total_ms = timing.total_ms,
            success = timing.success,
            phases = %serde_json::to_string(&timing.phases)
                .unwrap_or_else(|_| "[]".to_string()),
            skipped_phases = %serde_json::to_string(&timing.skipped_phases)
                .unwrap_or_else(|_| "[]".to_string()),
            "conversion timing report"
        );
    }

    fn parse_and_convert_package(
        &self,
        distro: &str,
        repo_pkg: RepositoryPackage,
        pkg_path: PathBuf,
        output_dir: PathBuf,
        original_checksum: String,
    ) -> Result<ParsedConversion> {
        let mut phase_timings = Vec::new();
        let mut skipped_phases = Vec::new();

        let conn = conary_core::db::open(&self.db_path)?;
        let started = Instant::now();
        let (mut metadata, files, format) = self.parse_package(&pkg_path, distro)?;
        phase_timings.push(ConversionPhaseTiming {
            phase: ConversionPhase::ArchiveExtraction,
            duration_ms: started.elapsed().as_millis(),
        });

        let started = Instant::now();
        Self::apply_repository_identity(&mut metadata, &repo_pkg);
        Self::merge_repository_provides(&conn, &repo_pkg, &mut metadata)?;
        Self::ensure_metadata_not_critical(&metadata)?;
        info!(
            "Parsed: {} v{} ({} files, {} native provides)",
            metadata.name,
            metadata.version,
            files.len(),
            metadata.provides.len()
        );
        phase_timings.push(ConversionPhaseTiming {
            phase: ConversionPhase::NativeMetadataExtraction,
            duration_ms: started.elapsed().as_millis(),
        });

        std::fs::create_dir_all(&output_dir)?;
        let output_dir = output_dir.canonicalize().unwrap_or(output_dir);

        let options = ConversionOptions {
            enable_chunking: true,
            output_dir,
            auto_classify: true,
            ..Default::default()
        };

        let converter = LegacyConverter::new(options)
            .with_source_distro(distro)
            .with_conversion_tool("remi");
        if metadata.scriptlets.is_empty() {
            skipped_phases.push(ConversionSkippedPhase {
                phase: ConversionPhase::Capture,
                reason: "package has no native scriptlets to capture".to_string(),
            });
        } else {
            skipped_phases.push(ConversionSkippedPhase {
                phase: ConversionPhase::Capture,
                reason: "capture timing is included in legacy converter timing".to_string(),
            });
        }
        skipped_phases.push(ConversionSkippedPhase {
            phase: ConversionPhase::AdapterDispatch,
            reason: "adapter dispatch timing is included in legacy converter timing".to_string(),
        });

        let started = Instant::now();
        let conversion_result = converter
            .convert(&metadata, &files, format, &original_checksum)
            .map_err(|e| anyhow!("Conversion failed: {}", e))?;
        phase_timings.push(ConversionPhaseTiming {
            phase: ConversionPhase::Chunking,
            duration_ms: started.elapsed().as_millis(),
        });

        info!(
            "Conversion complete: fidelity={}",
            conversion_result.fidelity.level
        );

        Ok(ParsedConversion {
            metadata,
            format,
            original_checksum,
            conversion_result,
            repo_pkg,
            phase_timings,
            skipped_phases,
        })
    }

    /// Build a package from a recipe URL
    ///
    /// 1. Fetch the recipe from the URL
    /// 2. Parse and validate the recipe
    /// 3. Cook it using the Kitchen (with isolation)
    /// 4. Store chunks in CAS
    /// 5. Return the result
    pub async fn build_from_recipe(&self, recipe_url: &str) -> Result<ServerConversionResult> {
        use conary_core::recipe::{Kitchen, KitchenConfig, parse_recipe};

        info!("Building package from recipe: {}", recipe_url);

        // Step 1: Fetch recipe content
        let recipe_content = Self::fetch_url(recipe_url).await?;
        info!("Fetched recipe ({} bytes)", recipe_content.len());

        // Step 2: Parse and validate recipe
        let recipe =
            parse_recipe(&recipe_content).map_err(|e| anyhow!("Failed to parse recipe: {}", e))?;

        info!(
            "Recipe: {} version {}",
            recipe.package.name, recipe.package.version
        );

        // Step 3: Cook the recipe
        let temp_dir =
            TempDir::new_in(&self.cache_dir).context("Failed to create temp directory")?;

        let config = KitchenConfig {
            source_cache: self.cache_dir.join("sources"),
            use_isolation: true, // Always use isolation on server
            ..Default::default()
        };

        let kitchen = Kitchen::new(config);
        let cook_result = kitchen
            .cook(&recipe, temp_dir.path())
            .map_err(|e| anyhow!("Recipe cooking failed: {}", e))?;

        info!(
            "Cooked: {} ({} warnings)",
            cook_result.package_path.display(),
            cook_result.warnings.len()
        );

        // Step 4: Store chunks
        let ccs_data = tokio::fs::read(&cook_result.package_path)
            .await
            .context("Failed to read cooked CCS package")?;

        let content_hash = conary_core::hash::sha256(&ccs_data);

        // Copy CCS package to persistent location
        let ccs_filename = Self::safe_ccs_filename(&recipe.package.name, &recipe.package.version)?;
        let final_ccs_path = self.cache_dir.join("packages").join(&ccs_filename);

        if let Some(parent) = final_ccs_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::copy(&cook_result.package_path, &final_ccs_path).await?;

        // Extract chunk hashes from the CCS package
        // For now, we'll just report the package itself
        let chunk_hashes = vec![content_hash.clone()];
        let total_size = ccs_data.len() as u64;

        Ok(ServerConversionResult {
            name: recipe.package.name,
            version: recipe.package.version,
            distro: "recipe".to_string(),
            chunk_hashes,
            total_size,
            content_hash,
            ccs_path: final_ccs_path,
            cache_state: "recipe".to_string(),
            scriptlets: ScriptletPackageMetadata::from(&ScriptletBundleSummary::default()),
            publication: None,
            timing: None,
        })
    }

    /// Fetch content from a URL (with security validation)
    ///
    /// SECURITY: This function validates URLs and blocks requests to:
    /// - Private IP ranges (10.x, 172.16-31.x, 192.168.x, 127.x)
    /// - Link-local addresses (169.254.x)
    /// - Loopback addresses
    /// - IPv6 private/local addresses
    ///
    /// This prevents SSRF attacks where a malicious recipe URL could be used
    /// to probe internal services.
    async fn fetch_url(url: &str) -> Result<String> {
        // Parse URL to validate scheme and extract host
        let parsed_url =
            url::Url::parse(url).map_err(|e| anyhow!("Invalid URL '{}': {}", url, e))?;

        // Only allow https (http redirects to https, but we reject http-only)
        let scheme = parsed_url.scheme();
        if scheme != "https" && scheme != "http" {
            return Err(anyhow!("Only http/https URLs are allowed, got: {}", scheme));
        }

        // Extract host
        let host = parsed_url
            .host_str()
            .ok_or_else(|| anyhow!("URL '{}' has no host", url))?;

        // Check for prohibited hosts
        Self::validate_host(host)?;

        // Resolve DNS and validate the resolved IP. We pin the validated IP
        // using reqwest's `resolve()` so that reqwest connects to the exact
        // address we checked, closing the DNS rebinding TOCTOU gap.
        let port = parsed_url
            .port()
            .unwrap_or(if scheme == "https" { 443 } else { 80 });
        let resolved_ips: Vec<std::net::SocketAddr> =
            tokio::net::lookup_host(format!("{host}:{port}"))
                .await
                .map_err(|e| anyhow!("Failed to resolve '{}': {}", host, e))?
                .collect();

        // Check all resolved IPs - if ANY is private, reject
        for addr in &resolved_ips {
            Self::validate_ip(&addr.ip())?;
        }

        // Pin the first validated IP so reqwest uses it instead of
        // re-resolving (which could return a different, malicious IP).
        let pinned_ip = resolved_ips
            .first()
            .ok_or_else(|| anyhow!("DNS resolution for '{}' returned no addresses", host))?;

        // SECURITY: Disable automatic redirects entirely AND pin the
        // resolved IP. This closes both the redirect-based and
        // DNS-rebinding SSRF vectors.
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .resolve(host, *pinned_ip)
            .user_agent("conary-remi/0.1")
            .build()
            .map_err(|e| anyhow!("Failed to create HTTP client: {}", e))?;

        let response = client
            .get(url)
            .send()
            .await
            .map_err(|e| anyhow!("Failed to fetch '{}': {}", url, e))?;

        // Reject redirects -- the response status will be 3xx if the server
        // tried to redirect. Return a clear error instead of following.
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("<unknown>");
            return Err(anyhow!(
                "URL '{}' returned redirect ({}) to '{}'. \
                 Redirects are rejected to prevent SSRF.",
                url,
                response.status().as_u16(),
                location
            ));
        }

        if !response.status().is_success() {
            return Err(anyhow!(
                "HTTP {} fetching '{}': {}",
                response.status().as_u16(),
                url,
                response.status().canonical_reason().unwrap_or("Unknown")
            ));
        }

        response
            .text()
            .await
            .map_err(|e| anyhow!("Failed to read response body: {}", e))
    }

    /// Validate a hostname is not a private/internal address
    fn validate_host(host: &str) -> Result<()> {
        // Check for localhost aliases
        let lower_host = host.to_lowercase();
        if lower_host == "localhost"
            || lower_host.ends_with(".localhost")
            || lower_host == "127.0.0.1"
            || lower_host == "::1"
            || lower_host == "0.0.0.0"
        {
            return Err(anyhow!("Localhost URLs are not allowed"));
        }

        // Check for AWS/cloud metadata endpoints
        if lower_host == "169.254.169.254"
            || lower_host.contains("metadata")
            || lower_host == "metadata.google.internal"
        {
            return Err(anyhow!("Cloud metadata endpoints are not allowed"));
        }

        // Check for internal domain suffixes
        let internal_suffixes = [".internal", ".local", ".lan", ".home", ".corp"];
        for suffix in internal_suffixes {
            if lower_host.ends_with(suffix) {
                return Err(anyhow!("Internal domain '{}' is not allowed", host));
            }
        }

        Ok(())
    }

    /// Validate an IP address is not private/internal
    fn validate_ip(ip: &std::net::IpAddr) -> Result<()> {
        match ip {
            std::net::IpAddr::V4(ipv4) => {
                let octets = ipv4.octets();

                // Loopback: 127.0.0.0/8
                if octets[0] == 127 {
                    return Err(anyhow!("Loopback addresses are not allowed"));
                }

                // Private: 10.0.0.0/8
                if octets[0] == 10 {
                    return Err(anyhow!("Private IP range 10.x.x.x is not allowed"));
                }

                // Private: 172.16.0.0/12
                if octets[0] == 172 && (16..=31).contains(&octets[1]) {
                    return Err(anyhow!("Private IP range 172.16-31.x.x is not allowed"));
                }

                // Private: 192.168.0.0/16
                if octets[0] == 192 && octets[1] == 168 {
                    return Err(anyhow!("Private IP range 192.168.x.x is not allowed"));
                }

                // Link-local: 169.254.0.0/16 (includes AWS metadata)
                if octets[0] == 169 && octets[1] == 254 {
                    return Err(anyhow!("Link-local addresses are not allowed"));
                }

                // Broadcast: 255.255.255.255
                if octets == [255, 255, 255, 255] {
                    return Err(anyhow!("Broadcast addresses are not allowed"));
                }

                // Unspecified: 0.0.0.0
                if octets == [0, 0, 0, 0] {
                    return Err(anyhow!("Unspecified addresses are not allowed"));
                }

                Ok(())
            }
            std::net::IpAddr::V6(ipv6) => {
                // Loopback: ::1
                if ipv6.is_loopback() {
                    return Err(anyhow!("IPv6 loopback is not allowed"));
                }

                // Unspecified: ::
                if ipv6.is_unspecified() {
                    return Err(anyhow!("IPv6 unspecified is not allowed"));
                }

                // Private/ULA: fc00::/7
                let segments = ipv6.segments();
                if (segments[0] & 0xfe00) == 0xfc00 {
                    return Err(anyhow!("IPv6 unique local addresses are not allowed"));
                }

                // Link-local: fe80::/10
                if (segments[0] & 0xffc0) == 0xfe80 {
                    return Err(anyhow!("IPv6 link-local addresses are not allowed"));
                }

                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    #[test]
    fn remi_server_conversion_paths_do_not_block_on_async_work() {
        for relative_path in [
            "src/server/admin_service.rs",
            "src/server/conversion.rs",
            "src/server/handlers/packages.rs",
            "src/server/prewarm.rs",
        ] {
            let source = production_source_without_comments(relative_path);
            assert!(
                !source.contains(".block_on("),
                "{relative_path} must not call Handle::block_on in production Remi server paths"
            );
        }
    }

    // --- validate_host tests ---

    #[test]
    fn test_validate_host_allows_public() {
        assert!(ConversionService::validate_host("remi.conary.io").is_ok());
        assert!(ConversionService::validate_host("github.com").is_ok());
        assert!(ConversionService::validate_host("example.com").is_ok());
    }

    #[test]
    fn test_validate_host_blocks_localhost() {
        assert!(ConversionService::validate_host("localhost").is_err());
        assert!(ConversionService::validate_host("LOCALHOST").is_err());
        assert!(ConversionService::validate_host("sub.localhost").is_err());
        assert!(ConversionService::validate_host("127.0.0.1").is_err());
        assert!(ConversionService::validate_host("::1").is_err());
        assert!(ConversionService::validate_host("0.0.0.0").is_err());
    }

    #[test]
    fn test_validate_host_blocks_cloud_metadata() {
        assert!(ConversionService::validate_host("169.254.169.254").is_err());
        assert!(ConversionService::validate_host("metadata.google.internal").is_err());
    }

    #[test]
    fn test_validate_host_blocks_internal_domains() {
        assert!(ConversionService::validate_host("server.internal").is_err());
        assert!(ConversionService::validate_host("mybox.local").is_err());
        assert!(ConversionService::validate_host("router.lan").is_err());
        assert!(ConversionService::validate_host("nas.home").is_err());
        assert!(ConversionService::validate_host("ldap.corp").is_err());
    }

    // --- validate_ip tests ---

    #[test]
    fn test_validate_ip_allows_public_ipv4() {
        let ip: std::net::IpAddr = "8.8.8.8".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_ok());

        let ip: std::net::IpAddr = "46.4.33.93".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_ok());
    }

    #[test]
    fn test_validate_ip_blocks_loopback() {
        let ip: std::net::IpAddr = "127.0.0.1".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());

        let ip: std::net::IpAddr = "127.0.0.2".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());
    }

    #[test]
    fn test_validate_ip_blocks_private_10() {
        let ip: std::net::IpAddr = "10.0.0.1".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());

        let ip: std::net::IpAddr = "10.255.255.255".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());
    }

    #[test]
    fn test_validate_ip_blocks_private_172() {
        let ip: std::net::IpAddr = "172.16.0.1".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());

        let ip: std::net::IpAddr = "172.31.255.255".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());

        // 172.15.x.x is NOT private
        let ip: std::net::IpAddr = "172.15.0.1".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_ok());

        // 172.32.x.x is NOT private
        let ip: std::net::IpAddr = "172.32.0.1".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_ok());
    }

    #[test]
    fn test_validate_ip_blocks_private_192_168() {
        let ip: std::net::IpAddr = "192.168.0.1".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());

        let ip: std::net::IpAddr = "192.168.255.255".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());
    }

    #[test]
    fn test_validate_ip_blocks_link_local() {
        let ip: std::net::IpAddr = "169.254.169.254".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());

        let ip: std::net::IpAddr = "169.254.0.1".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());
    }

    #[test]
    fn test_validate_ip_blocks_broadcast() {
        let ip: std::net::IpAddr = "255.255.255.255".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());
    }

    #[test]
    fn test_validate_ip_blocks_unspecified() {
        let ip: std::net::IpAddr = "0.0.0.0".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());
    }

    #[test]
    fn test_validate_ip_allows_public_ipv6() {
        let ip: std::net::IpAddr = "2a01:4f8:221:350b::2".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_ok());

        let ip: std::net::IpAddr = "2001:4860:4860::8888".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_ok());
    }

    #[test]
    fn test_validate_ip_blocks_ipv6_loopback() {
        let ip: std::net::IpAddr = "::1".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());
    }

    #[test]
    fn test_validate_ip_blocks_ipv6_unspecified() {
        let ip: std::net::IpAddr = "::".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());
    }

    #[test]
    fn test_validate_ip_blocks_ipv6_ula() {
        let ip: std::net::IpAddr = "fc00::1".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());

        let ip: std::net::IpAddr = "fd12:3456:789a::1".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());
    }

    #[test]
    fn test_validate_ip_blocks_ipv6_link_local() {
        let ip: std::net::IpAddr = "fe80::1".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());
    }

    // --- ConversionService::new tests ---

    #[test]
    fn test_conversion_service_new() {
        let service = ConversionService::new(
            PathBuf::from("/chunks"),
            PathBuf::from("/cache"),
            PathBuf::from("/db.sqlite"),
            None,
        );
        assert_eq!(service.chunk_dir, PathBuf::from("/chunks"));
        assert_eq!(service.cache_dir, PathBuf::from("/cache"));
        assert_eq!(service.db_path, PathBuf::from("/db.sqlite"));
        assert!(service.r2_store.is_none());
    }
}
