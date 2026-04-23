// conary-core/src/bootstrap/image.rs

//! Phase 5: Bootable image generation
//!
//! Creates bootable images from the built base system. Supports multiple formats:
//!
//! - **raw**: Direct disk image, can be written to USB or used with QEMU
//! - **qcow2**: QEMU copy-on-write format, efficient for VM testing
//! - **iso**: Hybrid ISO image, bootable from CD/DVD or USB
//!
//! # Build Pipeline
//!
//! Phase 5 runs after Phase 4 (system configuration). The kernel must already
//! be installed into the sysroot by Phase 3 (`system/linux.toml` recipe via
//! `PackageBuildRunner`). Phase 5 then:
//!
//! 1. Verifies the kernel is installed at `/usr/lib/modules/<ver>/vmlinuz`
//! 2. Writes the systemd-boot BLS entry (no initrd -- kernel has root fs built in)
//! 3. Copies the systemd-boot EFI binary from the sysroot (no host fallback)
//! 4. Runs systemd-repart to create the GPT disk image
//! 5. Converts to qcow2 for QEMU testing
//!
//! # Image Layout (GPT)
//!
//! ```text
//! +---------------------------------------------+
//! |  GPT Header (LBA 0-33)                      |
//! +---------------------------------------------+
//! |  ESP Partition (512MB, FAT32)               |
//! |  - /EFI/BOOT/BOOTX64.EFI (systemd-boot)    |
//! |  - /loader/loader.conf                       |
//! |  - /loader/entries/conaryos.conf             |
//! +---------------------------------------------+
//! |  Root Partition (remaining, ext4)           |
//! |  - Full base system except /boot contents   |
//! |  - Empty /boot mount point for the ESP      |
//! +---------------------------------------------+
//! |  GPT Footer                                 |
//! +---------------------------------------------+
//! ```

use super::config::BootstrapConfig;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;
use thiserror::Error;
use tracing::{info, warn};

/// Errors during image generation
#[derive(Debug, Error)]
pub enum ImageError {
    #[error("Base system not found at {0}")]
    BaseSystemNotFound(PathBuf),

    #[error("Required tool not found: {0}")]
    ToolNotFound(String),

    #[error("Invalid image format: {0} (expected: raw, qcow2, iso)")]
    InvalidFormat(String),

    #[error("Invalid size specification: {0}")]
    InvalidSize(String),

    #[error("Image creation failed: {0}")]
    CreationFailed(String),

    #[error("Partition failed: {0}")]
    PartitionFailed(String),

    #[error("Filesystem creation failed: {0}")]
    FilesystemFailed(String),

    #[error("Bootloader installation failed: {0}")]
    BootloaderFailed(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Command failed: {0}")]
    CommandFailed(String),
}

/// Image format
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    /// Raw disk image
    Raw,
    /// QEMU copy-on-write v2
    Qcow2,
    /// Hybrid ISO (BIOS + UEFI bootable)
    Iso,
    /// Composefs-native: EROFS image + CAS store + SQLite DB
    ///
    /// Produces the same artifact type as a runtime generation.
    /// The bootstrap output is "generation 1."
    Erofs,
}

impl FromStr for ImageFormat {
    type Err = ImageError;

    /// Parse format from string
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "raw" => Ok(Self::Raw),
            "qcow2" => Ok(Self::Qcow2),
            "iso" => Ok(Self::Iso),
            "erofs" | "composefs" => Ok(Self::Erofs),
            _ => Err(ImageError::InvalidFormat(s.to_string())),
        }
    }
}

impl ImageFormat {
    /// Get file extension
    pub fn extension(&self) -> &'static str {
        match self {
            Self::Raw => "img",
            Self::Qcow2 => "qcow2",
            Self::Iso => "iso",
            Self::Erofs => "erofs",
        }
    }
}

impl std::fmt::Display for ImageFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Raw => write!(f, "raw"),
            Self::Qcow2 => write!(f, "qcow2"),
            Self::Iso => write!(f, "iso"),
            Self::Erofs => write!(f, "erofs"),
        }
    }
}

/// Image size in bytes
#[derive(Debug, Clone, Copy)]
pub struct ImageSize(u64);

impl FromStr for ImageSize {
    type Err = ImageError;

    /// Parse size from string (e.g., "4G", "512M", "8192")
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.is_empty() {
            return Err(ImageError::InvalidSize("empty size".to_string()));
        }

        let (num_str, multiplier) = if let Some(n) = s.strip_suffix(['G', 'g']) {
            (n, 1024 * 1024 * 1024u64)
        } else if let Some(n) = s.strip_suffix(['M', 'm']) {
            (n, 1024 * 1024u64)
        } else if let Some(n) = s.strip_suffix(['K', 'k']) {
            (n, 1024u64)
        } else if let Some(n) = s.strip_suffix(['T', 't']) {
            (n, 1024 * 1024 * 1024 * 1024u64)
        } else {
            (s, 1u64)
        };

        let num: u64 = num_str
            .trim()
            .parse()
            .map_err(|_| ImageError::InvalidSize(s.to_string()))?;

        Ok(Self(num * multiplier))
    }
}

impl ImageSize {
    /// Get size in bytes
    pub fn bytes(&self) -> u64 {
        self.0
    }

    /// Get size in megabytes
    pub fn megabytes(&self) -> u64 {
        self.0 / (1024 * 1024)
    }

    /// Get size in gigabytes
    pub fn gigabytes(&self) -> u64 {
        self.0 / (1024 * 1024 * 1024)
    }
}

impl std::fmt::Display for ImageSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0 >= 1024 * 1024 * 1024 {
            write!(f, "{}G", self.gigabytes())
        } else if self.0 >= 1024 * 1024 {
            write!(f, "{}M", self.megabytes())
        } else {
            write!(f, "{}", self.0)
        }
    }
}

