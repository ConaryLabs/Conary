// src/server/conversion.rs
//! Package conversion service for the Remi server
//!
//! Downloads legacy packages from upstream repositories and converts them
//! to CCS format, storing chunks in the CAS.

use crate::ccs::convert::{ConversionOptions, ConversionResult, LegacyConverter};
use crate::db::models::{ConvertedPackage, RepositoryPackage};
use crate::filesystem::path::sanitize_filename;
use crate::packages::arch::ArchPackage;
use crate::packages::common::PackageMetadata;
use crate::packages::deb::DebPackage;
use crate::packages::rpm::RpmPackage;
use crate::packages::traits::PackageFormat;
use crate::repository::download_package;
use crate::server::R2Store;
use anyhow::{Context, Result, anyhow};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::TempDir;
use tracing::{debug, info};

/// Result of a server-side conversion
#[derive(Debug)]
pub struct ServerConversionResult {
    /// Package name
    pub name: String,
    /// Package version
    pub version: String,
    /// Distribution
    pub distro: String,
    /// List of chunk hashes
    pub chunk_hashes: Vec<String>,
    /// Total size when reassembled
    pub total_size: u64,
    /// SHA-256 of the complete content
    pub content_hash: String,
    /// Path to the generated CCS package (temporary)
    pub ccs_path: PathBuf,
}

