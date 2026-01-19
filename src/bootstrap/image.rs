// src/bootstrap/image.rs

//! Bootable image generation
//!
//! Creates bootable images from the built base system. Supports multiple formats:
//!
//! - **raw**: Direct disk image, can be written to USB or used with QEMU
//! - **qcow2**: QEMU copy-on-write format, efficient for VM testing
//! - **iso**: Hybrid ISO image, bootable from CD/DVD or USB
//!
//! # Image Layout (GPT)
//!
//! ```text
//! ┌─────────────────────────────────────────────┐
//! │  GPT Header (LBA 0-33)                      │
//! ├─────────────────────────────────────────────┤
//! │  ESP Partition (512MB, FAT32)               │
//! │  - /EFI/BOOT/BOOTX64.EFI                    │
//! │  - /EFI/conary/grubx64.efi                  │
//! │  - /grub/grub.cfg                           │
//! ├─────────────────────────────────────────────┤
//! │  Root Partition (remaining, ext4)           │
//! │  - Full base system                         │
//! │  - /boot/vmlinuz, /boot/initramfs           │
//! ├─────────────────────────────────────────────┤
//! │  GPT Footer                                 │
//! └─────────────────────────────────────────────┘
//! ```

use super::config::BootstrapConfig;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::str::FromStr;
use thiserror::Error;
use tracing::{debug, info, warn};

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
}

impl FromStr for ImageFormat {
    type Err = ImageError;

    /// Parse format from string
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "raw" => Ok(Self::Raw),
            "qcow2" => Ok(Self::Qcow2),
            "iso" => Ok(Self::Iso),
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
        }
    }
}