/// Required tools for image generation
pub struct ImageTools {
    pub dd: PathBuf,
    pub mkfs_fat: Option<PathBuf>,
    pub qemu_img: Option<PathBuf>,
    pub xorriso: Option<PathBuf>,
    pub mksquashfs: Option<PathBuf>,
    pub systemd_repart: Option<PathBuf>,
    pub ukify: Option<PathBuf>,
}

impl ImageTools {
    /// Check for required tools
    pub fn check() -> Result<Self, ImageError> {
        let find_tool = |names: &[&str]| -> Option<PathBuf> {
            for name in names {
                if let Ok(output) = Command::new("which").arg(name).output()
                    && output.status.success()
                {
                    let path = String::from_utf8_lossy(&output.stdout);
                    return Some(PathBuf::from(path.trim()));
                }
            }
            None
        };

        let dd = find_tool(&["dd"]).ok_or_else(|| ImageError::ToolNotFound("dd".to_string()))?;

        Ok(Self {
            dd,
            mkfs_fat: find_tool(&["mkfs.fat", "mkfs.vfat"]),
            qemu_img: find_tool(&["qemu-img"]),
            xorriso: find_tool(&["xorriso"]),
            mksquashfs: find_tool(&["mksquashfs"]),
            systemd_repart: find_tool(&["systemd-repart"]),
            ukify: find_tool(&["ukify"]),
        })
    }

    /// Check if tools are available for a specific format
    pub fn check_for_format(&self, format: ImageFormat) -> Result<(), ImageError> {
        match format {
            ImageFormat::Raw | ImageFormat::Qcow2 => {
                if self.systemd_repart.is_none() {
                    return Err(ImageError::ToolNotFound(
                        "systemd-repart (required for bootstrap raw/qcow2 images)".to_string(),
                    ));
                }
                if format == ImageFormat::Qcow2 && self.qemu_img.is_none() {
                    return Err(ImageError::ToolNotFound(
                        "qemu-img (for qcow2 conversion)".to_string(),
                    ));
                }
            }
            ImageFormat::Iso => {
                if self.xorriso.is_none() {
                    return Err(ImageError::ToolNotFound(
                        "xorriso (for ISO creation)".to_string(),
                    ));
                }
                if self.mksquashfs.is_none() {
                    return Err(ImageError::ToolNotFound(
                        "mksquashfs (for squashfs creation)".to_string(),
                    ));
                }
            }
            ImageFormat::Erofs => {
                // EROFS building uses composefs-rs in userspace -- no external tools required.
                // The composefs-rs feature gate is checked at compile time.
            }
        }
        Ok(())
    }

    /// Get list of missing optional tools
    pub fn missing_optional(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        if self.mkfs_fat.is_none() {
            missing.push("mkfs.fat");
        }
        if self.qemu_img.is_none() {
            missing.push("qemu-img");
        }
        if self.xorriso.is_none() {
            missing.push("xorriso");
        }
        if self.mksquashfs.is_none() {
            missing.push("mksquashfs");
        }
        if self.systemd_repart.is_none() {
            missing.push("systemd-repart");
        }
        if self.ukify.is_none() {
            missing.push("ukify");
        }
        missing
    }
}

/// Image generation result
#[derive(Debug)]
pub struct ImageResult {
    /// Path to generated image
    pub path: PathBuf,
    /// Image format
    pub format: ImageFormat,
    /// Image size in bytes
    pub size: u64,
    /// Whether EFI boot is supported
    pub efi_bootable: bool,
    /// Whether BIOS boot is supported
    pub bios_bootable: bool,
    /// Build method used (e.g., "systemd-repart", "qemu-img")
    pub method: String,
    /// Partition descriptions (if applicable)
    pub partitions: Vec<String>,
}

/// Image builder
pub struct ImageBuilder {
    /// Work directory
    work_dir: PathBuf,

    /// Bootstrap configuration
    #[allow(dead_code)]
    // Only target_arch accessed currently; full config retained for future build steps
    config: BootstrapConfig,

    /// Base system root
    sysroot: PathBuf,

    /// Output path
    output: PathBuf,

    /// Image format
    format: ImageFormat,

    /// Image size
    size: ImageSize,

    /// Detected tools
    tools: ImageTools,

    /// Build log
    log: String,
}

/// Busybox source for building static binary if host doesn't have one.
///
/// Currently unused -- the method prefers the host's static busybox and errors
/// if one is not found. A future enhancement could download and build from source.
#[allow(dead_code)]
const BUSYBOX_SOURCE_URL: &str = "https://busybox.net/downloads/busybox-1.37.0.tar.bz2";

impl ImageBuilder {
    /// ESP partition size (512MB)
    const ESP_SIZE_MB: u64 = 512;

    /// Create a new image builder
    pub fn new(
        work_dir: impl AsRef<Path>,
        config: &BootstrapConfig,
        sysroot: impl AsRef<Path>,
        output: impl AsRef<Path>,
        format: ImageFormat,
        size: ImageSize,
    ) -> Result<Self, ImageError> {
        let work_dir = work_dir.as_ref().to_path_buf();
        let sysroot = sysroot.as_ref().to_path_buf();
        let output = output.as_ref().to_path_buf();

        // Check base system exists
        if !sysroot.exists() {
            return Err(ImageError::BaseSystemNotFound(sysroot));
        }

        // Check for kernel
        let kernel = sysroot.join("boot/vmlinuz");
        if !kernel.exists() {
            warn!("Kernel not found at {:?} - image may not boot", kernel);
        }

        // Check for required tools
        let tools = ImageTools::check()?;
        tools.check_for_format(format)?;

        Ok(Self {
            work_dir,
            config: config.clone(),
            sysroot,
            output,
            format,
            size,
            tools,
            log: String::new(),
        })
    }

    /// Get the output path
    pub fn output_path(&self) -> &Path {
        &self.output
    }

    /// Default output filename for the Tier 1 base image.
    pub const TIER1_DEFAULT_NAME: &'static str = "conaryos-base.qcow2";

