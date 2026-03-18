# Boot Tools Recipes

These recipes build the bootloader and related utilities needed to
boot a Conary Linux system.

## Package List

| Package | Version | Description |
|---------|---------|-------------|
| grub | 2.14 | GRand Unified Bootloader (BIOS and UEFI) |
| efibootmgr | 19 | EFI Boot Manager for managing UEFI boot entries |
| efivar | 39 | Tools and library for EFI variables |
| popt | 1.19 | Command line option parsing library |
| dosfstools | 4.3 | FAT filesystem utilities (for EFI partitions) |

## Build Order

```
glibc, zlib, xz (from libs/)
         │
         ▼
    ┌─────────┐
    │  popt   │  (dependency for efivar/efibootmgr)
    └─────────┘
         │
    ┌────┴────┐
    ▼         ▼
┌────────┐ ┌────────────┐
│ efivar │ │ dosfstools │  (can build in parallel)
└────────┘ └────────────┘
    │
    ▼
┌─────────────┐
│ efibootmgr  │
└─────────────┘

    ┌─────────┐
    │  grub   │  (independent, can build in parallel with above)
    └─────────┘
```

## Building Boot Tools

```bash
# Build all boot tools (orchestrated)
conary bootstrap boot

# Or build individual packages
conary cook recipes/core/boot/popt.toml
conary cook recipes/core/boot/efivar.toml
conary cook recipes/core/boot/efibootmgr.toml
conary cook recipes/core/boot/dosfstools.toml
conary cook recipes/core/boot/grub.toml
```

## Package Details

### GRUB

GRUB (GRand Unified Bootloader) is the standard Linux bootloader.

**Installed commands:**
| Command | Description |
|---------|-------------|
| grub-install | Install GRUB to a device |
| grub-mkconfig | Generate GRUB configuration |
| update-grub | Helper script for grub-mkconfig |
| grub-mkimage | Create GRUB boot image |
| grub-probe | Probe device information |

**Supported platforms:**
- **BIOS (i386-pc)**: Traditional PC boot
- **UEFI (x86_64-efi)**: Modern UEFI boot

### efibootmgr

Manages UEFI boot entries stored in firmware NVRAM.

**Example usage:**
```bash
# List boot entries
efibootmgr -v

# Create new boot entry
efibootmgr -c -d /dev/sda -p 1 -L "Conary" -l '\EFI\conary\grubx64.efi'

# Change boot order
efibootmgr -o 0001,0002,0003

# Delete boot entry
efibootmgr -B -b 0004

# Set next boot (one-time)
efibootmgr -n 0002
```

### dosfstools

Tools for FAT filesystems, required for EFI System Partitions.

**Installed commands:**
| Command | Description |
|---------|-------------|
| mkfs.fat | Create FAT filesystem |
| mkfs.vfat | Alias for mkfs.fat |
| mkfs.msdos | Alias for mkfs.fat |
| fsck.fat | Check/repair FAT filesystem |
| fatlabel | Set FAT volume label |

## BIOS Installation

For traditional BIOS systems:

```bash
# Install GRUB to MBR
grub-install --target=i386-pc /dev/sda

# Generate configuration
grub-mkconfig -o /boot/grub/grub.cfg
```

**Partition layout (BIOS):**
```
/dev/sda1  /boot     ext4    500MB
/dev/sda2  /         ext4    remaining
```

## UEFI Installation

For modern UEFI systems:

```bash
# Create EFI System Partition (if needed)
mkfs.fat -F32 /dev/sda1

# Mount EFI partition
mount /dev/sda1 /boot/efi

# Install GRUB for UEFI
grub-install --target=x86_64-efi --efi-directory=/boot/efi --bootloader-id=conary

# Generate configuration
grub-mkconfig -o /boot/grub/grub.cfg
```

**Partition layout (UEFI with GPT):**
```
/dev/sda1  /boot/efi  vfat    512MB   (EFI System Partition)
/dev/sda2  /          ext4    remaining
```

## Configuration

### /etc/default/grub

Main GRUB configuration file:

```bash
# Default boot entry (0 = first)
GRUB_DEFAULT=0

# Menu timeout (seconds)
GRUB_TIMEOUT=5

# Kernel parameters for all entries
GRUB_CMDLINE_LINUX=""

# Kernel parameters for normal (non-recovery) entries
GRUB_CMDLINE_LINUX_DEFAULT="quiet"

# Distributor name
GRUB_DISTRIBUTOR="Conary"
```

After modifying, regenerate configuration:
```bash
update-grub
# or
grub-mkconfig -o /boot/grub/grub.cfg
```

### /etc/grub.d/

Custom menu entries and scripts:

| File | Purpose |
|------|---------|
| 00_header | GRUB environment setup |
| 10_linux | Auto-detect Linux kernels |
| 20_linux_xen | Xen hypervisor entries |
| 30_os-prober | Detect other OSes |
| 40_custom | Custom menu entries |

**Adding custom entry:**
```bash
cat >> /etc/grub.d/40_custom << 'EOF'
menuentry "Custom Entry" {
    set root=(hd0,1)
    linux /vmlinuz root=/dev/sda2
    initrd /initramfs.img
}
EOF
chmod +x /etc/grub.d/40_custom
update-grub
```

## Secure Boot

For Secure Boot support (UEFI), additional steps are needed:

1. Sign the GRUB EFI binary
2. Enroll signing keys in firmware
3. Use shim bootloader for Microsoft trust chain

This requires additional packages not included in basic bootstrap.

## Troubleshooting

### GRUB Rescue Shell

If GRUB can't find its modules:

```
grub rescue> set prefix=(hd0,1)/boot/grub
grub rescue> set root=(hd0,1)
grub rescue> insmod normal
grub rescue> normal
```

### Reinstalling GRUB

If GRUB is corrupted:

```bash
# Boot from live media, mount root filesystem
mount /dev/sda2 /mnt
mount /dev/sda1 /mnt/boot/efi  # UEFI only

# Bind mount system directories
mount --bind /dev /mnt/dev
mount --bind /proc /mnt/proc
mount --bind /sys /mnt/sys

# Chroot and reinstall
chroot /mnt
grub-install --target=x86_64-efi --efi-directory=/boot/efi
update-grub
exit

# Unmount and reboot
umount -R /mnt
reboot
```

### EFI Variables Not Available

If `/sys/firmware/efi` doesn't exist:
- System booted in BIOS mode, not UEFI
- Kernel compiled without EFI support

### Boot Entry Not Appearing

Check UEFI boot order:
```bash
efibootmgr -v
```

Verify EFI binary exists:
```bash
ls -la /boot/efi/EFI/conary/
```

## Verification

```bash
# Check GRUB version
grub-install --version

# Check EFI support (on UEFI systems)
efibootmgr --version
ls /sys/firmware/efi

# Check dosfstools
mkfs.fat --help | head -1

# List current boot configuration
efibootmgr -v
```

## Boot Process Overview

### BIOS Boot

```
BIOS → MBR → GRUB Stage 1 → GRUB Stage 1.5 → GRUB Stage 2 → Linux Kernel
```

### UEFI Boot

```
UEFI Firmware → EFI System Partition → grubx64.efi → Linux Kernel
```

## Summary

With boot tools installed, your Conary system has everything needed
to boot independently:

1. **GRUB**: Loads and starts the Linux kernel
2. **efibootmgr**: Manages UEFI firmware boot entries
3. **dosfstools**: Creates/maintains EFI System Partition

This completes the core bootstrap recipe set for a bootable Conary system.
