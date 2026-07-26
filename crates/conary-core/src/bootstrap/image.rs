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

mod erofs_generation;

use super::config::BootstrapConfig;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;
use thiserror::Error;
use tracing::{info, warn};

pub use crate::image::size::ImageSize;

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
    const TIER1_ROOT_EXCLUDE_FILES: &'static [&'static str] = &[
        "/boot",
        "/dev",
        "/home",
        "/media",
        "/mnt",
        "/opt",
        "/proc",
        "/root/.cargo",
        "/run",
        "/srv",
        "/sys",
        "/tmp",
        "/tools",
        "/var/tmp/conary-bootstrap",
        "/var/tmp/conary-bootstrap/*",
    ];
    const TIER1_ROOT_MAKE_DIRECTORIES: &'static [&'static str] = &[
        "/boot", "/dev", "/home", "/media", "/mnt", "/opt", "/proc", "/run", "/srv", "/sys",
        "/tmp", "/var/tmp",
    ];

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

    fn tier1_root_repart_definition(
        arch: crate::bootstrap::TargetArch,
    ) -> crate::image::repart::RepartDefinition {
        let mut root = crate::image::repart::RepartDefinition::root(arch, Path::new("/"));
        root.exclude_files = Self::TIER1_ROOT_EXCLUDE_FILES
            .iter()
            .map(ToString::to_string)
            .collect();
        root.make_directories = Self::TIER1_ROOT_MAKE_DIRECTORIES
            .iter()
            .map(ToString::to_string)
            .collect();
        root
    }

    fn write_tier1_repart_definitions(&self, output_dir: &Path) -> Result<(), ImageError> {
        fs::create_dir_all(output_dir)?;

        let esp =
            crate::image::repart::RepartDefinition::esp(Path::new("/boot"), Self::ESP_SIZE_MB);
        fs::write(output_dir.join("00-esp.conf"), esp.to_string())?;

        let root = Self::tier1_root_repart_definition(self.config.target_arch);
        fs::write(output_dir.join("10-root.conf"), root.to_string())?;

        Ok(())
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
        self.write_tier1_repart_definitions(&repart_dir)?;

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
#[path = "image/tests.rs"]
mod tests;