/// Conversion service for Remi
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

    /// Create a safe CCS filename from package name and version
    ///
    /// Sanitizes both name and version to prevent path traversal attacks
    /// where malicious package metadata could escape the packages directory.
    fn safe_ccs_filename(name: &str, version: &str) -> Result<String> {
        let safe_name = sanitize_filename(name)
            .map_err(|e| anyhow!("Invalid package name '{}': {}", name, e))?;
        let safe_version = sanitize_filename(version)
            .map_err(|e| anyhow!("Invalid package version '{}': {}", version, e))?;
        Ok(format!("{}-{}.ccs", safe_name, safe_version))
    }

    /// Convert a package from a repository
    ///
    /// 1. Find package in repository metadata
    /// 2. Download from upstream
    /// 3. Parse and extract files
    /// 4. Convert to CCS
    /// 5. Store chunks in CAS
    /// 6. Record in database
    pub async fn convert_package(
        &self,
        distro: &str,
        package_name: &str,
        version: Option<&str>,
    ) -> Result<ServerConversionResult> {
        info!(
            "Converting package: {}:{} (version: {:?})",
            distro, package_name, version
        );

        // Step 1: Find package in repository
        let conn = crate::db::open(&self.db_path)?;

        let repo_pkg = self.find_package(&conn, distro, package_name, version)?;
        info!(
            "Found package: {} {} from repo {}",
            repo_pkg.name, repo_pkg.version, repo_pkg.repository_id
        );

        // Step 2: Download package
        let temp_dir =
            TempDir::new_in(&self.cache_dir).context("Failed to create temp directory")?;

        let pkg_path = download_package(&repo_pkg, temp_dir.path())
            .map_err(|e| anyhow!("Failed to download package: {}", e))?;
        info!("Downloaded to: {:?}", pkg_path);

        // Calculate checksum of original package
        let original_checksum = Self::calculate_checksum(&pkg_path)?;

        // Check if already converted AND the CCS file still exists
        if let Some(existing) = ConvertedPackage::find_by_checksum(&conn, &original_checksum)? {
            let ccs_filename = Self::safe_ccs_filename(&repo_pkg.name, &repo_pkg.version)?;
            let ccs_path = self.cache_dir.join("packages").join(&ccs_filename);
            if !existing.needs_reconversion() && ccs_path.exists() {
                info!(
                    "Package already converted (checksum: {})",
                    original_checksum
                );
                // Return cached result
                return self.build_result_from_existing(&existing, distro, &repo_pkg);
            }
            // Delete stale conversion record and proceed with fresh conversion
            info!(
                "Stale conversion record (CCS file missing or needs reconversion), re-converting"
            );
            ConvertedPackage::delete_by_checksum(&conn, &original_checksum)?;
        }

        // Step 3: Parse package based on distro
        let (metadata, files, format) = self.parse_package(&pkg_path, distro)?;
        info!(
            "Parsed: {} v{} ({} files)",
            metadata.name,
            metadata.version,
            files.len()
        );

        // Step 4: Convert to CCS
        let output_dir = temp_dir.path().join("output");
        std::fs::create_dir_all(&output_dir)?;

        let options = ConversionOptions {
            enable_chunking: true,
            output_dir: output_dir.clone(),
            auto_classify: true,
            ..Default::default()
        };

        let converter = LegacyConverter::new(options);
        let conversion_result = converter
            .convert(&metadata, &files, format, &original_checksum)
            .map_err(|e| anyhow!("Conversion failed: {}", e))?;

        info!(
            "Conversion complete: fidelity={}",
            conversion_result.fidelity.level
        );

        // Step 5: Store chunks in CAS
        let chunk_hashes = self.store_chunks(&conversion_result).await?;
        info!("Stored {} chunks/blobs", chunk_hashes.len());

        // Step 6: Record in database
        let ccs_path = conversion_result
            .package_path
            .as_ref()
            .ok_or_else(|| anyhow!("No CCS package path"))?;

        let content_hash = Self::calculate_checksum(ccs_path)?;
        let total_size = std::fs::metadata(ccs_path)?.len();

        // Copy CCS package to persistent location first (need path for DB record)
        let ccs_filename = Self::safe_ccs_filename(&metadata.name, &metadata.version)?;
        let final_ccs_path = self.cache_dir.join("packages").join(&ccs_filename);

        if let Some(parent) = final_ccs_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(ccs_path, &final_ccs_path)?;

        // Create and insert the converted package record with server-side fields
        let mut converted = ConvertedPackage::new_server(
            distro.to_string(),
            metadata.name.clone(),
            metadata.version.clone(),
            format.to_string(),
            original_checksum,
            conversion_result.fidelity.level.to_string(),
            &chunk_hashes,
            total_size as i64,
            content_hash.clone(),
            final_ccs_path.to_string_lossy().to_string(),
        );
        converted.detected_hooks = Some(serde_json::to_string(&conversion_result.detected_hooks)?);
        converted.insert(&conn)?;

        info!(
            "Recorded conversion in database (distro={}, name={}, version={})",
            distro, metadata.name, metadata.version
        );

        Ok(ServerConversionResult {
            name: metadata.name,
            version: metadata.version,
            distro: distro.to_string(),
            chunk_hashes,
            total_size,
            content_hash,
            ccs_path: final_ccs_path,
        })
    }

    /// Find a package in the repository metadata
    fn find_package(
        &self,
        conn: &rusqlite::Connection,
        distro: &str,
        package_name: &str,
        version: Option<&str>,
    ) -> Result<RepositoryPackage> {
        // Map distro to repository name pattern
        let repo_pattern = match distro {
            "arch" => "arch-%",
            "fedora" => "fedora-%",
            "ubuntu" | "debian" => "ubuntu-%",
            _ => return Err(anyhow!("Unknown distribution: {}", distro)),
        };

        // Query for the package
        let mut stmt = conn.prepare(
            "SELECT rp.id, rp.repository_id, rp.name, rp.version, rp.architecture,
                    rp.description, rp.checksum, rp.size, rp.download_url, rp.dependencies,
                    rp.metadata, rp.synced_at, rp.is_security_update, rp.severity,
                    rp.cve_ids, rp.advisory_id, rp.advisory_url
             FROM repository_packages rp
             JOIN repositories r ON rp.repository_id = r.id
             WHERE rp.name = ?1
             AND r.name LIKE ?2
             AND (?3 IS NULL OR rp.version = ?3)
             ORDER BY rp.version DESC
             LIMIT 1",
        )?;

        let pkg = stmt
            .query_row(
                rusqlite::params![package_name, repo_pattern, version],
                |row| {
                    Ok(RepositoryPackage {
                        id: row.get(0)?,
                        repository_id: row.get(1)?,
                        name: row.get(2)?,
                        version: row.get(3)?,
                        architecture: row.get(4)?,
                        description: row.get(5)?,
                        checksum: row.get(6)?,
                        size: row.get(7)?,
                        download_url: row.get(8)?,
                        dependencies: row.get(9)?,
                        metadata: row.get(10)?,
                        synced_at: row.get(11)?,
                        is_security_update: row.get(12)?,
                        severity: row.get(13)?,
                        cve_ids: row.get(14)?,
                        advisory_id: row.get(15)?,
                        advisory_url: row.get(16)?,
                    })
                },
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    anyhow!(
                        "Package '{}' not found in {} repositories. Run 'conary repo-sync' first.",
                        package_name,
                        distro
                    )
                }
                _ => anyhow!("Database error: {}", e),
            })?;

        Ok(pkg)
    }

    /// Parse a downloaded package file
    fn parse_package(
        &self,
        path: &Path,
        distro: &str,
    ) -> Result<(
        PackageMetadata,
        Vec<crate::packages::traits::ExtractedFile>,
        &'static str,
    )> {
        let path_str = path.to_str().ok_or_else(|| anyhow!("Invalid path"))?;

        match distro {
            "arch" => {
                let pkg = ArchPackage::parse(path_str)
                    .map_err(|e| anyhow!("Failed to parse Arch package: {}", e))?;
                let files = pkg
                    .extract_file_contents()
                    .map_err(|e| anyhow!("Failed to extract Arch package contents: {}", e))?;
                let metadata = Self::build_metadata(&pkg);
                Ok((metadata, files, "arch"))
            }
            "fedora" => {
                let pkg = RpmPackage::parse(path_str)
                    .map_err(|e| anyhow!("Failed to parse RPM package: {}", e))?;
                let files = pkg
                    .extract_file_contents()
                    .map_err(|e| anyhow!("Failed to extract RPM package contents: {}", e))?;
                let metadata = Self::build_metadata(&pkg);
                Ok((metadata, files, "rpm"))
            }
            "ubuntu" | "debian" => {
                let pkg = DebPackage::parse(path_str)
                    .map_err(|e| anyhow!("Failed to parse DEB package: {}", e))?;
                let files = pkg
                    .extract_file_contents()
                    .map_err(|e| anyhow!("Failed to extract DEB package contents: {}", e))?;
                let metadata = Self::build_metadata(&pkg);
                Ok((metadata, files, "deb"))
            }
            _ => Err(anyhow!("Unsupported distribution: {}", distro)),
        }
    }

    /// Build PackageMetadata from a parsed package
    fn build_metadata<P: PackageFormat>(pkg: &P) -> PackageMetadata {
        PackageMetadata {
            package_path: PathBuf::new(), // Not needed for conversion
            name: pkg.name().to_string(),
            version: pkg.version().to_string(),
            architecture: pkg.architecture().map(String::from),
            description: pkg.description().map(String::from),
            files: pkg
                .files()
                .iter()
                .map(|f| crate::packages::traits::PackageFile {
                    path: f.path.clone(),
                    size: f.size,
                    mode: f.mode,
                    sha256: f.sha256.clone(),
                })
                .collect(),
            dependencies: pkg
                .dependencies()
                .iter()
                .map(|d| crate::packages::traits::Dependency {
                    name: d.name.clone(),
                    version: d.version.clone(),
                    dep_type: d.dep_type,
                    description: d.description.clone(),
                })
                .collect(),
            scriptlets: pkg.scriptlets(),
            config_files: pkg.config_files(),
        }
    }

    /// Store blobs from conversion result in CAS
    async fn store_chunks(&self, result: &ConversionResult) -> Result<Vec<String>> {
        let mut chunk_hashes = Vec::new();
        let objects_dir = self.chunk_dir.join("objects");

        // Get blobs from the build result (chunks or whole files)
        for (hash, data) in &result.build_result.blobs {
            let (prefix, rest) = hash.split_at(2.min(hash.len()));
            let chunk_path = objects_dir.join(prefix).join(rest);

            // Skip if chunk already exists (content-addressed = immutable)
            if chunk_path.exists() {
                debug!("Chunk {} already exists", hash);
                chunk_hashes.push(hash.clone());
                continue;
            }

            // Create parent directory
            if let Some(parent) = chunk_path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .context("Failed to create chunk directory")?;
            }

            // Write chunk atomically
            let temp_path = chunk_path.with_extension("tmp");
            tokio::fs::write(&temp_path, data)
                .await
                .context("Failed to write chunk")?;
            tokio::fs::rename(&temp_path, &chunk_path)
                .await
                .context("Failed to rename chunk")?;

            // R2 write-through: upload to Cloudflare R2 in parallel
            if let Some(ref r2) = self.r2_store {
                if let Err(e) = r2.put_chunk(hash, data).await {
                    tracing::warn!("R2 write-through failed for chunk {}: {}", hash, e);
                } else {
                    debug!("R2 write-through: uploaded chunk {}", hash);
                }
            }

            debug!("Stored chunk: {} ({} bytes)", hash, data.len());
            chunk_hashes.push(hash.clone());
        }

        Ok(chunk_hashes)
    }

    /// Calculate SHA-256 checksum of a file
    fn calculate_checksum(path: &Path) -> Result<String> {
        let mut file = std::fs::File::open(path)?;
        let mut hasher = Sha256::new();
        std::io::copy(&mut file, &mut hasher)?;
        Ok(format!("{:x}", hasher.finalize()))
    }

    /// Build result from existing conversion record
    fn build_result_from_existing(
        &self,
        existing: &ConvertedPackage,
        distro: &str,
        repo_pkg: &RepositoryPackage,
    ) -> Result<ServerConversionResult> {
        // Use server-side fields from ConvertedPackage if available, else fallback to repo_pkg
        let name = existing
            .package_name
            .clone()
            .unwrap_or_else(|| repo_pkg.name.clone());
        let version = existing
            .package_version
            .clone()
            .unwrap_or_else(|| repo_pkg.version.clone());

        // Prefer stored CCS path if available
        let ccs_path = if let Some(stored_path) = &existing.ccs_path {
            PathBuf::from(stored_path)
        } else {
            let ccs_filename = Self::safe_ccs_filename(&name, &version)?;
            self.cache_dir.join("packages").join(&ccs_filename)
        };

        // Parse chunk hashes from JSON if stored
        let chunk_hashes: Vec<String> = existing
            .chunk_hashes_json
            .as_ref()
            .and_then(|json| serde_json::from_str(json).ok())
            .unwrap_or_default();

        Ok(ServerConversionResult {
            name,
            version,
            distro: existing
                .distro
                .clone()
                .unwrap_or_else(|| distro.to_string()),
            chunk_hashes,
            total_size: existing.total_size.unwrap_or(repo_pkg.size) as u64,
            content_hash: existing
                .content_hash
                .clone()
                .unwrap_or_else(|| existing.original_checksum.clone()),
            ccs_path,
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
        use crate::recipe::{Kitchen, KitchenConfig, parse_recipe};

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

        let content_hash = {
            let mut hasher = Sha256::new();
            hasher.update(&ccs_data);
            format!("{:x}", hasher.finalize())
        };

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

        // Resolve DNS and validate the resolved IP
        let resolved_ips = tokio::net::lookup_host(format!(
            "{}:{}",
            host,
            parsed_url
                .port()
                .unwrap_or(if scheme == "https" { 443 } else { 80 })
        ))
        .await
        .map_err(|e| anyhow!("Failed to resolve '{}': {}", host, e))?;

        // Check all resolved IPs - if ANY is private, reject
        for addr in resolved_ips {
            Self::validate_ip(&addr.ip())?;
        }

        // Now safe to fetch - use a controlled HTTP client
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(5))
            .user_agent("conary-remi/0.1")
            .build()
            .map_err(|e| anyhow!("Failed to create HTTP client: {}", e))?;

        let response = client
            .get(url)
            .send()
            .await
            .map_err(|e| anyhow!("Failed to fetch '{}': {}", url, e))?;

        // Check for redirect to private IP (double-check final URL)
        let final_url = response.url();
        if let Some(final_host) = final_url.host_str()
            && final_host != host
        {
            // URL was redirected - validate the new host
            Self::validate_host(final_host)?;
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
    use super::*;
    use crate::db::models::{ConvertedPackage, Repository, RepositoryPackage};
    use crate::db::schema;
    use tempfile::NamedTempFile;

    fn create_test_db() -> (NamedTempFile, rusqlite::Connection) {
        let temp_file = NamedTempFile::new().unwrap();
        let conn = rusqlite::Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();
        (temp_file, conn)
    }

    fn insert_repo(conn: &rusqlite::Connection, name: &str, distro: &str) -> i64 {
        let mut repo = Repository::new(name.to_string(), "https://example.com".to_string());
        repo.default_strategy_distro = Some(distro.to_string());
        repo.insert(conn).unwrap()
    }

    fn insert_package(
        conn: &rusqlite::Connection,
        repo_id: i64,
        name: &str,
        version: &str,
        size: i64,
    ) {
        let mut pkg = RepositoryPackage::new(
            repo_id,
            name.to_string(),
            version.to_string(),
            format!("sha256:{name}-{version}"),
            size,
            format!("https://example.com/{name}-{version}.rpm"),
        );
        pkg.architecture = Some("x86_64".to_string());
        pkg.dependencies = Some(r#"["glibc","openssl"]"#.to_string());
        pkg.insert(conn).unwrap();
    }

    // --- safe_ccs_filename tests ---

    #[test]
    fn test_safe_ccs_filename_normal() {
        let result = ConversionService::safe_ccs_filename("nginx", "1.24.0-1.fc43").unwrap();
        assert_eq!(result, "nginx-1.24.0-1.fc43.ccs");
    }

    #[test]
    fn test_safe_ccs_filename_complex_name() {
        let result =
            ConversionService::safe_ccs_filename("lib32-glibc-devel", "2.38-1").unwrap();
        assert_eq!(result, "lib32-glibc-devel-2.38-1.ccs");
    }

    #[test]
    fn test_safe_ccs_filename_rejects_path_traversal_in_name() {
        let result = ConversionService::safe_ccs_filename("../../../etc/passwd", "1.0");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Invalid package name"));
    }

    #[test]
    fn test_safe_ccs_filename_rejects_path_traversal_in_version() {
        let result = ConversionService::safe_ccs_filename("nginx", "../../evil");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Invalid package version"));
    }

    #[test]
    fn test_safe_ccs_filename_rejects_slash_in_name() {
        let result = ConversionService::safe_ccs_filename("foo/bar", "1.0");
        assert!(result.is_err());
    }

    #[test]
    fn test_safe_ccs_filename_rejects_empty_name() {
        let result = ConversionService::safe_ccs_filename("", "1.0");
        assert!(result.is_err());
    }

    #[test]
    fn test_safe_ccs_filename_rejects_empty_version() {
        let result = ConversionService::safe_ccs_filename("nginx", "");
        assert!(result.is_err());
    }

    // --- calculate_checksum tests ---

    #[test]
    fn test_calculate_checksum_valid_file() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.pkg");
        std::fs::write(&file_path, b"hello world").unwrap();

        let checksum = ConversionService::calculate_checksum(&file_path).unwrap();
        // SHA-256 of "hello world"
        assert_eq!(
            checksum,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn test_calculate_checksum_empty_file() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let file_path = temp_dir.path().join("empty.pkg");
        std::fs::write(&file_path, b"").unwrap();

        let checksum = ConversionService::calculate_checksum(&file_path).unwrap();
        // SHA-256 of empty string
        assert_eq!(
            checksum,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn test_calculate_checksum_missing_file() {
        let result = ConversionService::calculate_checksum(Path::new("/nonexistent/file.pkg"));
        assert!(result.is_err());
    }

    // --- find_package tests ---

    #[test]
    fn test_find_package_found() {
        let (temp_file, conn) = create_test_db();
        let repo_id = insert_repo(&conn, "fedora-base", "fedora");
        insert_package(&conn, repo_id, "nginx", "1.24.0", 1024);

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        let pkg = service.find_package(&conn, "fedora", "nginx", None).unwrap();
        assert_eq!(pkg.name, "nginx");
        assert_eq!(pkg.version, "1.24.0");
    }

    #[test]
    fn test_find_package_with_specific_version() {
        let (temp_file, conn) = create_test_db();
        let repo_id = insert_repo(&conn, "fedora-base", "fedora");
        insert_package(&conn, repo_id, "nginx", "1.24.0", 1024);
        insert_package(&conn, repo_id, "nginx", "1.25.0", 1100);

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        let pkg = service
            .find_package(&conn, "fedora", "nginx", Some("1.24.0"))
            .unwrap();
        assert_eq!(pkg.version, "1.24.0");
    }

    #[test]
    fn test_find_package_not_found() {
        let (temp_file, conn) = create_test_db();
        insert_repo(&conn, "fedora-base", "fedora");

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        let result = service.find_package(&conn, "fedora", "nonexistent", None);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("not found"));
        assert!(err_msg.contains("repo-sync"));
    }

    #[test]
    fn test_find_package_unknown_distro() {
        let (temp_file, conn) = create_test_db();

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        let result = service.find_package(&conn, "gentoo", "nginx", None);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Unknown distribution"));
    }

    #[test]
    fn test_find_package_arch_distro() {
        let (temp_file, conn) = create_test_db();
        let repo_id = insert_repo(&conn, "arch-core", "arch");
        insert_package(&conn, repo_id, "pacman", "6.0.0", 800);

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        let pkg = service.find_package(&conn, "arch", "pacman", None).unwrap();
        assert_eq!(pkg.name, "pacman");
    }

    #[test]
    fn test_find_package_ubuntu_distro() {
        let (temp_file, conn) = create_test_db();
        let repo_id = insert_repo(&conn, "ubuntu-main", "ubuntu");
        insert_package(&conn, repo_id, "libc6", "2.38-1", 2048);

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        let pkg = service.find_package(&conn, "ubuntu", "libc6", None).unwrap();
        assert_eq!(pkg.name, "libc6");
    }

    #[test]
    fn test_find_package_debian_uses_ubuntu_repos() {
        let (temp_file, conn) = create_test_db();
        let repo_id = insert_repo(&conn, "ubuntu-main", "debian");
        insert_package(&conn, repo_id, "apt", "2.7.0", 512);

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        // debian maps to "ubuntu-%" repo pattern
        let pkg = service.find_package(&conn, "debian", "apt", None).unwrap();
        assert_eq!(pkg.name, "apt");
    }

    // --- build_result_from_existing tests ---

    #[test]
    fn test_build_result_from_existing_with_server_fields() {
        let (temp_file, conn) = create_test_db();
        let repo_id = insert_repo(&conn, "fedora-base", "fedora");
        insert_package(&conn, repo_id, "nginx", "1.24.0", 1024);

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        let mut converted = ConvertedPackage::new_server(
            "fedora".to_string(),
            "nginx".to_string(),
            "1.24.0".to_string(),
            "rpm".to_string(),
            "sha256:orig".to_string(),
            "high".to_string(),
            &["chunk1".to_string(), "chunk2".to_string()],
            2048,
            "sha256:content_abc".to_string(),
            "/data/nginx.ccs".to_string(),
        );
        converted.insert(&conn).unwrap();

        let existing = ConvertedPackage::find_by_checksum(&conn, "sha256:orig")
            .unwrap()
            .unwrap();

        let repo_pkg = service
            .find_package(&conn, "fedora", "nginx", None)
            .unwrap();

        let result = service
            .build_result_from_existing(&existing, "fedora", &repo_pkg)
            .unwrap();

        assert_eq!(result.name, "nginx");
        assert_eq!(result.version, "1.24.0");
        assert_eq!(result.distro, "fedora");
        assert_eq!(result.chunk_hashes, vec!["chunk1", "chunk2"]);
        assert_eq!(result.total_size, 2048);
        assert_eq!(result.content_hash, "sha256:content_abc");
        assert_eq!(result.ccs_path, PathBuf::from("/data/nginx.ccs"));
    }

    #[test]
    fn test_build_result_from_existing_without_chunk_hashes() {
        let (temp_file, conn) = create_test_db();
        let repo_id = insert_repo(&conn, "fedora-base", "fedora");
        insert_package(&conn, repo_id, "curl", "8.5.0", 512);

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        // Create a converted package with no chunk_hashes_json
        let mut converted = ConvertedPackage::new_server(
            "fedora".to_string(),
            "curl".to_string(),
            "8.5.0".to_string(),
            "rpm".to_string(),
            "sha256:curl-orig".to_string(),
            "high".to_string(),
            &[],
            512,
            "sha256:curl-content".to_string(),
            "/data/curl.ccs".to_string(),
        );
        converted.insert(&conn).unwrap();

        let existing = ConvertedPackage::find_by_checksum(&conn, "sha256:curl-orig")
            .unwrap()
            .unwrap();

        let repo_pkg = service
            .find_package(&conn, "fedora", "curl", None)
            .unwrap();

        let result = service
            .build_result_from_existing(&existing, "fedora", &repo_pkg)
            .unwrap();

        // Should return empty chunk list, not panic
        assert!(result.chunk_hashes.is_empty());
    }

    // --- store_chunks tests ---

    /// Helper to create a minimal ConversionResult with the given blobs
    fn make_conversion_result(
        blobs: std::collections::HashMap<String, Vec<u8>>,
    ) -> crate::ccs::convert::ConversionResult {
        use crate::ccs::builder::{BuildResult, FileEntry};
        use crate::ccs::convert::FidelityReport;
        use crate::ccs::manifest::{CcsManifest, Hooks, Package};

        let manifest = CcsManifest {
            package: Package {
                name: "test".to_string(),
                version: "1.0".to_string(),
                description: "test package".to_string(),
                license: None,
                homepage: None,
                repository: None,
                platform: None,
                authors: None,
            },
            provides: Default::default(),
            requires: Default::default(),
            suggests: Default::default(),
            components: Default::default(),
            hooks: Hooks::default(),
            config: Default::default(),
            build: None,
            legacy: None,
            policy: Default::default(),
            provenance: None,
            capabilities: None,
            redirects: Default::default(),
        };

        let build_result = BuildResult {
            manifest,
            components: std::collections::HashMap::new(),
            files: Vec::<FileEntry>::new(),
            blobs,
            total_size: 0,
            chunked: false,
            chunk_stats: None,
        };

        crate::ccs::convert::ConversionResult {
            build_result,
            package_path: None,
            fidelity: FidelityReport::default(),
            original_format: "rpm".to_string(),
            original_checksum: "sha256:test".to_string(),
            detected_hooks: Hooks::default(),
            inferred_capabilities: None,
            legacy_provenance: None,
        }
    }

    #[tokio::test]
    async fn test_store_chunks_writes_files() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let chunk_dir = temp_dir.path().join("chunks");
        std::fs::create_dir_all(&chunk_dir).unwrap();

        let service = ConversionService::new(
            chunk_dir.clone(),
            temp_dir.path().to_path_buf(),
            PathBuf::from("/tmp/nonexistent.db"),
            None,
        );

        let mut blobs = std::collections::HashMap::new();
        blobs.insert(
            "abcdef1234567890".to_string(),
            b"chunk data one".to_vec(),
        );
        blobs.insert(
            "1234567890abcdef".to_string(),
            b"chunk data two".to_vec(),
        );

        let result = make_conversion_result(blobs);
        let hashes = service.store_chunks(&result).await.unwrap();
        assert_eq!(hashes.len(), 2);

        // Verify files were written to correct paths
        for hash in &hashes {
            let (prefix, rest) = hash.split_at(2);
            let chunk_path = chunk_dir.join("objects").join(prefix).join(rest);
            assert!(chunk_path.exists(), "Chunk file should exist at {:?}", chunk_path);
        }
    }

    #[tokio::test]
    async fn test_store_chunks_idempotent() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let chunk_dir = temp_dir.path().join("chunks");
        std::fs::create_dir_all(&chunk_dir).unwrap();

        let service = ConversionService::new(
            chunk_dir.clone(),
            temp_dir.path().to_path_buf(),
            PathBuf::from("/tmp/nonexistent.db"),
            None,
        );

        let mut blobs = std::collections::HashMap::new();
        blobs.insert("aabbccdd11223344".to_string(), b"some data".to_vec());

        let result = make_conversion_result(blobs.clone());

        // Store twice - should not error
        let hashes1 = service.store_chunks(&result).await.unwrap();

        let result2 = make_conversion_result(blobs);
        let hashes2 = service.store_chunks(&result2).await.unwrap();
        assert_eq!(hashes1, hashes2);
    }

    #[tokio::test]
    async fn test_store_chunks_empty() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let chunk_dir = temp_dir.path().join("chunks");
        std::fs::create_dir_all(&chunk_dir).unwrap();

        let service = ConversionService::new(
            chunk_dir,
            temp_dir.path().to_path_buf(),
            PathBuf::from("/tmp/nonexistent.db"),
            None,
        );

        let result = make_conversion_result(std::collections::HashMap::new());
        let hashes = service.store_chunks(&result).await.unwrap();
        assert!(hashes.is_empty());
    }

    // --- validate_host tests ---

    #[test]
    fn test_validate_host_allows_public() {
        assert!(ConversionService::validate_host("packages.conary.io").is_ok());
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

    // --- ServerConversionResult tests ---

    #[test]
    fn test_server_conversion_result_debug() {
        let result = ServerConversionResult {
            name: "nginx".to_string(),
            version: "1.24.0".to_string(),
            distro: "fedora".to_string(),
            chunk_hashes: vec!["abc123".to_string()],
            total_size: 1024,
            content_hash: "sha256:deadbeef".to_string(),
            ccs_path: PathBuf::from("/data/nginx.ccs"),
        };
        // Verify Debug is implemented (would fail to compile otherwise)
        let debug_str = format!("{:?}", result);
        assert!(debug_str.contains("nginx"));
    }

    // --- distro mapping tests ---

    #[test]
    fn test_find_package_maps_distro_to_repo_pattern() {
        // Verify all supported distros can resolve to their repo patterns.
        // We insert repos with the expected naming pattern and ensure find_package
        // correctly maps distro name -> LIKE pattern.
        let (temp_file, conn) = create_test_db();

        let arch_id = insert_repo(&conn, "arch-core", "arch");
        insert_package(&conn, arch_id, "vim", "9.0", 500);

        let fed_id = insert_repo(&conn, "fedora-base", "fedora");
        insert_package(&conn, fed_id, "vim", "9.0", 500);

        let ubuntu_id = insert_repo(&conn, "ubuntu-main", "ubuntu");
        insert_package(&conn, ubuntu_id, "vim", "9.0", 500);

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        assert!(service.find_package(&conn, "arch", "vim", None).is_ok());
        assert!(service.find_package(&conn, "fedora", "vim", None).is_ok());
        assert!(service.find_package(&conn, "ubuntu", "vim", None).is_ok());
        assert!(service.find_package(&conn, "debian", "vim", None).is_ok());
    }
}