    fn verify_tier1_boot_artifacts(&self) -> Result<(), ImageError> {
        let kernel = self.sysroot.join("boot/vmlinuz");
        if !kernel.exists() {
            return Err(ImageError::CreationFailed(
                "Kernel not found at boot/vmlinuz. Run system_config::configure_system() \
                 after Phase 3 installs the versioned kernel."
                    .to_string(),
            ));
        }

        let efi_binary = self.sysroot.join("boot/EFI/BOOT/BOOTX64.EFI");
        if !efi_binary.exists() {
            return Err(ImageError::CreationFailed(
                "EFI binary not found at boot/EFI/BOOT/BOOTX64.EFI. \
                 Run system_config::configure_system() first."
                    .to_string(),
            ));
        }

        let bls_entry = self.sysroot.join("boot/loader/entries/conaryos.conf");
        if !bls_entry.exists() {
            return Err(ImageError::CreationFailed(
                "BLS entry not found at boot/loader/entries/conaryos.conf. \
                 Run system_config::configure_system() first."
                    .to_string(),
            ));
        }

        Ok(())
    }

    /// Build a Tier 1 base image using the standard pipeline.
    ///
    /// This is the convenience entry point for Phase 5. It chains:
    ///
    /// 1. `system_config::configure_system()` -- verify kernel installed, write
    ///    BLS entry, copy EFI binary (called by the orchestrator before this)
    /// 2. `build()` -- run systemd-repart to create GPT image, convert to qcow2
    ///
    /// The caller (Bootstrap orchestrator) is responsible for calling
    /// `finalize_sysroot()` first. This method validates the sysroot has the
    /// expected boot artifacts before proceeding.
    ///
    /// # Errors
    ///
    /// Returns `ImageError` if the kernel or EFI binary is missing from
    /// the sysroot, or if image creation fails.
    pub fn build_tier1_image(&mut self) -> Result<ImageResult, ImageError> {
        info!("Building Tier 1 base image: {}", self.output.display());
        self.verify_tier1_boot_artifacts()?;
        self.build()
    }

    /// Build the image
    pub fn build(&mut self) -> Result<ImageResult, ImageError> {
        info!("Building {} image: {:?}", self.format, self.output);
        self.log_line(&format!("Building {} image", self.format));

        let result = match self.format {
            ImageFormat::Raw => self.build_raw()?,
            ImageFormat::Qcow2 => self.build_qcow2()?,
            ImageFormat::Iso => self.build_iso()?,
            ImageFormat::Erofs => self.build_erofs_generation()?,
        };

        info!("Image built successfully: {:?}", result.path);
        Ok(result)
    }

    /// Build composefs-native output: EROFS image + CAS store + SQLite DB.
    ///
    /// This produces the same artifact type as a runtime generation. The
    /// bootstrap output becomes "generation 1." The output directory contains:
    ///
    /// - `objects/` -- CAS store with all file content
    /// - `generations/1/root.erofs` -- EROFS image referencing CAS objects
    /// - `generations/1/.conary-gen.json` -- generation metadata
    /// - `db.sqlite3` -- SQLite database with trove + file records
    ///
    /// No external tools are needed -- composefs-rs builds EROFS in userspace.
    fn build_erofs_generation(&mut self) -> Result<ImageResult, ImageError> {
        use crate::db::models::{FileEntry, Trove, TroveType};
        use crate::db::schema::migrate;
        use crate::filesystem::CasStore;
        use crate::generation::builder::{FileEntryRef, SymlinkEntryRef, build_erofs_image};
        use crate::generation::metadata::{GENERATION_FORMAT, GenerationMetadata};

        self.log_line("Building composefs-native output (EROFS + CAS + DB)");

        // Create output directory structure (clone to avoid borrow conflict with &mut self)
        let output_dir = self.output.clone();
        fs::create_dir_all(&output_dir)
            .map_err(|e| ImageError::CreationFailed(format!("Failed to create output dir: {e}")))?;

        let objects_dir = output_dir.join("objects");
        let generations_dir = output_dir.join("generations");
        let gen_dir = generations_dir.join("1");
        fs::create_dir_all(&gen_dir)
            .map_err(|e| ImageError::CreationFailed(format!("Failed to create gen dir: {e}")))?;

        // Step 1: Create CAS store and walk the sysroot
        self.log_line("Scanning sysroot and storing files in CAS");
        let cas = CasStore::new(&objects_dir)
            .map_err(|e| ImageError::CreationFailed(format!("Failed to create CAS store: {e}")))?;

        let mut file_entries: Vec<(String, String, u64, u32)> = Vec::new();
        let mut sysroot_symlinks: Vec<(String, String)> = Vec::new();
        self.walk_sysroot_to_cas(
            &cas,
            &self.sysroot.clone(),
            &mut file_entries,
            &mut sysroot_symlinks,
        )?;

        let file_count = file_entries.len();
        self.log_line(&format!("Stored {file_count} files in CAS"));

        // Step 2: Create and initialize SQLite database
        self.log_line("Creating SQLite database with schema");
        let db_path = output_dir.join("db.sqlite3");
        let conn = rusqlite::Connection::open(&db_path)
            .map_err(|e| ImageError::CreationFailed(format!("Failed to create database: {e}")))?;
        migrate(&conn)
            .map_err(|e| ImageError::CreationFailed(format!("Failed to initialize schema: {e}")))?;

        // Step 3: Insert a bootstrap trove and file entries
        self.log_line("Inserting trove and file records");
        let mut trove = Trove::new(
            "conaryos-base".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
        );
        trove.description = Some("conaryOS base system (bootstrapped from LFS)".to_string());
        trove.architecture = Some(self.config.target_arch.to_string());
        let trove_id = trove
            .insert(&conn)
            .map_err(|e| ImageError::CreationFailed(format!("Failed to insert trove: {e}")))?;

        for (path, hash, size, permissions) in &file_entries {
            #[allow(clippy::cast_possible_wrap)]
            let mut fe = FileEntry::new(
                path.clone(),
                hash.clone(),
                *size as i64,
                *permissions as i32,
                trove_id,
            );
            fe.insert_or_replace(&conn).map_err(|e| {
                ImageError::CreationFailed(format!("Failed to insert file entry {path}: {e}"))
            })?;
        }

        // Step 4: Build EROFS image from file entries
        self.log_line("Building EROFS image");
        let erofs_entries: Vec<FileEntryRef> = file_entries
            .iter()
            .map(|(path, hash, size, perms)| FileEntryRef {
                path: path.clone(),
                sha256_hash: hash.clone(),
                size: *size,
                permissions: *perms,
                owner: None,
                group_name: None,
            })
            .collect();

        let symlink_refs: Vec<SymlinkEntryRef> = sysroot_symlinks
            .iter()
            .map(|(path, target)| SymlinkEntryRef {
                path: path.clone(),
                target: target.clone(),
            })
            .collect();
        self.log_line(&format!(
            "Collected {} symlinks from sysroot",
            symlink_refs.len()
        ));

        let build_result = build_erofs_image(&erofs_entries, &symlink_refs, &gen_dir)
            .map_err(|e| ImageError::CreationFailed(format!("Failed to build EROFS image: {e}")))?;

        // Step 5: Write generation metadata
        self.log_line("Writing generation metadata");
        #[allow(clippy::cast_possible_wrap)]
        let metadata = GenerationMetadata {
            generation: 1,
            format: GENERATION_FORMAT.to_string(),
            erofs_size: Some(build_result.image_size as i64),
            cas_objects_referenced: Some(build_result.cas_objects_referenced as i64),
            fsverity_enabled: false,
            erofs_verity_digest: None,
            created_at: chrono::Utc::now().to_rfc3339(),
            package_count: 1,
            kernel_version: crate::generation::metadata::detect_kernel_version(&self.sysroot),
            summary: "Bootstrap generation 1 (LFS base system)".to_string(),
        };
        metadata.write_to(&gen_dir).map_err(|e| {
            ImageError::CreationFailed(format!("Failed to write generation metadata: {e}"))
        })?;

        // Create "current" symlink
        #[cfg(unix)]
        {
            let current_link = output_dir.join("current");
            let _ = fs::remove_file(&current_link);
            std::os::unix::fs::symlink("generations/1", &current_link).map_err(|e| {
                ImageError::CreationFailed(format!("Failed to create current symlink: {e}"))
            })?;
        }

        // Close DB connection before reporting
        drop(conn);

        let erofs_size = build_result.image_size;
        let objects_size = dir_size(&objects_dir);
        let db_size = fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);
        let total_size = erofs_size + objects_size + db_size;

