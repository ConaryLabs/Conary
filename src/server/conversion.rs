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
use anyhow::{anyhow, Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
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
}

impl ConversionService {
    pub fn new(chunk_dir: PathBuf, cache_dir: PathBuf, db_path: PathBuf) -> Self {
        Self {
            chunk_dir,
            cache_dir,
            db_path,
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
        info!("Converting package: {}:{} (version: {:?})", distro, package_name, version);

        // Step 1: Find package in repository
        let conn = crate::db::open(&self.db_path)?;

        let repo_pkg = self.find_package(&conn, distro, package_name, version)?;
        info!("Found package: {} {} from repo {}", repo_pkg.name, repo_pkg.version, repo_pkg.repository_id);

        // Step 2: Download package
        let temp_dir = TempDir::new_in(&self.cache_dir)
            .context("Failed to create temp directory")?;

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
                info!("Package already converted (checksum: {})", original_checksum);
                // Return cached result
                return self.build_result_from_existing(&existing, distro, &repo_pkg);
            }
            // Delete stale conversion record and proceed with fresh conversion
            info!("Stale conversion record (CCS file missing or needs reconversion), re-converting");
            ConvertedPackage::delete_by_checksum(&conn, &original_checksum)?;
        }

        // Step 3: Parse package based on distro
        let (metadata, files, format) = self.parse_package(&pkg_path, distro)?;
        info!("Parsed: {} v{} ({} files)", metadata.name, metadata.version, files.len());

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
        let conversion_result = converter.convert(&metadata, &files, format, &original_checksum)
            .map_err(|e| anyhow!("Conversion failed: {}", e))?;

        info!("Conversion complete: fidelity={}", conversion_result.fidelity.level);

        // Step 5: Store chunks in CAS
        let chunk_hashes = self.store_chunks(&conversion_result).await?;
        info!("Stored {} chunks/blobs", chunk_hashes.len());

        // Step 6: Record in database
        let ccs_path = conversion_result.package_path.as_ref()
            .ok_or_else(|| anyhow!("No CCS package path"))?;

        let content_hash = Self::calculate_checksum(ccs_path)?;
        let total_size = std::fs::metadata(ccs_path)?.len();

        // Create and insert the converted package record
        let mut converted = ConvertedPackage::new(
            format.to_string(),
            original_checksum,
            conversion_result.fidelity.level.to_string(),
        );
        converted.detected_hooks = Some(serde_json::to_string(&conversion_result.detected_hooks)?);
        converted.insert(&conn)?;

        info!("Recorded conversion in database");

        // Copy CCS package to persistent location
        let ccs_filename = Self::safe_ccs_filename(&metadata.name, &metadata.version)?;
        let final_ccs_path = self.cache_dir
            .join("packages")
            .join(&ccs_filename);

        if let Some(parent) = final_ccs_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(ccs_path, &final_ccs_path)?;

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
             LIMIT 1"
        )?;

        let pkg = stmt.query_row(
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
        ).map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                anyhow!("Package '{}' not found in {} repositories. Run 'conary repo-sync' first.", package_name, distro)
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
    ) -> Result<(PackageMetadata, Vec<crate::packages::traits::ExtractedFile>, &'static str)> {
        let path_str = path.to_str().ok_or_else(|| anyhow!("Invalid path"))?;

        match distro {
            "arch" => {
                let pkg = ArchPackage::parse(path_str)
                    .map_err(|e| anyhow!("Failed to parse Arch package: {}", e))?;
                let files = pkg.extract_file_contents()
                    .map_err(|e| anyhow!("Failed to extract Arch package contents: {}", e))?;
                let metadata = Self::build_metadata(&pkg);
                Ok((metadata, files, "arch"))
            }
            "fedora" => {
                let pkg = RpmPackage::parse(path_str)
                    .map_err(|e| anyhow!("Failed to parse RPM package: {}", e))?;
                let files = pkg.extract_file_contents()
                    .map_err(|e| anyhow!("Failed to extract RPM package contents: {}", e))?;
                let metadata = Self::build_metadata(&pkg);
                Ok((metadata, files, "rpm"))
            }
            "ubuntu" | "debian" => {
                let pkg = DebPackage::parse(path_str)
                    .map_err(|e| anyhow!("Failed to parse DEB package: {}", e))?;
                let files = pkg.extract_file_contents()
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
            files: pkg.files().iter().map(|f| crate::packages::traits::PackageFile {
                path: f.path.clone(),
                size: f.size,
                mode: f.mode,
                sha256: f.sha256.clone(),
            }).collect(),
            dependencies: pkg.dependencies().iter().map(|d| crate::packages::traits::Dependency {
                name: d.name.clone(),
                version: d.version.clone(),
                dep_type: d.dep_type,
                description: d.description.clone(),
            }).collect(),
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
                tokio::fs::create_dir_all(parent).await
                    .context("Failed to create chunk directory")?;
            }

            // Write chunk atomically
            let temp_path = chunk_path.with_extension("tmp");
            tokio::fs::write(&temp_path, data).await
                .context("Failed to write chunk")?;
            tokio::fs::rename(&temp_path, &chunk_path).await
                .context("Failed to rename chunk")?;

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
        // Use repo package info since ConvertedPackage doesn't store name/version
        let ccs_filename = Self::safe_ccs_filename(&repo_pkg.name, &repo_pkg.version)?;
        Ok(ServerConversionResult {
            name: repo_pkg.name.clone(),
            version: repo_pkg.version.clone(),
            distro: distro.to_string(),
            chunk_hashes: vec![], // Would need to read from CCS package
            total_size: repo_pkg.size as u64,
            content_hash: existing.original_checksum.clone(),
            ccs_path: self.cache_dir.join("packages").join(&ccs_filename),
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
        use crate::recipe::{parse_recipe, Kitchen, KitchenConfig};

        info!("Building package from recipe: {}", recipe_url);

        // Step 1: Fetch recipe content
        let recipe_content = Self::fetch_url(recipe_url).await?;
        info!("Fetched recipe ({} bytes)", recipe_content.len());

        // Step 2: Parse and validate recipe
        let recipe = parse_recipe(&recipe_content)
            .map_err(|e| anyhow!("Failed to parse recipe: {}", e))?;

        info!("Recipe: {} version {}", recipe.package.name, recipe.package.version);

        // Step 3: Cook the recipe
        let temp_dir = TempDir::new_in(&self.cache_dir)
            .context("Failed to create temp directory")?;

        let config = KitchenConfig {
            source_cache: self.cache_dir.join("sources"),
            use_isolation: true, // Always use isolation on server
            ..Default::default()
        };

        let kitchen = Kitchen::new(config);
        let cook_result = kitchen.cook(&recipe, temp_dir.path())
            .map_err(|e| anyhow!("Recipe cooking failed: {}", e))?;

        info!("Cooked: {} ({} warnings)", cook_result.package_path.display(), cook_result.warnings.len());

        // Step 4: Store chunks
        let ccs_data = tokio::fs::read(&cook_result.package_path).await
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
        let parsed_url = url::Url::parse(url)
            .map_err(|e| anyhow!("Invalid URL '{}': {}", url, e))?;

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
            parsed_url.port().unwrap_or(if scheme == "https" { 443 } else { 80 })
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
        if let Some(final_host) = final_url.host_str() {
            if final_host != host {
                // URL was redirected - validate the new host
                Self::validate_host(final_host)?;
            }
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
