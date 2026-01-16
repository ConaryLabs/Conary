// src/server/conversion.rs
//! Package conversion service for the Refinery server
//!
//! Downloads legacy packages from upstream repositories and converts them
//! to CCS format, storing chunks in the CAS.

use crate::ccs::convert::{ConversionOptions, ConversionResult, LegacyConverter};
use crate::db::models::{ConvertedPackage, RepositoryPackage};
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

/// Conversion service for the Refinery
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
            let ccs_path = self.cache_dir.join("packages").join(format!("{}-{}.ccs", repo_pkg.name, repo_pkg.version));
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
        let final_ccs_path = self.cache_dir
            .join("packages")
            .join(format!("{}-{}.ccs", metadata.name, metadata.version));

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
        Ok(ServerConversionResult {
            name: repo_pkg.name.clone(),
            version: repo_pkg.version.clone(),
            distro: distro.to_string(),
            chunk_hashes: vec![], // Would need to read from CCS package
            total_size: repo_pkg.size as u64,
            content_hash: existing.original_checksum.clone(),
            ccs_path: self.cache_dir.join("packages").join(format!("{}-{}.ccs", repo_pkg.name, repo_pkg.version)),
        })
    }
}