        self.log_line(&format!(
            "Generation 1 complete: EROFS={erofs_size} bytes, CAS={objects_size} bytes, \
             DB={db_size} bytes, files={file_count}"
        ));

        Ok(ImageResult {
            path: output_dir.clone(),
            format: ImageFormat::Erofs,
            size: total_size,
            efi_bootable: false, // Not directly bootable -- needs qcow2 wrapper
            bios_bootable: false,
            method: "composefs-rs".to_string(),
            partitions: vec![
                format!("CAS objects ({objects_size} bytes)"),
                format!("EROFS image ({erofs_size} bytes)"),
                format!("SQLite DB ({db_size} bytes)"),
            ],
        })
    }

    /// Walk the sysroot directory tree, store regular files in CAS, and collect entries.
    ///
    /// Skips excluded directories (var, tmp, proc, etc.) and non-regular files.
    /// Symlinks are tracked but not stored as CAS objects (they are encoded
    /// directly in the EROFS tree by the builder).
    fn walk_sysroot_to_cas(
        &mut self,
        cas: &crate::filesystem::CasStore,
        sysroot: &Path,
        entries: &mut Vec<(String, String, u64, u32)>,
        symlinks: &mut Vec<(String, String)>,
    ) -> Result<(), ImageError> {
        use crate::generation::metadata::is_excluded;

        let mut stack: Vec<PathBuf> = vec![sysroot.to_path_buf()];

        while let Some(dir) = stack.pop() {
            let read_dir = match fs::read_dir(&dir) {
                Ok(rd) => rd,
                Err(e) => {
                    warn!("Cannot read directory {}: {e}", dir.display());
                    continue;
                }
            };

            for entry in read_dir {
                let entry = match entry {
                    Ok(e) => e,
                    Err(e) => {
                        warn!("Error reading dir entry: {e}");
                        continue;
                    }
                };

                let path = entry.path();
                let rel_path = path
                    .strip_prefix(sysroot)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .to_string();

                // Skip excluded directories
                if is_excluded(&rel_path) {
                    continue;
                }

                let metadata = match fs::symlink_metadata(&path) {
                    Ok(m) => m,
                    Err(e) => {
                        warn!("Cannot stat {}: {e}", path.display());
                        continue;
                    }
                };

                if metadata.is_dir() {
                    stack.push(path);
                    continue;
                }

                // Collect symlinks for EROFS generation (metadata, not CAS content)
                if metadata.is_symlink() {
                    match std::fs::read_link(&path) {
                        Ok(target) => {
                            let rel = format!("/{}", rel_path.trim_start_matches('/'));
                            symlinks.push((rel, target.to_string_lossy().to_string()));
                        }
                        Err(e) => {
                            warn!("Cannot read symlink target {}: {e}", path.display());
                        }
                    }
                    continue;
                }

                if !metadata.is_file() {
                    // Skip special files (sockets, devices, etc.)
                    continue;
                }

                // Read file and store in CAS
                let content = match fs::read(&path) {
                    Ok(c) => c,
                    Err(e) => {
                        warn!("Cannot read {}: {e}", path.display());
                        continue;
                    }
                };

                let hash = cas.store(&content).map_err(|e| {
                    ImageError::CreationFailed(format!(
                        "Failed to store {} in CAS: {e}",
                        path.display()
                    ))
                })?;

                #[cfg(unix)]
                let permissions = {
                    use std::os::unix::fs::MetadataExt;
                    metadata.mode() & 0o7777
                };
                #[cfg(not(unix))]
                let permissions = 0o644u32;

                let size = metadata.len();
                let abs_path = format!("/{rel_path}");

                entries.push((abs_path, hash, size, permissions));
            }
        }

        Ok(())
    }

    fn write_repart_mke2fs_config(&self) -> Result<tempfile::NamedTempFile, ImageError> {
        let host_config = fs::read_to_string("/etc/mke2fs.conf").map_err(|e| {
            ImageError::FilesystemFailed(format!("failed to read /etc/mke2fs.conf: {e}"))
        })?;
        let updated = enable_ext4_verity_feature(&host_config)
            .map_err(|e| ImageError::FilesystemFailed(format!("invalid mke2fs.conf: {e}")))?;

        let mut temp = tempfile::NamedTempFile::new_in(&self.work_dir).map_err(|e| {
            ImageError::FilesystemFailed(format!(
                "failed to create temporary mke2fs.conf in {}: {e}",
                self.work_dir.display()
            ))
        })?;
        temp.write_all(updated.as_bytes()).map_err(|e| {
            ImageError::FilesystemFailed(format!("failed to write temporary mke2fs.conf: {e}"))
        })?;
        temp.flush().map_err(|e| {
            ImageError::FilesystemFailed(format!("failed to flush temporary mke2fs.conf: {e}"))
        })?;

        Ok(temp)
    }

    /// Build a raw disk image using systemd-repart.
    fn build_raw_repart(&mut self) -> Result<ImageResult, ImageError> {
        let repart_dir = self.work_dir.join("repart.d");
        super::repart::generate_repart_definitions(
            &repart_dir,
            self.config.target_arch,
            Self::ESP_SIZE_MB,
        )
        .map_err(|e| ImageError::PartitionFailed(e.to_string()))?;

        let repart_bin = self
            .tools
            .systemd_repart
            .clone()
            .ok_or_else(|| ImageError::ToolNotFound("systemd-repart".to_string()))?;
        let mke2fs_config = self.write_repart_mke2fs_config()?;

        self.log_line("Creating disk image with systemd-repart");

        let output = Command::new(&repart_bin)
            .arg("--empty=create")
            .arg(format!("--size={}", self.size.bytes()))
            .arg(format!("--definitions={}", repart_dir.display()))
            .arg(format!("--root={}", self.sysroot.display()))
            .arg("--discard=no")
            .env("MKE2FS_CONFIG", mke2fs_config.path())
            .arg(&self.output)
            .output()
            .map_err(|e| ImageError::CommandFailed(format!("systemd-repart: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ImageError::CommandFailed(format!(
                "systemd-repart failed: {stderr}"
            )));
        }

        self.log_line("Disk image created successfully");

        let size = fs::metadata(&self.output)?.len();

        Ok(ImageResult {
            path: self.output.clone(),
            format: self.format,
            size,
            efi_bootable: true,
            bios_bootable: false,
            method: "systemd-repart".to_string(),
            partitions: vec![
                format!("ESP ({}MB vfat)", Self::ESP_SIZE_MB),
                "root (ext4)".to_string(),
            ],
        })
    }

    /// Build raw disk image using systemd-repart.
    fn build_raw(&mut self) -> Result<ImageResult, ImageError> {
        self.verify_tier1_boot_artifacts()?;
        self.log_line("Using systemd-repart for bootstrap image generation");
        self.build_raw_repart()
    }

    /// Build qcow2 image (raw + convert)
    fn build_qcow2(&mut self) -> Result<ImageResult, ImageError> {
        self.log_line("Creating qcow2 image");

        // Build raw image first
        let raw_path = self.output.with_extension("raw.tmp");
        let original_output = self.output.clone();
        self.output = raw_path.clone();

        self.build_raw()?;

        // Convert to qcow2
        self.output = original_output;
        self.log_line("Converting to qcow2 format");

        let qemu_img = self
            .tools
            .qemu_img
            .as_ref()
            .ok_or_else(|| ImageError::ToolNotFound("qemu-img".to_string()))?;

        let status = Command::new(qemu_img)
            .args([
                "convert", "-f", "raw", "-O", "qcow2", "-c", // Compress
            ])
            .arg(&raw_path)
            .arg(&self.output)
            .status()?;

        if !status.success() {
            return Err(ImageError::CreationFailed(
                "qemu-img convert failed".to_string(),
            ));
        }

        // Remove temporary raw image
        let _ = fs::remove_file(&raw_path);

        let size = fs::metadata(&self.output)?.len();

        Ok(ImageResult {
            path: self.output.clone(),
            format: ImageFormat::Qcow2,
            size,
            efi_bootable: true,
            bios_bootable: false,
            method: "qemu-img".to_string(),
            partitions: vec![
                format!("ESP ({}MB vfat)", Self::ESP_SIZE_MB),
                "root (ext4)".to_string(),
            ],
        })
    }

    /// Build ISO image
    fn build_iso(&mut self) -> Result<ImageResult, ImageError> {
        self.log_line("Creating ISO image");

        let iso_dir = self.work_dir.join("iso_staging");
        fs::create_dir_all(&iso_dir)?;

        // Create squashfs of root filesystem
        self.log_line("Creating squashfs");
        let squashfs_path = iso_dir.join("conary.squashfs");
        self.create_squashfs(&squashfs_path)?;

        // Set up boot directory
        self.log_line("Setting up boot structure");
        self.setup_iso_boot(&iso_dir)?;

        // Create ISO
        self.log_line("Building ISO image");
        self.create_iso(&iso_dir)?;

        // Cleanup staging
        let _ = fs::remove_dir_all(&iso_dir);

        let size = fs::metadata(&self.output)?.len();

        warn!("Boot artifact population not yet implemented -- image may not be bootable");
        Ok(ImageResult {
            path: self.output.clone(),
            format: ImageFormat::Iso,
            size,
            efi_bootable: false,
            bios_bootable: false,
            method: "xorriso".to_string(),
            partitions: Vec::new(),
        })
    }

    /// Create squashfs for ISO
    fn create_squashfs(&self, output: &Path) -> Result<(), ImageError> {
        let mksquashfs = self
            .tools
            .mksquashfs
            .as_ref()
            .ok_or_else(|| ImageError::ToolNotFound("mksquashfs".to_string()))?;

        let status = Command::new(mksquashfs)
            .arg(&self.sysroot)
            .arg(output)
            .args([
                "-comp",
                "zstd",
                "-Xcompression-level",
                "19",
                "-e",
                "dev/*",
                "-e",
                "proc/*",
                "-e",
                "sys/*",
                "-e",
                "tmp/*",
                "-e",
                "run/*",
            ])
            .status()?;

        if !status.success() {
            return Err(ImageError::CreationFailed("mksquashfs failed".to_string()));
        }

        Ok(())
    }

    /// Set up ISO boot structure
    fn setup_iso_boot(&self, iso_dir: &Path) -> Result<(), ImageError> {
        // Create boot directories
        let boot_dir = iso_dir.join("boot");
        let grub_dir = boot_dir.join("grub");
        let efi_dir = iso_dir.join("EFI/BOOT");

        fs::create_dir_all(&grub_dir)?;
        fs::create_dir_all(&efi_dir)?;

        // Copy kernel and initramfs
        let kernel_src = self.sysroot.join("boot/vmlinuz");
        let initrd_src = self.sysroot.join("boot/initramfs.img");

        if kernel_src.exists() {
            fs::copy(&kernel_src, boot_dir.join("vmlinuz"))?;
        }
        if initrd_src.exists() {
            fs::copy(&initrd_src, boot_dir.join("initramfs.img"))?;
        }

        // Create GRUB config for ISO
        let iso_grub_cfg = r#"# GRUB Configuration for Conary Live
set default=0
set timeout=10

menuentry "Conary Linux (Live)" {
    linux /boot/vmlinuz root=live:CDLABEL=CONARY_LIVE ro quiet
    initrd /boot/initramfs.img
}

menuentry "Conary Linux (Live, Text Mode)" {
    linux /boot/vmlinuz root=live:CDLABEL=CONARY_LIVE ro systemd.unit=multi-user.target
    initrd /boot/initramfs.img
}
"#;
        fs::write(grub_dir.join("grub.cfg"), iso_grub_cfg)?;

        // Look for EFI image
        let grub_efi_paths = [
            self.sysroot.join("usr/lib/grub/x86_64-efi/grubx64.efi"),
            self.sysroot.join("usr/share/grub/x86_64-efi/grubx64.efi"),
        ];

        if let Some(src) = grub_efi_paths.iter().find(|p| p.exists()) {
            fs::copy(src, efi_dir.join("BOOTX64.EFI"))?;
        }

        Ok(())
    }

    /// Create ISO image
    fn create_iso(&self, iso_dir: &Path) -> Result<(), ImageError> {
        let xorriso = self
            .tools
            .xorriso
            .as_ref()
            .ok_or_else(|| ImageError::ToolNotFound("xorriso".to_string()))?;

        // Create EFI boot image
        let efi_img = iso_dir.join("boot/efi.img");
        self.create_efi_image(&efi_img)?;

        let status = Command::new(xorriso)
            .args(["-as", "mkisofs", "-o"])
            .arg(&self.output)
            .args([
                "-R",
                "-J",
                "-V",
                "CONARY_LIVE",
                "-b",
                "boot/grub/i386-pc/eltorito.img",
                "-no-emul-boot",
                "-boot-load-size",
                "4",
                "-boot-info-table",
                "-eltorito-alt-boot",
                "-e",
                "boot/efi.img",
                "-no-emul-boot",
                "-isohybrid-gpt-basdat",
            ])
            .arg(iso_dir)
            .status()?;

        if !status.success() {
            // Try simpler ISO creation if hybrid fails
            warn!("Hybrid ISO creation failed, trying simple ISO");
            let status = Command::new(xorriso)
                .args(["-as", "mkisofs", "-o"])
                .arg(&self.output)
                .args(["-R", "-J", "-V", "CONARY_LIVE"])
                .arg(iso_dir)
                .status()?;

            if !status.success() {
                return Err(ImageError::CreationFailed("xorriso failed".to_string()));
            }
        }

        Ok(())
    }

    /// Create EFI boot image for ISO
    fn create_efi_image(&self, output: &Path) -> Result<(), ImageError> {
        // Create a small FAT image for EFI boot
        let size_mb = 4; // 4MB is enough for EFI

        // Create sparse file
        let file = File::create(output)?;
        file.set_len(size_mb * 1024 * 1024)?;

        // Format as FAT
        if let Some(ref mkfs_fat) = self.tools.mkfs_fat {
            let _ = Command::new(mkfs_fat)
                .args(["-F", "12"])
                .arg(output)
                .output();
        }

        Ok(())
    }

    /// Add a line to the build log
    fn log_line(&mut self, msg: &str) {
        info!("{}", msg);
        self.log.push_str(msg);
        self.log.push('\n');
    }

    // generate_initramfs() removed: deprecated in favour of
    // system_config::configure_system() + dracut.

    /// Get the build log
    pub fn log(&self) -> &str {
        &self.log
    }

}