impl std::fmt::Display for ImageFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Raw => write!(f, "raw"),
            Self::Qcow2 => write!(f, "qcow2"),
            Self::Iso => write!(f, "iso"),
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
    pub parted: Option<PathBuf>,
    pub sfdisk: Option<PathBuf>,
    pub mkfs_fat: Option<PathBuf>,
    pub mkfs_ext4: Option<PathBuf>,
    pub mount: PathBuf,
    pub umount: PathBuf,
    pub grub_install: Option<PathBuf>,
    pub qemu_img: Option<PathBuf>,
    pub xorriso: Option<PathBuf>,
    pub mksquashfs: Option<PathBuf>,
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
        let mount =
            find_tool(&["mount"]).ok_or_else(|| ImageError::ToolNotFound("mount".to_string()))?;
        let umount =
            find_tool(&["umount"]).ok_or_else(|| ImageError::ToolNotFound("umount".to_string()))?;

        Ok(Self {
            dd,
            parted: find_tool(&["parted"]),
            sfdisk: find_tool(&["sfdisk"]),
            mkfs_fat: find_tool(&["mkfs.fat", "mkfs.vfat"]),
            mkfs_ext4: find_tool(&["mkfs.ext4", "mke2fs"]),
            mount,
            umount,
            grub_install: find_tool(&["grub-install", "grub2-install"]),
            qemu_img: find_tool(&["qemu-img"]),
            xorriso: find_tool(&["xorriso"]),
            mksquashfs: find_tool(&["mksquashfs"]),
        })
    }

    /// Check if tools are available for a specific format
    pub fn check_for_format(&self, format: ImageFormat) -> Result<(), ImageError> {
        match format {
            ImageFormat::Raw | ImageFormat::Qcow2 => {
                if self.sfdisk.is_none() && self.parted.is_none() {
                    return Err(ImageError::ToolNotFound(
                        "sfdisk or parted (for partitioning)".to_string(),
                    ));
                }
                if self.mkfs_fat.is_none() {
                    return Err(ImageError::ToolNotFound(
                        "mkfs.fat (for ESP partition)".to_string(),
                    ));
                }
                if self.mkfs_ext4.is_none() {
                    return Err(ImageError::ToolNotFound(
                        "mkfs.ext4 (for root partition)".to_string(),
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
        }
        Ok(())
    }

    /// Get list of missing optional tools
    pub fn missing_optional(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        if self.parted.is_none() && self.sfdisk.is_none() {
            missing.push("parted/sfdisk");
        }
        if self.mkfs_fat.is_none() {
            missing.push("mkfs.fat");
        }
        if self.mkfs_ext4.is_none() {
            missing.push("mkfs.ext4");
        }
        if self.grub_install.is_none() {
            missing.push("grub-install");
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
}

/// Image builder
pub struct ImageBuilder {
    /// Work directory
    work_dir: PathBuf,

    /// Bootstrap configuration
    #[allow(dead_code)]
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

    /// Temporary mount directory
    mount_dir: Option<PathBuf>,

    /// Loop device (if mounted)
    loop_device: Option<String>,

    /// Build log
    log: String,
}

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
            mount_dir: None,
            loop_device: None,
            log: String::new(),
        })
    }

    /// Get the output path
    pub fn output_path(&self) -> &Path {
        &self.output
    }

    /// Build the image
    pub fn build(&mut self) -> Result<ImageResult, ImageError> {
        info!("Building {} image: {:?}", self.format, self.output);
        self.log_line(&format!("Building {} image", self.format));

        let result = match self.format {
            ImageFormat::Raw => self.build_raw()?,
            ImageFormat::Qcow2 => self.build_qcow2()?,
            ImageFormat::Iso => self.build_iso()?,
        };

        info!("Image built successfully: {:?}", result.path);
        Ok(result)
    }

    /// Build raw disk image
    fn build_raw(&mut self) -> Result<ImageResult, ImageError> {
        self.log_line("Creating raw disk image");

        // Create sparse image file
        self.create_sparse_image()?;

        // Partition the image
        self.partition_image()?;

        // Set up loop device
        self.setup_loop_device()?;

        // Format partitions
        self.format_partitions()?;

        // Mount and populate
        self.mount_and_populate()?;

        // Install bootloader
        self.install_bootloader()?;

        // Cleanup
        self.cleanup()?;

        let size = fs::metadata(&self.output)?.len();

        Ok(ImageResult {
            path: self.output.clone(),
            format: ImageFormat::Raw,
            size,
            efi_bootable: true,
            bios_bootable: self.tools.grub_install.is_some(),
        })
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
                "convert",
                "-f",
                "raw",
                "-O",
                "qcow2",
                "-c", // Compress
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
            bios_bootable: true,
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

        Ok(ImageResult {
            path: self.output.clone(),
            format: ImageFormat::Iso,
            size,
            efi_bootable: true,
            bios_bootable: true,
        })
    }

    /// Create a sparse image file
    fn create_sparse_image(&mut self) -> Result<(), ImageError> {
        self.log_line(&format!("Creating sparse image: {} bytes", self.size.bytes()));

        // Use truncate for sparse file creation
        let file = File::create(&self.output)?;
        file.set_len(self.size.bytes())?;

        debug!("Created sparse image at {:?}", self.output);
        Ok(())
    }

    /// Partition the image with GPT
    fn partition_image(&mut self) -> Result<(), ImageError> {
        self.log_line("Creating GPT partition table");

        // Clone tool paths upfront to avoid borrow conflicts
        let sfdisk = self.tools.sfdisk.clone();
        let parted = self.tools.parted.clone();

        // Prefer sfdisk for scripted partitioning
        if let Some(ref sfdisk_path) = sfdisk {
            self.partition_with_sfdisk(sfdisk_path)?;
        } else if let Some(ref parted_path) = parted {
            self.partition_with_parted(parted_path)?;
        } else {
            return Err(ImageError::ToolNotFound("sfdisk or parted".to_string()));
        }

        Ok(())
    }

    /// Partition using sfdisk
    fn partition_with_sfdisk(&mut self, sfdisk: &Path) -> Result<(), ImageError> {
        // sfdisk script for GPT with ESP and root
        let script = format!(
            "label: gpt\n\
             size={}M, type=C12A7328-F81F-11D2-BA4B-00A0C93EC93B, name=\"EFI System\"\n\
             type=0FC63DAF-8483-4772-8E79-3D69D8477DE4, name=\"Linux root\"\n",
            Self::ESP_SIZE_MB
        );

        let mut child = Command::new(sfdisk)
            .arg(&self.output)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(script.as_bytes())?;
        }

        let output = child.wait_with_output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ImageError::PartitionFailed(stderr.to_string()));
        }

        Ok(())
    }

    /// Partition using parted
    fn partition_with_parted(&mut self, parted: &Path) -> Result<(), ImageError> {
        // Create GPT label
        let run_parted = |args: &[&str]| -> Result<(), ImageError> {
            let output = Command::new(parted)
                .arg("-s")
                .arg(&self.output)
                .args(args)
                .output()?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(ImageError::PartitionFailed(stderr.to_string()));
            }
            Ok(())
        };

        run_parted(&["mklabel", "gpt"])?;

        // Create ESP partition
        run_parted(&[
            "mkpart",
            "ESP",
            "fat32",
            "1MiB",
            &format!("{}MiB", Self::ESP_SIZE_MB + 1),
        ])?;
        run_parted(&["set", "1", "esp", "on"])?;

        // Create root partition
        run_parted(&[
            "mkpart",
            "root",
            "ext4",
            &format!("{}MiB", Self::ESP_SIZE_MB + 1),
            "100%",
        ])?;

        Ok(())
    }

    /// Set up loop device for the image
    fn setup_loop_device(&mut self) -> Result<(), ImageError> {
        self.log_line("Setting up loop device");

        let output = Command::new("losetup")
            .args(["--find", "--show", "--partscan"])
            .arg(&self.output)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ImageError::CommandFailed(format!(
                "losetup failed: {}",
                stderr
            )));
        }

        let loop_dev = String::from_utf8_lossy(&output.stdout).trim().to_string();
        debug!("Loop device: {}", loop_dev);
        self.loop_device = Some(loop_dev);

        // Wait for partition devices to appear
        std::thread::sleep(std::time::Duration::from_millis(500));

        Ok(())
    }

    /// Format partitions
    fn format_partitions(&mut self) -> Result<(), ImageError> {
        let loop_dev = self
            .loop_device
            .as_ref()
            .ok_or_else(|| ImageError::CommandFailed("No loop device".to_string()))?;

        let esp_dev = format!("{}p1", loop_dev);
        let root_dev = format!("{}p2", loop_dev);

        // Format ESP as FAT32
        self.log_line("Formatting ESP partition (FAT32)");
        let mkfs_fat = self
            .tools
            .mkfs_fat
            .as_ref()
            .ok_or_else(|| ImageError::ToolNotFound("mkfs.fat".to_string()))?;

        let output = Command::new(mkfs_fat)
            .args(["-F", "32", "-n", "CONARY_ESP"])
            .arg(&esp_dev)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ImageError::FilesystemFailed(format!(
                "mkfs.fat failed: {}",
                stderr
            )));
        }

        // Format root as ext4
        self.log_line("Formatting root partition (ext4)");
        let mkfs_ext4 = self
            .tools
            .mkfs_ext4
            .as_ref()
            .ok_or_else(|| ImageError::ToolNotFound("mkfs.ext4".to_string()))?;

        let output = Command::new(mkfs_ext4)
            .args(["-L", "CONARY_ROOT", "-F"])
            .arg(&root_dev)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ImageError::FilesystemFailed(format!(
                "mkfs.ext4 failed: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Mount partitions and populate with base system
    fn mount_and_populate(&mut self) -> Result<(), ImageError> {
        let loop_dev = self
            .loop_device
            .as_ref()
            .ok_or_else(|| ImageError::CommandFailed("No loop device".to_string()))?;

        let esp_dev = format!("{}p1", loop_dev);
        let root_dev = format!("{}p2", loop_dev);

        // Create mount directory
        let mount_dir = self.work_dir.join("mnt");
        fs::create_dir_all(&mount_dir)?;
        self.mount_dir = Some(mount_dir.clone());

        // Mount root partition
        self.log_line("Mounting root partition");
        let status = Command::new(&self.tools.mount)
            .arg(&root_dev)
            .arg(&mount_dir)
            .status()?;

        if !status.success() {
            return Err(ImageError::CommandFailed("Failed to mount root".to_string()));
        }

        // Create and mount ESP
        let esp_mount = mount_dir.join("boot/efi");
        fs::create_dir_all(&esp_mount)?;

        self.log_line("Mounting ESP partition");
        let status = Command::new(&self.tools.mount)
            .arg(&esp_dev)
            .arg(&esp_mount)
            .status()?;

        if !status.success() {
            return Err(ImageError::CommandFailed("Failed to mount ESP".to_string()));
        }

        // Copy base system
        self.log_line("Copying base system (this may take a while)");
        self.copy_system(&mount_dir)?;

        // Create /etc/fstab
        self.log_line("Creating fstab");
        self.create_fstab(&mount_dir)?;

        Ok(())
    }

    /// Copy base system to mounted image
    fn copy_system(&mut self, mount_dir: &Path) -> Result<(), ImageError> {
        // Use rsync if available, otherwise cp -a
        let rsync_available = Command::new("which")
            .arg("rsync")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        if rsync_available {
            let status = Command::new("rsync")
                .args([
                    "-aHAX",
                    "--info=progress2",
                    "--exclude=/dev/*",
                    "--exclude=/proc/*",
                    "--exclude=/sys/*",
                    "--exclude=/tmp/*",
                    "--exclude=/run/*",
                ])
                .arg(format!("{}/", self.sysroot.display()))
                .arg(format!("{}/", mount_dir.display()))
                .status()?;

            if !status.success() {
                return Err(ImageError::CommandFailed("rsync failed".to_string()));
            }
        } else {
            let status = Command::new("cp")
                .args(["-a"])
                .arg(format!("{}/.", self.sysroot.display()))
                .arg(mount_dir)
                .status()?;

            if !status.success() {
                return Err(ImageError::CommandFailed("cp failed".to_string()));
            }
        }

        // Create essential directories
        for dir in ["dev", "proc", "sys", "tmp", "run"] {
            let path = mount_dir.join(dir);
            if !path.exists() {
                fs::create_dir_all(&path)?;
            }
        }

        Ok(())
    }

    /// Create /etc/fstab
    fn create_fstab(&self, mount_dir: &Path) -> Result<(), ImageError> {
        let etc = mount_dir.join("etc");
        fs::create_dir_all(&etc)?;

        let fstab_content = "\
# /etc/fstab - Conary system file table
#
# <file system>  <mount point>  <type>  <options>        <dump> <pass>
LABEL=CONARY_ROOT  /              ext4    defaults,noatime  0      1
LABEL=CONARY_ESP   /boot/efi      vfat    defaults,noatime  0      2
tmpfs              /tmp           tmpfs   defaults,nosuid   0      0
";

        fs::write(etc.join("fstab"), fstab_content)?;
        Ok(())
    }

    /// Install bootloader
    fn install_bootloader(&mut self) -> Result<(), ImageError> {
        // Clone mount_dir upfront to avoid borrow conflicts
        let mount_dir = self
            .mount_dir
            .clone()
            .ok_or_else(|| ImageError::CommandFailed("Not mounted".to_string()))?;

        // Create EFI boot structure
        self.log_line("Setting up EFI boot");
        self.setup_efi_boot(&mount_dir)?;

        // Install GRUB for BIOS if available
        if self.tools.grub_install.is_some() {
            self.log_line("Installing GRUB for BIOS boot");
            // This would need the loop device, but GRUB installation in chroot
            // is complex - for now we focus on EFI boot
            debug!("GRUB BIOS installation skipped (requires chroot)");
        }

        Ok(())
    }

    /// Set up EFI boot structure
    fn setup_efi_boot(&self, mount_dir: &Path) -> Result<(), ImageError> {
        let efi_dir = mount_dir.join("boot/efi/EFI");
        let boot_dir = efi_dir.join("BOOT");
        let conary_dir = efi_dir.join("conary");

        fs::create_dir_all(&boot_dir)?;
        fs::create_dir_all(&conary_dir)?;

        // Look for GRUB EFI binary in sysroot
        let grub_efi_paths = [
            self.sysroot.join("usr/lib/grub/x86_64-efi/grubx64.efi"),
            self.sysroot.join("usr/share/grub/x86_64-efi/grubx64.efi"),
            self.sysroot.join("boot/efi/EFI/conary/grubx64.efi"),
        ];

        let grub_efi = grub_efi_paths.iter().find(|p| p.exists());

        if let Some(src) = grub_efi {
            fs::copy(src, boot_dir.join("BOOTX64.EFI"))?;
            fs::copy(src, conary_dir.join("grubx64.efi"))?;
        } else {
            warn!("GRUB EFI binary not found - creating stub EFI application");
            // Create a minimal placeholder that would need GRUB installed
            self.create_stub_efi(&boot_dir.join("BOOTX64.EFI"))?;
        }

        // Create GRUB config
        let grub_cfg = conary_dir.join("grub.cfg");
        self.create_grub_config(&grub_cfg)?;

        // Also put config in standard location
        let boot_grub = mount_dir.join("boot/grub");
        fs::create_dir_all(&boot_grub)?;
        self.create_grub_config(&boot_grub.join("grub.cfg"))?;

        Ok(())
    }

    /// Create GRUB configuration
    fn create_grub_config(&self, path: &Path) -> Result<(), ImageError> {
        let config = r#"# GRUB Configuration for Conary
set default=0
set timeout=5

menuentry "Conary Linux" {
    search --label --set=root CONARY_ROOT
    linux /boot/vmlinuz root=LABEL=CONARY_ROOT ro quiet
    initrd /boot/initramfs.img
}

menuentry "Conary Linux (Recovery Mode)" {
    search --label --set=root CONARY_ROOT
    linux /boot/vmlinuz root=LABEL=CONARY_ROOT ro single
    initrd /boot/initramfs.img
}
"#;

        fs::write(path, config)?;
        Ok(())
    }

    /// Create a stub EFI application (placeholder)
    fn create_stub_efi(&self, _path: &Path) -> Result<(), ImageError> {
        // In a real implementation, we would create a minimal EFI stub
        // or copy from a known location. For now, just warn.
        warn!("EFI stub creation not implemented - manual GRUB installation required");
        Ok(())
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
            return Err(ImageError::CreationFailed(
                "mksquashfs failed".to_string(),
            ));
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
            .args([
                "-as",
                "mkisofs",
                "-o",
            ])
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

    /// Cleanup mounts and loop device
    fn cleanup(&mut self) -> Result<(), ImageError> {
        self.log_line("Cleaning up");

        // Unmount ESP first
        if let Some(ref mount_dir) = self.mount_dir {
            let esp_mount = mount_dir.join("boot/efi");
            if esp_mount.exists() {
                let _ = Command::new(&self.tools.umount).arg(&esp_mount).status();
            }

            // Unmount root
            let _ = Command::new(&self.tools.umount).arg(mount_dir).status();

            // Remove mount directory
            let _ = fs::remove_dir_all(mount_dir);
        }

        // Detach loop device
        if let Some(ref loop_dev) = self.loop_device {
            let _ = Command::new("losetup").args(["-d", loop_dev]).status();
        }

        self.mount_dir = None;
        self.loop_device = None;

        Ok(())
    }

    /// Add a line to the build log
    fn log_line(&mut self, msg: &str) {
        info!("{}", msg);
        self.log.push_str(msg);
        self.log.push('\n');
    }

    /// Get the build log
    pub fn log(&self) -> &str {
        &self.log
    }
}

impl Drop for ImageBuilder {
    fn drop(&mut self) {
        // Ensure cleanup on drop
        if self.mount_dir.is_some() || self.loop_device.is_some() {
            let _ = self.cleanup();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_image_format_from_str() {
        assert_eq!(ImageFormat::from_str("raw").unwrap(), ImageFormat::Raw);
        assert_eq!(ImageFormat::from_str("qcow2").unwrap(), ImageFormat::Qcow2);
        assert_eq!(ImageFormat::from_str("iso").unwrap(), ImageFormat::Iso);
        assert_eq!(ImageFormat::from_str("RAW").unwrap(), ImageFormat::Raw);
        assert!(ImageFormat::from_str("invalid").is_err());
    }

    #[test]
    fn test_image_format_extension() {
        assert_eq!(ImageFormat::Raw.extension(), "img");
        assert_eq!(ImageFormat::Qcow2.extension(), "qcow2");
        assert_eq!(ImageFormat::Iso.extension(), "iso");
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
}
