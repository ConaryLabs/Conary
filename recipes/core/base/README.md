# Base System Recipes

The base system contains the minimum packages needed for a bootable,
functional Conary system. These recipes are built after Stage 1
(self-hosted toolchain) is complete.

## Package Categories

### Kernel
| Package | Version | Description |
|---------|---------|-------------|
| linux | 6.18 | Linux kernel with Conary defaults |

### Core Utilities
| Package | Version | Description |
|---------|---------|-------------|
| coreutils | 9.9 | GNU core utilities (ls, cp, mv, etc.) |
| bash | 5.3 | GNU Bourne-Again Shell |
| util-linux | 2.41.3 | System utilities (mount, fdisk, etc.) |

### Init & Services
| Package | Version | Description |
|---------|---------|-------------|
| systemd | 257.9 | System and service manager |

### Networking
| Package | Version | Description |
|---------|---------|-------------|
| iproute2 | 6.15.0 | Network configuration (ip, ss, tc) |
| openssh | 10.1p1 | Secure Shell client and server |

## Build Order

The base system should be built in this order to satisfy dependencies:

```
Stage 1 Toolchain (must be complete)
         │
         ▼
    ┌─────────┐
    │  linux  │  (kernel - can build in parallel with userspace)
    └─────────┘

         ▼
    ┌─────────────┐
    │  coreutils  │  (basic utilities)
    └─────────────┘
         │
         ▼
    ┌─────────────┐
    │    bash     │  (shell, needs readline/ncurses)
    └─────────────┘
         │
         ▼
    ┌─────────────┐
    │ util-linux  │  (system utilities)
    └─────────────┘
         │
         ▼
    ┌─────────────┐
    │   systemd   │  (init system, needs many deps)
    └─────────────┘
         │
    ┌────┴────┐
    ▼         ▼
┌──────────┐ ┌─────────┐
│ iproute2 │ │ openssh │  (networking, can build in parallel)
└──────────┘ └─────────┘
```

## Building the Base System

```bash
# Ensure Stage 1 toolchain is complete
conary bootstrap status

# Build all base packages (orchestrated)
conary bootstrap base

# Or build individual packages
conary cook recipes/core/base/linux.toml
conary cook recipes/core/base/coreutils.toml
conary cook recipes/core/base/bash.toml
conary cook recipes/core/base/util-linux.toml
conary cook recipes/core/base/systemd.toml
conary cook recipes/core/base/iproute2.toml
conary cook recipes/core/base/openssh.toml
```

## Additional Dependencies

These packages also need recipes but are dependencies of the core packages:

### Libraries
- readline (bash dependency)
- ncurses (bash, util-linux dependency)
- zlib (various)
- xz (kernel, systemd)
- zstd (systemd)
- openssl (openssh, systemd)
- libcap (systemd, coreutils)
- libmnl (iproute2)
- libelf (kernel, iproute2)

### System
- kmod (kernel module loading)
- dbus (systemd dependency)
- pam (openssh, util-linux)

## Verification

After building the base system:

```bash
# Check kernel is bootable
file /boot/vmlinuz

# Check init is linked
ls -la /sbin/init

# Verify essential tools
/bin/ls --version
/bin/bash --version
/sbin/ip -V
/usr/sbin/sshd -V

# Check systemd
systemctl --version
```

## Creating a Bootable Image

After base system is complete:

```bash
# Generate initramfs
dracut --force /boot/initramfs-$(uname -r).img $(uname -r)

# Install bootloader (UEFI)
bootctl install

# Or for BIOS/Legacy
grub-install /dev/sda
grub-mkconfig -o /boot/grub/grub.cfg
```

## System Configuration

Basic configuration files are installed by the recipes:

- `/etc/ssh/sshd_config` - SSH server configuration
- `/etc/skel/.bashrc` - Default user shell config
- `/etc/skel/.bash_profile` - Default login shell config

Post-install configuration needed:

```bash
# Set hostname
hostnamectl set-hostname myconary

# Set timezone
timedatectl set-timezone America/New_York

# Create user
useradd -m -G wheel username
passwd username

# Enable services
systemctl enable sshd
systemctl enable systemd-networkd
systemctl enable systemd-resolved
```

## Next Steps

After the base system is bootable, proceed to build additional packages:

1. **Development tools** - make, autoconf, pkg-config
2. **Compression** - gzip, bzip2, tar
3. **Text processing** - grep, sed, awk, less
4. **Networking** - curl, wget, dns tools
5. **Package management** - conary itself!