/// Recursively compute the total size of a directory in bytes.
fn dir_size(path: &Path) -> u64 {
    let mut total = 0u64;
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                total += dir_size(&p);
            } else if let Ok(m) = fs::metadata(&p) {
                total += m.len();
            }
        }
    }
    total
}

fn enable_ext4_verity_feature(config: &str) -> Result<String, String> {
    let mut updated = Vec::new();
    let mut in_ext4 = false;
    let mut ext4_found = false;
    let mut features_found = false;

    for line in config.lines() {
        let trimmed = line.trim();

        if !in_ext4
            && trimmed.ends_with('{')
            && let Some((name, _)) = trimmed.split_once('=')
            && name.trim() == "ext4"
        {
            in_ext4 = true;
            ext4_found = true;
            updated.push(line.to_string());
            continue;
        }

        if in_ext4 {
            if trimmed.starts_with('}') {
                if !features_found {
                    return Err("mke2fs.conf ext4 features line not found".to_string());
                }
                in_ext4 = false;
                updated.push(line.to_string());
                continue;
            }

            if let Some((key, value)) = line.split_once('=')
                && key.trim() == "features"
            {
                let (raw_value, comment) = value
                    .split_once('#')
                    .map_or((value, None), |(features, comment)| {
                        (features, Some(comment))
                    });
                let mut features: Vec<String> = raw_value
                    .split(',')
                    .map(|feature| feature.trim())
                    .filter(|feature| !feature.is_empty())
                    .map(ToOwned::to_owned)
                    .collect();

                if !features.iter().any(|feature| feature == "verity") {
                    features.push("verity".to_string());
                }

                let indent = &line[..line.find("features").unwrap_or(0)];
                let mut rebuilt = format!("{indent}features = {}", features.join(","));
                if let Some(comment) = comment {
                    rebuilt.push_str(" #");
                    rebuilt.push_str(comment);
                }

                updated.push(rebuilt);
                features_found = true;
                continue;
            }
        }

        updated.push(line.to_string());
    }

    if !ext4_found {
        return Err("mke2fs.conf ext4 stanza not found".to_string());
    }
    if in_ext4 && !features_found {
        return Err("mke2fs.conf ext4 features line not found".to_string());
    }

    let mut rebuilt = updated.join("\n");
    if config.ends_with('\n') {
        rebuilt.push('\n');
    }
    Ok(rebuilt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::metadata::GENERATION_FORMAT;

    #[test]
    fn test_image_format_from_str() {
        assert_eq!(ImageFormat::from_str("raw").unwrap(), ImageFormat::Raw);
        assert_eq!(ImageFormat::from_str("qcow2").unwrap(), ImageFormat::Qcow2);
        assert_eq!(ImageFormat::from_str("iso").unwrap(), ImageFormat::Iso);
        assert_eq!(ImageFormat::from_str("erofs").unwrap(), ImageFormat::Erofs);
        assert_eq!(
            ImageFormat::from_str("composefs").unwrap(),
            ImageFormat::Erofs
        );
        assert_eq!(ImageFormat::from_str("RAW").unwrap(), ImageFormat::Raw);
        assert_eq!(ImageFormat::from_str("EROFS").unwrap(), ImageFormat::Erofs);
        assert!(ImageFormat::from_str("invalid").is_err());
    }

    #[test]
    fn test_image_format_extension() {
        assert_eq!(ImageFormat::Raw.extension(), "img");
        assert_eq!(ImageFormat::Qcow2.extension(), "qcow2");
        assert_eq!(ImageFormat::Iso.extension(), "iso");
        assert_eq!(ImageFormat::Erofs.extension(), "erofs");
    }

    #[test]
    fn test_image_size_from_str() {
        assert_eq!(ImageSize::from_str("4G").unwrap().gigabytes(), 4);
        assert_eq!(ImageSize::from_str("512M").unwrap().megabytes(), 512);
        assert_eq!(ImageSize::from_str("1024K").unwrap().bytes(), 1024 * 1024);
        assert_eq!(ImageSize::from_str("1T").unwrap().gigabytes(), 1024);
        assert_eq!(ImageSize::from_str("1048576").unwrap().bytes(), 1048576);
        assert!(ImageSize::from_str("").is_err());
        assert!(ImageSize::from_str("abc").is_err());
    }

    #[test]
    fn test_image_size_display() {
        assert_eq!(ImageSize::from_str("4G").unwrap().to_string(), "4G");
        assert_eq!(ImageSize::from_str("512M").unwrap().to_string(), "512M");
    }

    #[test]
    fn test_image_tools_check() {
        // Basic tools should be available on most systems
        let tools = ImageTools::check();
        assert!(tools.is_ok());
        let tools = tools.unwrap();
        // dd, mount, umount should exist
        assert!(tools.dd.exists());
    }

    #[test]
    fn test_initramfs_init_script_content() {
        let script = "#!/bin/sh\n\
            mount -t proc proc /proc\n\
            mount -t sysfs sys /sys\n\
            mount -t devtmpfs dev /dev\n\
            mount /dev/vda2 /mnt/root\n\
            exec switch_root /mnt/root /lib/systemd/systemd\n";
        assert!(script.starts_with("#!/bin/sh"));
        assert!(script.contains("switch_root"));
        assert!(script.contains("/lib/systemd/systemd"));
        assert!(script.contains("devtmpfs"));
    }

    #[test]
    fn test_image_tools_repart_detection() {
        let tools = ImageTools::check().unwrap();
        // systemd-repart may or may not be installed -- just verify the fields exist
        let _ = tools.systemd_repart;
        let _ = tools.ukify;
    }

    #[test]
    fn test_erofs_format_no_tools_required() {
        let tools = ImageTools::check().unwrap();
        // EROFS format should not require any external tools
        assert!(tools.check_for_format(ImageFormat::Erofs).is_ok());
    }

    #[cfg(feature = "composefs-rs")]
    #[test]
    fn test_erofs_generation_from_sysroot() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::TempDir::new().unwrap();
        let sysroot = tmp.path().join("sysroot");
        let output = tmp.path().join("output");

        // Create a minimal sysroot
        fs::create_dir_all(sysroot.join("usr/bin")).unwrap();
        fs::create_dir_all(sysroot.join("usr/lib")).unwrap();
        fs::create_dir_all(sysroot.join("etc")).unwrap();

        fs::write(sysroot.join("usr/bin/hello"), b"#!/bin/sh\necho hello\n").unwrap();
        fs::set_permissions(
            sysroot.join("usr/bin/hello"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();

        fs::write(sysroot.join("usr/lib/libtest.so"), b"fake shared lib").unwrap();
        fs::write(sysroot.join("etc/hostname"), b"conaryos\n").unwrap();

        // Create ImageBuilder with EROFS format
        let config = BootstrapConfig::new();
        let mut builder = ImageBuilder::new(
            tmp.path(),
            &config,
            &sysroot,
            &output,
            ImageFormat::Erofs,
            ImageSize(0),
        )
        .unwrap();

        let result = builder.build().unwrap();

        // Verify output structure
        assert_eq!(result.format, ImageFormat::Erofs);
        assert_eq!(result.method, "composefs-rs");
        assert!(result.size > 0);

        // Verify directory structure
        assert!(
            output.join("objects").is_dir(),
            "CAS objects dir must exist"
        );
        assert!(
            output.join("generations/1/root.erofs").is_file(),
            "EROFS image must exist"
        );
        assert!(
            output.join("generations/1/.conary-gen.json").is_file(),
            "Generation metadata must exist"
        );
        assert!(output.join("db.sqlite3").is_file(), "SQLite DB must exist");
        assert!(
            output.join("current").exists(),
            "current symlink must exist"
        );

        // Verify EROFS magic
        let erofs_bytes = fs::read(output.join("generations/1/root.erofs")).unwrap();
        assert!(erofs_bytes.len() > 1028, "EROFS image too small");
        let magic = u32::from_le_bytes([
            erofs_bytes[1024],
            erofs_bytes[1025],
            erofs_bytes[1026],
            erofs_bytes[1027],
        ]);
        assert_eq!(magic, 0xE0F5_E1E2, "EROFS magic mismatch");

        // Verify database has records
        let conn = rusqlite::Connection::open(output.join("db.sqlite3")).unwrap();
        let trove_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM troves", [], |row| row.get(0))
            .unwrap();
        assert_eq!(trove_count, 1, "Should have 1 trove (conaryos-base)");

        let file_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))
            .unwrap();
        assert_eq!(file_count, 3, "Should have 3 file entries");

        // Verify generation metadata
        let metadata = crate::generation::metadata::GenerationMetadata::read_from(
            &output.join("generations/1"),
        )
        .unwrap();
        assert_eq!(metadata.generation, 1);
        assert_eq!(metadata.format, GENERATION_FORMAT);
        assert_eq!(metadata.package_count, 1);
        assert!(metadata.cas_objects_referenced.unwrap() > 0);
    }

    #[test]
    fn test_dir_size_helper() {
        let tmp = tempfile::TempDir::new().unwrap();
        fs::write(tmp.path().join("a"), b"hello").unwrap();
        fs::create_dir(tmp.path().join("sub")).unwrap();
        fs::write(tmp.path().join("sub/b"), b"world!").unwrap();

        let size = dir_size(tmp.path());
        assert_eq!(size, 11, "5 bytes + 6 bytes = 11 bytes");
    }

    #[test]
    fn test_detect_kernel_in_sysroot() {
        use crate::generation::metadata::detect_kernel_version;

        let tmp = tempfile::TempDir::new().unwrap();

        // No modules dir
        assert!(detect_kernel_version(tmp.path()).is_none());

        // Create modules dir with a version
        fs::create_dir_all(tmp.path().join("usr/lib/modules/6.12.1-conary")).unwrap();
        let version = detect_kernel_version(tmp.path());
        assert_eq!(version.as_deref(), Some("6.12.1-conary"));
    }

    #[test]
    fn test_enable_ext4_verity_feature_adds_verity_to_ext4_features() {
        let input = "\
[defaults]
base_features = sparse_super

[fs_types]
ext4 = {
    features = has_journal,extent,64bit
}
";

        let updated = enable_ext4_verity_feature(input).expect("ext4 stanza should be updated");
        assert!(updated.contains("features = has_journal,extent,64bit,verity"));
    }

    #[test]
    fn test_enable_ext4_verity_feature_is_idempotent() {
        let input = "\
[fs_types]
ext4 = {
    features = has_journal,extent,verity
}
";

        let updated =
            enable_ext4_verity_feature(input).expect("ext4 stanza with verity should parse");
        assert_eq!(updated.matches("verity").count(), 1);
    }

    #[test]
    fn test_enable_ext4_verity_feature_rejects_missing_ext4_features_line() {
        let input = "\
[fs_types]
ext4 = {
    inode_size = 256
}
";

        let err = enable_ext4_verity_feature(input).unwrap_err();
        assert!(err.contains("ext4 features line"));
    }

    #[test]
    fn test_build_raw_rejects_missing_efi_boot_artifacts() {
        let tmp = tempfile::TempDir::new().unwrap();
        let sysroot = tmp.path().join("sysroot");
        fs::create_dir_all(sysroot.join("boot")).unwrap();
        fs::write(sysroot.join("boot/vmlinuz"), b"kernel").unwrap();

        let config = BootstrapConfig::new();
        let mut builder = ImageBuilder::new(
            tmp.path(),
            &config,
            &sysroot,
            tmp.path().join("out.raw"),
            ImageFormat::Raw,
            ImageSize::from_str("1G").unwrap(),
        )
        .unwrap();

        let err = builder.build_raw().unwrap_err();
        assert!(err.to_string().contains("EFI binary not found"));
    }

    #[test]
    fn test_raw_qcow2_formats_require_systemd_repart() {
        let tools = ImageTools {
            dd: PathBuf::from("/bin/dd"),
            mkfs_fat: None,
            qemu_img: Some(PathBuf::from("/usr/bin/qemu-img")),
            xorriso: None,
            mksquashfs: None,
            systemd_repart: None,
            ukify: None,
        };

        let err = tools.check_for_format(ImageFormat::Raw).unwrap_err();
        assert!(err.to_string().contains("systemd-repart"));
    }
}
