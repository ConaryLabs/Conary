// apps/conary/src/commands/bootstrap/image.rs

use std::str::FromStr;

use anyhow::{Context, Result};
use conary_core::bootstrap::{
    Bootstrap, BootstrapConfig, ImageBuilder, ImageFormat, ImageSize, ImageTools,
};

/// Generate bootable image
pub async fn cmd_bootstrap_image(
    work_dir: &str,
    output: &str,
    format: &str,
    size: &str,
) -> Result<()> {
    println!("Generating bootable image...");
    println!("  Work directory: {}", work_dir);
    println!("  Output: {}", output);
    println!("  Format: {}", format);
    println!("  Size: {}", size);

    // Parse format
    let image_format = ImageFormat::from_str(format)
        .context("Invalid image format. Use: raw, qcow2, iso, erofs")?;

    // Parse size
    let image_size = ImageSize::from_str(size).context("Invalid size. Use: 4G, 8G, 512M, etc.")?;

    // Check prerequisites
    println!("\nChecking required tools...");
    let tools = ImageTools::check()?;

    if let Err(e) = tools.check_for_format(image_format) {
        println!("[ERROR] {}", e);
        println!("\nRequired tools for {} format:", image_format);
        match image_format {
            ImageFormat::Raw | ImageFormat::Qcow2 => {
                println!("  - systemd-repart (GPT image creation and filesystem population)");
                if image_format == ImageFormat::Qcow2 {
                    println!("  - qemu-img (format conversion)");
                }
            }
            ImageFormat::Iso => {
                println!("  - xorriso (ISO creation)");
                println!("  - mksquashfs (squashfs creation)");
            }
            ImageFormat::Erofs => {
                println!("  (no external tools required -- composefs-rs builds in userspace)");
            }
        }
        return Err(e.into());
    }
    println!("[OK] All required tools found.");

    // Check if base system exists
    let bootstrap = Bootstrap::new(work_dir)?;
    let Some(sysroot) = bootstrap.get_sysroot() else {
        println!("[ERROR] Base system not found.");
        println!("Run 'conary bootstrap system' first to build the base system.");
        return Err(anyhow::anyhow!("Base system not complete"));
    };
    println!("  Base system: {}", sysroot.display());

    // Build the image
    match image_format {
        ImageFormat::Erofs => {
            println!("\nThis will create composefs-native output (EROFS + CAS + DB).");
            println!("Output directory: {}", output);
        }
        _ => {
            println!("\nThis will create a bootable {} image.", image_format);
            println!("Image size: {}", image_size);
        }
    }
    println!();

    let config = BootstrapConfig::new();
    let mut builder = ImageBuilder::new(
        work_dir,
        &config,
        &sysroot,
        output,
        image_format,
        image_size,
    )?;

    let result = builder.build()?;

    println!("\n[OK] Image generated successfully!");
    println!("  Path: {}", result.path.display());
    println!("  Format: {}", result.format);
    println!(
        "  Size: {} bytes ({:.1} GB)",
        result.size,
        result.size as f64 / 1_073_741_824.0
    );
    println!("  Method: {}", result.method);

    match image_format {
        ImageFormat::Erofs => {
            println!("\nOutput layout:");
            for desc in &result.partitions {
                println!("  - {desc}");
            }
            println!("\nThis is generation 1 -- the same artifact type as runtime generations.");
            println!("To export it as a bootable disk image, run:");
            println!(
                "  conary system generation export --path {}/generations/1 --format qcow2 --output conaryos.qcow2",
                output
            );
        }
        _ => {
            println!(
                "  EFI bootable: {}",
                if result.efi_bootable { "yes" } else { "no" }
            );
            println!(
                "  BIOS bootable: {}",
                if result.bios_bootable { "yes" } else { "no" }
            );
            println!("\nUsage:");
            match image_format {
                ImageFormat::Raw => {
                    println!(
                        "  QEMU: qemu-system-x86_64 -drive file={},format=raw -m 2G -enable-kvm",
                        output
                    );
                    println!(
                        "  USB:  sudo dd if={} of=/dev/sdX bs=4M status=progress",
                        output
                    );
                }
                ImageFormat::Qcow2 => {
                    println!(
                        "  QEMU: qemu-system-x86_64 -drive file={},format=qcow2 -m 2G -enable-kvm",
                        output
                    );
                }
                ImageFormat::Iso => {
                    println!(
                        "  QEMU: qemu-system-x86_64 -cdrom {} -m 2G -enable-kvm",
                        output
                    );
                    println!(
                        "  USB:  sudo dd if={} of=/dev/sdX bs=4M status=progress",
                        output
                    );
                }
                ImageFormat::Erofs => unreachable!(),
            }
        }
    }

    Ok(())
}
