// conary-core/src/bootstrap/system_config.rs

//! Phase 4: System configuration (LFS Chapter 9)
//!
//! Configures the built system for booting: user accounts, networking,
//! fstab, locale, systemd targets, shell configuration, and the boot
//! artifacts Phase 5 copies onto the ESP. This phase transforms the
//! collection of built packages into a bootable system.
//!
//! Does NOT include SSH guest-validation configuration (`sshd_config`,
//! host keys, `authorized_keys`) -- that belongs in the self-host guest
//! profile applied after Tier 2.

use std::collections::HashSet;
use std::fs;
#[cfg(unix)]
use std::io::ErrorKind;
use std::io::Write;
use std::path::{Path, PathBuf};
use tracing::info;

/// Errors specific to system configuration.
#[derive(Debug, thiserror::Error)]
pub enum SystemConfigError {
    /// The target root directory does not exist.
    #[error("System root not found: {0}")]
    RootNotFound(String),

    /// A configuration step failed.
    #[error("Configuration failed: {0}")]
    ConfigFailed(String),

    /// I/O error during configuration.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(unix)]
fn ensure_symlink(target: &Path, link_path: &Path) -> Result<(), std::io::Error> {
    use std::os::unix::fs::symlink;

    if let Ok(metadata) = fs::symlink_metadata(link_path) {
        if metadata.file_type().is_symlink()
            && fs::read_link(link_path).ok().as_deref() == Some(target)
        {
            return Ok(());
        }

        if metadata.file_type().is_dir() {
            fs::remove_dir_all(link_path)?;
        } else {
            fs::remove_file(link_path)?;
        }
    }

    symlink(target, link_path)
}

fn detect_boot_kernel(root: &Path) -> Result<std::path::PathBuf, SystemConfigError> {
    let boot_dir = root.join("boot");
    let entries = fs::read_dir(&boot_dir).map_err(|e| {
        SystemConfigError::ConfigFailed(format!("boot directory not readable: {e}"))
    })?;

    let mut versioned = Vec::new();
    let mut unversioned = None;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name == "vmlinuz" {
            unversioned = Some(path);
        } else if name.starts_with("vmlinuz-") {
            versioned.push(path);
        }
    }

    versioned.sort();
    versioned.into_iter().next().or(unversioned).ok_or_else(|| {
        SystemConfigError::ConfigFailed(
            "kernel image not found under /boot (expected vmlinuz-* from Phase 3)".into(),
        )
    })
}

fn merge_named_entries(
    existing_path: &Path,
    baseline_entries: &[&str],
) -> Result<String, std::io::Error> {
    let mut seen = HashSet::new();
    let mut merged = Vec::new();

    for entry in baseline_entries {
        let name = entry.split(':').next().unwrap_or_default();
        if !name.is_empty() && seen.insert(name.to_string()) {
            merged.push((*entry).to_string());
        }
    }

    if let Ok(existing) = fs::read_to_string(existing_path) {
        for line in existing.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            let name = trimmed.split(':').next().unwrap_or_default();
            if !name.is_empty() && seen.insert(name.to_string()) {
                merged.push(trimmed.to_string());
            }
        }
    }

    Ok(merged.join("\n") + "\n")
}

fn detect_systemd_boot_efi(root: &Path) -> Result<std::path::PathBuf, SystemConfigError> {
    let candidates = [
        root.join("usr/lib/systemd/boot/efi/systemd-bootx64.efi"),
        root.join("usr/lib/systemd/boot/efi/systemd-bootaa64.efi"),
    ];

    candidates
        .into_iter()
        .find(|path| path.exists())
        .ok_or_else(|| {
            SystemConfigError::ConfigFailed(
                "systemd-boot EFI binary not found under /usr/lib/systemd/boot/efi".into(),
            )
        })
}

#[cfg(unix)]
fn ensure_exported_usr_merge_linker_layout(root: &Path) -> Result<(), SystemConfigError> {
    let usr = root.join("usr");
    let usr_lib = usr.join("lib");
    let usr_lib64 = usr.join("lib64");

    fs::create_dir_all(&usr_lib)?;
    match fs::symlink_metadata(&usr_lib64) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            // Exported generation rootfs projection creates /lib64 -> usr/lib64.
            // LFS stores the x86_64 loader in /usr/lib, so bridge that contract
            // without replacing a distro-style /usr/lib64 when one exists.
            std::os::unix::fs::symlink("lib", usr_lib64)?;
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

#[cfg(not(unix))]
fn ensure_exported_usr_merge_linker_layout(_root: &Path) -> Result<(), SystemConfigError> {
    Ok(())
}

const INITRAMFS_BINARIES: &[&str] = &[
    "usr/bin/bash",
    "usr/bin/basename",
    "usr/bin/cat",
    "usr/bin/mkdir",
    "usr/bin/mount",
    "usr/bin/readlink",
    "usr/sbin/mount.composefs",
    "usr/sbin/switch_root",
];

const INITRAMFS_LIBRARIES: &[&str] = &[
    "usr/lib/ld-linux-x86-64.so.2",
    "usr/lib/libblkid.so.1",
    "usr/lib/libc.so.6",
    "usr/lib/libcomposefs.so.1",
    "usr/lib/libcrypto.so.3",
    "usr/lib/libhistory.so.8",
    "usr/lib/libmount.so.1",
    "usr/lib/libncursesw.so.6",
    "usr/lib/libreadline.so.8",
];

const INITRAMFS_INIT: &str = r#"#!/bin/sh
PATH=/usr/bin:/usr/sbin:/bin:/sbin
export PATH

fail() {
    echo "conary-initramfs: $*" >&2
    exec /bin/sh
}

mkdir -p /proc /sys /dev /sysroot
mount -t proc proc /proc || fail "failed to mount /proc"
mount -t sysfs sysfs /sys || fail "failed to mount /sys"
mount -t devtmpfs devtmpfs /dev 2>/dev/null || true

ROOT_SPEC="PARTLABEL=CONARY_ROOT"
CONARY_GEN=""
for opt in $(cat /proc/cmdline); do
    case "$opt" in
        root=*) ROOT_SPEC="${opt#root=}" ;;
        conary.generation=*) CONARY_GEN="${opt#conary.generation=}" ;;
    esac
done

mount -t ext4 -o rw "$ROOT_SPEC" /sysroot || fail "failed to mount root filesystem $ROOT_SPEC"

if [ -z "$CONARY_GEN" ] && [ -L /sysroot/conary/current ]; then
    CONARY_GEN="$(basename "$(readlink /sysroot/conary/current)")"
fi

if [ -n "$CONARY_GEN" ]; then
    GEN_DIR="/sysroot/conary/generations/$CONARY_GEN"
    EROFS_IMG="$GEN_DIR/root.erofs"
    CAS_DIR="/sysroot/conary/objects"

    [ -f "$EROFS_IMG" ] || fail "generation $CONARY_GEN is missing root.erofs"
    mkdir -p /sysroot/conary/mnt
    mount -t composefs "$EROFS_IMG" /sysroot/conary/mnt -o "basedir=$CAS_DIR,verity_check=1" 2>/dev/null ||
        mount -t composefs "$EROFS_IMG" /sysroot/conary/mnt -o "basedir=$CAS_DIR" ||
        fail "composefs mount failed for generation $CONARY_GEN"

    if [ -d /sysroot/conary/mnt/usr ]; then
        mkdir -p /sysroot/usr
        mount --bind /sysroot/conary/mnt/usr /sysroot/usr || fail "failed to bind generation /usr"
        mount -o remount,ro /sysroot/usr 2>/dev/null || true
    fi

    if [ -d /sysroot/conary/mnt/etc ]; then
        ETC_UPPER="/sysroot/conary/etc-state/$CONARY_GEN"
        ETC_WORK="/sysroot/conary/etc-state/$CONARY_GEN-work"
        mkdir -p "$ETC_UPPER" "$ETC_WORK" /sysroot/etc
        mount -t overlay overlay /sysroot/etc \
            -o "lowerdir=/sysroot/conary/mnt/etc,upperdir=$ETC_UPPER,workdir=$ETC_WORK" ||
            fail "failed to mount generation /etc overlay"
    fi
fi

mkdir -p /sysroot/dev /sysroot/proc /sysroot/sys /sysroot/run
mount --move /dev /sysroot/dev 2>/dev/null || true
mount --move /proc /sysroot/proc 2>/dev/null || true
mount --move /sys /sysroot/sys 2>/dev/null || true
exec switch_root /sysroot /usr/lib/systemd/systemd
fail "switch_root failed"
"#;

#[cfg(unix)]
fn write_bootstrap_initramfs(root: &Path, initramfs_dest: &Path) -> Result<(), SystemConfigError> {
    let mut archive = Vec::new();
    let mut seen = HashSet::new();

    for dir in [
        "dev", "proc", "run", "sys", "sysroot", "tmp", "usr", "usr/bin", "usr/lib", "usr/sbin",
        "lib64",
    ] {
        append_cpio_dir(&mut archive, &mut seen, dir, 0o755)?;
    }

    append_cpio_symlink(&mut archive, &mut seen, "bin", "usr/bin")?;
    append_cpio_symlink(&mut archive, &mut seen, "sbin", "usr/sbin")?;
    append_cpio_symlink(&mut archive, &mut seen, "lib", "usr/lib")?;
    append_cpio_symlink(
        &mut archive,
        &mut seen,
        "lib64/ld-linux-x86-64.so.2",
        "../lib/ld-linux-x86-64.so.2",
    )?;

    append_cpio_file(
        &mut archive,
        &mut seen,
        "init",
        INITRAMFS_INIT.as_bytes(),
        0o755,
    )?;

    for rel in INITRAMFS_BINARIES.iter().chain(INITRAMFS_LIBRARIES.iter()) {
        append_sysroot_path(&mut archive, &mut seen, root, Path::new(rel))?;
    }

    if !seen.contains("usr/bin/sh") {
        append_cpio_symlink(&mut archive, &mut seen, "usr/bin/sh", "bash")?;
    }

    append_cpio_entry(&mut archive, "TRAILER!!!", 0, &[])?;
    fs::write(initramfs_dest, archive)?;
    Ok(())
}

#[cfg(not(unix))]
fn write_bootstrap_initramfs(
    _root: &Path,
    _initramfs_dest: &Path,
) -> Result<(), SystemConfigError> {
    Err(SystemConfigError::ConfigFailed(
        "bootstrap initramfs generation requires Unix filesystem metadata".into(),
    ))
}

#[cfg(unix)]
fn append_sysroot_path(
    archive: &mut Vec<u8>,
    seen: &mut HashSet<String>,
    root: &Path,
    rel: &Path,
) -> Result<(), SystemConfigError> {
    use std::os::unix::fs::PermissionsExt;

    let archive_name = path_to_archive_name(rel)?;
    if seen.contains(&archive_name) {
        return Ok(());
    }

    let source = root.join(rel);
    let metadata = fs::symlink_metadata(&source).map_err(|e| {
        SystemConfigError::ConfigFailed(format!(
            "required bootstrap initramfs input missing at {}: {e}",
            source.display()
        ))
    })?;

    if metadata.file_type().is_symlink() {
        let target = fs::read_link(&source)?;
        append_cpio_symlink(
            archive,
            seen,
            &archive_name,
            target.to_str().ok_or_else(|| {
                SystemConfigError::ConfigFailed(format!(
                    "bootstrap initramfs symlink target is not UTF-8: {}",
                    source.display()
                ))
            })?,
        )?;

        let resolved = if target.is_absolute() {
            root.join(target.strip_prefix("/").map_err(|_| {
                SystemConfigError::ConfigFailed(format!(
                    "invalid absolute symlink target for {}",
                    source.display()
                ))
            })?)
        } else {
            source
                .parent()
                .ok_or_else(|| {
                    SystemConfigError::ConfigFailed(format!(
                        "bootstrap initramfs symlink has no parent: {}",
                        source.display()
                    ))
                })?
                .join(target)
        };
        let resolved = normalize_path(&resolved);
        let resolved_rel = resolved.strip_prefix(root).map_err(|_| {
            SystemConfigError::ConfigFailed(format!(
                "bootstrap initramfs symlink {} resolves outside sysroot to {}",
                source.display(),
                resolved.display()
            ))
        })?;
        return append_sysroot_path(archive, seen, root, resolved_rel);
    }

    if metadata.is_dir() {
        append_cpio_dir(
            archive,
            seen,
            &archive_name,
            metadata.permissions().mode() & 0o777,
        )
    } else if metadata.is_file() {
        let bytes = fs::read(&source)?;
        append_cpio_file(
            archive,
            seen,
            &archive_name,
            &bytes,
            metadata.permissions().mode() & 0o777,
        )
    } else {
        Err(SystemConfigError::ConfigFailed(format!(
            "unsupported bootstrap initramfs input type at {}",
            source.display()
        )))
    }
}

#[cfg(unix)]
fn append_cpio_dir(
    archive: &mut Vec<u8>,
    seen: &mut HashSet<String>,
    name: &str,
    mode: u32,
) -> Result<(), SystemConfigError> {
    if seen.insert(name.to_string()) {
        append_cpio_entry(archive, name, 0o040000 | mode, &[])?;
    }
    Ok(())
}

#[cfg(unix)]
fn append_cpio_file(
    archive: &mut Vec<u8>,
    seen: &mut HashSet<String>,
    name: &str,
    bytes: &[u8],
    mode: u32,
) -> Result<(), SystemConfigError> {
    if seen.insert(name.to_string()) {
        append_cpio_entry(archive, name, 0o100000 | mode, bytes)?;
    }
    Ok(())
}

#[cfg(unix)]
fn append_cpio_symlink(
    archive: &mut Vec<u8>,
    seen: &mut HashSet<String>,
    name: &str,
    target: &str,
) -> Result<(), SystemConfigError> {
    if seen.insert(name.to_string()) {
        append_cpio_entry(archive, name, 0o120777, target.as_bytes())?;
    }
    Ok(())
}

#[cfg(unix)]
fn append_cpio_entry(
    archive: &mut Vec<u8>,
    name: &str,
    mode: u32,
    bytes: &[u8],
) -> Result<(), SystemConfigError> {
    let inode = (archive.len() as u32).wrapping_add(1);
    let namesize = name.len().checked_add(1).ok_or_else(|| {
        SystemConfigError::ConfigFailed("bootstrap initramfs entry name too large".into())
    })?;

    write!(
        archive,
        "070701{inode:08x}{mode:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{namesize:08x}{:08x}",
        0,
        0,
        1,
        0,
        bytes.len(),
        0,
        0,
        0,
        0,
        0,
    )
    .map_err(|e| SystemConfigError::ConfigFailed(format!("failed to write cpio header: {e}")))?;
    archive.extend_from_slice(name.as_bytes());
    archive.push(0);
    pad_cpio(archive);
    archive.extend_from_slice(bytes);
    pad_cpio(archive);
    Ok(())
}

#[cfg(unix)]
fn pad_cpio(archive: &mut Vec<u8>) {
    while !archive.len().is_multiple_of(4) {
        archive.push(0);
    }
}

#[cfg(unix)]
fn path_to_archive_name(path: &Path) -> Result<String, SystemConfigError> {
    let value = path.to_string_lossy().trim_start_matches('/').to_string();
    if value.is_empty() || value.contains("..") {
        return Err(SystemConfigError::ConfigFailed(format!(
            "invalid bootstrap initramfs archive path: {}",
            path.display()
        )));
    }
    Ok(value)
}

#[cfg(unix)]
fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn configure_boot_artifacts(root: &Path) -> Result<(), SystemConfigError> {
    let boot_dir = root.join("boot");
    fs::create_dir_all(&boot_dir)?;

    let kernel_src = detect_boot_kernel(root)?;
    let canonical_kernel = boot_dir.join("vmlinuz");
    if kernel_src != canonical_kernel {
        fs::copy(&kernel_src, &canonical_kernel)?;
    }

    let efi_dest = boot_dir.join("EFI/BOOT/BOOTX64.EFI");
    fs::create_dir_all(
        efi_dest
            .parent()
            .ok_or_else(|| SystemConfigError::ConfigFailed("invalid EFI destination".into()))?,
    )?;
    fs::copy(detect_systemd_boot_efi(root)?, &efi_dest)?;

    let initramfs_dest = boot_dir.join("initramfs.img");
    write_bootstrap_initramfs(root, &initramfs_dest)?;

    let loader_dir = boot_dir.join("loader/entries");
    fs::create_dir_all(&loader_dir)?;
    fs::write(
        boot_dir.join("loader/loader.conf"),
        "default conaryos\ntimeout 3\nconsole-mode max\neditor no\n",
    )?;
    fs::write(
        loader_dir.join("conaryos.conf"),
        "title   conaryOS\n\
         linux   /vmlinuz\n\
         initrd  /initramfs.img\n\
         options root=PARTLABEL=CONARY_ROOT rootfstype=ext4 rw console=tty0 console=ttyS0\n",
    )?;

    info!("Created boot artifacts (kernel copy, initramfs, loader config, and EFI binary)");
    Ok(())
}

/// Configure the final system for booting (LFS Chapter 9).
///
/// Creates essential system configuration files: user accounts, hostname,
/// os-release, fstab, networking, locale, readline, systemd targets, shell
/// prompt, and the systemd-boot artifacts Phase 5 copies onto the ESP.
///
/// # Arguments
///
/// * `root` - root directory of the LFS system to configure
///
/// # Errors
///
/// Returns `SystemConfigError::RootNotFound` if `root` does not exist.
pub fn configure_system(root: &Path) -> Result<(), SystemConfigError> {
    if !root.exists() {
        return Err(SystemConfigError::RootNotFound(format!(
            "Root directory does not exist: {}",
            root.display()
        )));
    }

    info!("Phase 4: Configuring system at {}", root.display());

    let etc = root.join("etc");
    fs::create_dir_all(&etc)?;

    // 1. /etc/passwd -- LFS systemd baseline users needed for core services.
    let passwd_path = etc.join("passwd");
    let passwd = merge_named_entries(
        &passwd_path,
        &[
            "root:x:0:0:root:/root:/bin/bash",
            "bin:x:1:1:bin:/dev/null:/usr/bin/false",
            "daemon:x:6:6:Daemon User:/dev/null:/usr/bin/false",
            "messagebus:x:18:18:D-Bus Message Daemon User:/run/dbus:/usr/bin/false",
            "systemd-journal-gateway:x:73:73:systemd Journal Gateway:/:/usr/bin/false",
            "systemd-journal-remote:x:74:74:systemd Journal Remote:/:/usr/bin/false",
            "systemd-journal-upload:x:75:75:systemd Journal Upload:/:/usr/bin/false",
            "systemd-network:x:76:76:systemd Network Management:/:/usr/bin/false",
            "systemd-resolve:x:77:77:systemd Resolver:/:/usr/bin/false",
            "systemd-timesync:x:78:78:systemd Time Synchronization:/:/usr/bin/false",
            "systemd-coredump:x:79:79:systemd Core Dumper:/:/usr/bin/false",
            "uuidd:x:80:80:UUID Generation Daemon User:/dev/null:/usr/bin/false",
            "systemd-oom:x:81:81:systemd Out Of Memory Daemon:/:/usr/bin/false",
            "nobody:x:65534:65534:Unprivileged User:/dev/null:/usr/bin/false",
        ],
    )?;
    fs::write(&passwd_path, passwd)?;
    info!("Created /etc/passwd");

    // 2. /etc/group -- LFS systemd baseline groups for core services and devices.
    let group_path = etc.join("group");
    let group = merge_named_entries(
        &group_path,
        &[
            "root:x:0:",
            "bin:x:1:daemon",
            "sys:x:2:",
            "kmem:x:3:",
            "tape:x:4:",
            "tty:x:5:",
            "daemon:x:6:",
            "floppy:x:7:",
            "disk:x:8:",
            "lp:x:9:",
            "dialout:x:10:",
            "audio:x:11:",
            "video:x:12:",
            "utmp:x:13:",
            "cdrom:x:15:",
            "adm:x:16:",
            "messagebus:x:18:",
            "systemd-journal:x:23:",
            "input:x:24:",
            "mail:x:34:",
            "kvm:x:61:",
            "systemd-journal-gateway:x:73:",
            "systemd-journal-remote:x:74:",
            "systemd-journal-upload:x:75:",
            "systemd-network:x:76:",
            "systemd-resolve:x:77:",
            "systemd-timesync:x:78:",
            "systemd-coredump:x:79:",
            "uuidd:x:80:",
            "systemd-oom:x:81:",
            "wheel:x:97:",
            "users:x:999:",
            "nogroup:x:65534:",
        ],
    )?;
    fs::write(&group_path, group)?;
    info!("Created /etc/group");

    // 3. /etc/shadow -- root with empty password (permits passwordless login)
    fs::write(
        etc.join("shadow"),
        "root::1:0:99999:7:::\nnobody:!:0:0:99999:7:::\n",
    )?;

    // Restrict shadow permissions (LFS 9.3)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(etc.join("shadow"), fs::Permissions::from_mode(0o600))?;
    }
    info!("Created /etc/shadow (mode 0600)");

    // 4. /etc/hostname (LFS 9.5)
    fs::write(etc.join("hostname"), "conaryos\n")?;
    info!("Created /etc/hostname");

    // 5. /etc/os-release -- required by systemd (LFS 9.2)
    fs::write(
        etc.join("os-release"),
        "NAME=\"conaryOS\"\n\
         ID=conaryos\n\
         VERSION_ID=0.1\n\
         PRETTY_NAME=\"conaryOS 0.1 (Bootstrap)\"\n\
         HOME_URL=\"https://conaryos.com\"\n",
    )?;
    info!("Created /etc/os-release");

    // 6. /etc/machine-id -- empty file, systemd generates on first boot.
    // Re-runs must tolerate a read-only machine-id from an existing sysroot.
    let machine_id_path = etc.join("machine-id");
    #[cfg(unix)]
    if machine_id_path.exists() {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&machine_id_path, fs::Permissions::from_mode(0o644))?;
    }
    fs::write(&machine_id_path, "")?;
    info!("Created /etc/machine-id (empty, systemd fills on first boot)");

    // 7. /etc/fstab (LFS 10.2)
    fs::write(
        etc.join("fstab"),
        "# /etc/fstab - conaryOS\n\
         PARTLABEL=CONARY_ROOT  /      ext4  defaults,noatime  0 1\n\
         PARTLABEL=CONARY_ESP   /boot  vfat  defaults,noatime  0 2\n\
         tmpfs              /tmp   tmpfs defaults,nosuid   0 0\n",
    )?;
    info!("Created /etc/fstab");

    // 8. /etc/nsswitch.conf -- required for name resolution (LFS 9.2)
    fs::write(
        etc.join("nsswitch.conf"),
        "passwd: files\n\
         group:  files\n\
         shadow: files\n\
         hosts:  files dns\n",
    )?;
    info!("Created /etc/nsswitch.conf");

    // 9. /etc/locale.conf -- locale configuration (LFS 9.7)
    fs::write(etc.join("locale.conf"), "LANG=en_US.UTF-8\n")?;
    info!("Created /etc/locale.conf");

    // 10. /etc/inputrc -- readline configuration (LFS 9.8)
    fs::write(
        etc.join("inputrc"),
        "# /etc/inputrc - conaryOS readline configuration\n\
         # See readline(3readline) and `info rstripping am am am readline' for more information.\n\
         \n\
         # Be 8 bit clean.\n\
         set input-meta on\n\
         set output-meta on\n\
         set convert-meta off\n\
         \n\
         # Allow the command prompt to wrap to the next line.\n\
         set horizontal-scroll-mode off\n\
         \n\
         # Try to enable the application keypad when it is called.\n\
         set enable-keypad on\n\
         \n\
         # Completion options.\n\
         set show-all-if-ambiguous on\n\
         set completion-ignore-case on\n\
         set colored-stats on\n\
         \n\
         # Mappings for \"page up\" and \"page down\" to search the history.\n\
         \"\\e[5~\": history-search-backward\n\
         \"\\e[6~\": history-search-forward\n\
         \n\
         # Mappings for Ctrl+left and Ctrl+right for word movement.\n\
         \"\\e[1;5C\": forward-word\n\
         \"\\e[1;5D\": backward-word\n",
    )?;
    info!("Created /etc/inputrc");

    // 11. systemd-networkd DHCP config for all ethernet interfaces (LFS 9.5)
    let networkd_dir = etc.join("systemd/network");
    fs::create_dir_all(&networkd_dir)?;
    fs::write(
        networkd_dir.join("80-dhcp.network"),
        "[Match]\n\
         Name=en*\n\n\
         [Network]\n\
         DHCP=yes\n",
    )?;
    info!("Created systemd-networkd DHCP configuration");

    // 12. Systemd service wiring (LFS 9.10)
    let systemd_system = etc.join("systemd/system");
    fs::create_dir_all(systemd_system.join("multi-user.target.wants"))?;
    fs::create_dir_all(systemd_system.join("getty.target.wants"))?;

    #[cfg(unix)]
    {
        // default.target -> multi-user.target
        ensure_symlink(
            Path::new("/usr/lib/systemd/system/multi-user.target"),
            &systemd_system.join("default.target"),
        )?;

        // Enable systemd-networkd
        ensure_symlink(
            Path::new("/usr/lib/systemd/system/systemd-networkd.service"),
            &systemd_system.join("multi-user.target.wants/systemd-networkd.service"),
        )?;

        // Enable systemd-resolved so the guest gets a live resolver stub.
        ensure_symlink(
            Path::new("/usr/lib/systemd/system/systemd-resolved.service"),
            &systemd_system.join("multi-user.target.wants/systemd-resolved.service"),
        )?;

        // Enable serial console for QEMU -nographic
        ensure_symlink(
            Path::new("/usr/lib/systemd/system/serial-getty@.service"),
            &systemd_system.join("getty.target.wants/serial-getty@ttyS0.service"),
        )?;

        // EFI variable storage is optional for the bootstrap VM path. Mask the
        // generated efivars mount so boot does not depend on kernel/runtime
        // efivarfs support being healthy inside the guest.
        ensure_symlink(
            Path::new("/dev/null"),
            &systemd_system.join("sys-firmware-efi-efivars.mount"),
        )?;

        // Use the systemd-resolved stub at boot time.
        ensure_symlink(
            Path::new("/run/systemd/resolve/stub-resolv.conf"),
            &etc.join("resolv.conf"),
        )?;
    }
    info!("Created systemd service symlinks (default.target, networkd, resolved, serial-getty)");

    // 13. /root/.bashrc -- minimal shell prompt
    let root_home = root.join("root");
    fs::create_dir_all(&root_home)?;
    fs::write(
        root_home.join(".bashrc"),
        "export PS1='[\\u@\\h \\W]\\$ '\n\
         alias ls='ls --color=auto'\n",
    )?;
    info!("Created /root/.bashrc");

    // 14. systemd-boot + BLS artifacts for Phase 5 image generation.
    ensure_exported_usr_merge_linker_layout(root)?;
    configure_boot_artifacts(root)?;

    info!("Phase 4 complete: system configuration applied");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed_minimal_boot_inputs(root: &Path) {
        let boot_dir = root.join("boot");
        let efi_dir = root.join("usr/lib/systemd/boot/efi");
        std::fs::create_dir_all(&boot_dir).unwrap();
        std::fs::create_dir_all(&efi_dir).unwrap();
        std::fs::write(boot_dir.join("vmlinuz-6.19.8-conary"), b"kernel").unwrap();
        std::fs::write(efi_dir.join("systemd-bootx64.efi"), b"efi").unwrap();

        for rel in INITRAMFS_BINARIES.iter().chain(INITRAMFS_LIBRARIES.iter()) {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, format!("fake initramfs input: {rel}\n")).unwrap();
        }

        let sh = root.join("usr/bin/sh");
        let _ = std::fs::remove_file(&sh);
        #[cfg(unix)]
        std::os::unix::fs::symlink("bash", sh).unwrap();
        #[cfg(not(unix))]
        std::fs::write(sh, b"fake sh").unwrap();
    }

    #[test]
    fn test_configure_nonexistent_root() {
        let result = configure_system(Path::new("/nonexistent/root/path"));
        assert!(result.is_err());
        match result.unwrap_err() {
            SystemConfigError::RootNotFound(msg) => {
                assert!(msg.contains("/nonexistent/root/path"));
            }
            other => panic!("Expected RootNotFound, got: {other}"),
        }
    }

    #[test]
    fn test_configure_system_creates_all_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("sysroot");
        std::fs::create_dir_all(&root).unwrap();
        seed_minimal_boot_inputs(&root);

        configure_system(&root).unwrap();

        // Core identity files
        assert!(root.join("etc/passwd").exists());
        assert!(root.join("etc/group").exists());
        assert!(root.join("etc/shadow").exists());
        assert!(root.join("etc/hostname").exists());
        assert!(root.join("etc/os-release").exists());
        assert!(root.join("etc/machine-id").exists());
        assert!(root.join("etc/fstab").exists());
        assert!(root.join("etc/nsswitch.conf").exists());
        assert!(root.join("etc/resolv.conf").exists());

        // LFS Ch9 locale and readline
        assert!(root.join("etc/locale.conf").exists());
        assert!(root.join("etc/inputrc").exists());

        // Networking
        assert!(root.join("etc/systemd/network/80-dhcp.network").exists());

        // Shell
        assert!(root.join("root/.bashrc").exists());

        // Verify content
        let passwd = std::fs::read_to_string(root.join("etc/passwd")).unwrap();
        assert!(passwd.contains("root:x:0:0"));
        assert!(passwd.contains("nobody:x:65534"));

        let group = std::fs::read_to_string(root.join("etc/group")).unwrap();
        assert!(group.contains("root:x:0"));
        assert!(group.contains("wheel:x:97"));

        let hostname = std::fs::read_to_string(root.join("etc/hostname")).unwrap();
        assert_eq!(hostname.trim(), "conaryos");

        let os_release = std::fs::read_to_string(root.join("etc/os-release")).unwrap();
        assert!(os_release.contains("conaryOS"));
        assert!(os_release.contains("conaryos.com"));

        let machine_id = std::fs::read_to_string(root.join("etc/machine-id")).unwrap();
        assert!(machine_id.is_empty());

        let fstab = std::fs::read_to_string(root.join("etc/fstab")).unwrap();
        assert!(fstab.contains("PARTLABEL=CONARY_ROOT"));
        assert!(fstab.contains("PARTLABEL=CONARY_ESP"));
        assert!(fstab.contains("tmpfs"));

        let nsswitch = std::fs::read_to_string(root.join("etc/nsswitch.conf")).unwrap();
        assert!(nsswitch.contains("hosts:"));
        assert!(nsswitch.contains("files dns"));

        let passwd = std::fs::read_to_string(root.join("etc/passwd")).unwrap();
        assert!(passwd.contains("messagebus:x:18:18:"));
        assert!(passwd.contains("systemd-network:x:76:76:"));
        assert!(passwd.contains("systemd-resolve:x:77:77:"));
        assert!(passwd.contains("systemd-timesync:x:78:78:"));
        assert!(passwd.contains("uuidd:x:80:80:"));

        let group = std::fs::read_to_string(root.join("etc/group")).unwrap();
        assert!(group.contains("messagebus:x:18:"));
        assert!(group.contains("systemd-network:x:76:"));
        assert!(group.contains("systemd-resolve:x:77:"));
        assert!(group.contains("systemd-timesync:x:78:"));
        assert!(group.contains("uuidd:x:80:"));

        let locale = std::fs::read_to_string(root.join("etc/locale.conf")).unwrap();
        assert!(locale.contains("LANG=en_US.UTF-8"));

        let inputrc = std::fs::read_to_string(root.join("etc/inputrc")).unwrap();
        assert!(inputrc.contains("set input-meta on"));
        assert!(inputrc.contains("show-all-if-ambiguous"));

        let dhcp =
            std::fs::read_to_string(root.join("etc/systemd/network/80-dhcp.network")).unwrap();
        assert!(dhcp.contains("[Match]"));
        assert!(dhcp.contains("DHCP=yes"));

        let bashrc = std::fs::read_to_string(root.join("root/.bashrc")).unwrap();
        assert!(bashrc.contains("PS1="));

        // No SSH config should exist (Tier 2 territory)
        assert!(!root.join("etc/ssh/sshd_config").exists());
    }

    #[test]
    fn test_configure_system_root_shadow_does_not_force_password_rotation() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("sysroot");
        std::fs::create_dir_all(&root).unwrap();
        seed_minimal_boot_inputs(&root);

        configure_system(&root).unwrap();

        let shadow = std::fs::read_to_string(root.join("etc/shadow")).unwrap();
        assert!(
            shadow.lines().any(|line| line == "root::1:0:99999:7:::"),
            "empty root password needs a nonzero last-change day so login works without first-login rotation"
        );
    }

    #[test]
    fn test_configure_system_creates_bootloader_artifacts_from_versioned_kernel() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("sysroot");
        std::fs::create_dir_all(&root).unwrap();
        seed_minimal_boot_inputs(&root);

        configure_system(&root).unwrap();

        assert_eq!(std::fs::read(root.join("boot/vmlinuz")).unwrap(), b"kernel");
        assert_eq!(
            std::fs::read(root.join("boot/EFI/BOOT/BOOTX64.EFI")).unwrap(),
            b"efi"
        );

        let loader = std::fs::read_to_string(root.join("boot/loader/loader.conf")).unwrap();
        assert!(loader.contains("default conaryos"));

        let entry =
            std::fs::read_to_string(root.join("boot/loader/entries/conaryos.conf")).unwrap();
        assert!(entry.contains("title   conaryOS"));
        assert!(entry.contains("linux   /vmlinuz"));
        assert!(entry.contains("initrd  /initramfs.img"));
        assert!(entry.contains("root=PARTLABEL=CONARY_ROOT"));
        assert!(entry.contains("console=ttyS0"));
        assert!(root.join("boot/initramfs.img").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn test_configure_system_bridges_lib64_to_usr_lib_for_exported_generations() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("sysroot");
        std::fs::create_dir_all(&root).unwrap();
        seed_minimal_boot_inputs(&root);

        configure_system(&root).unwrap();

        assert_eq!(
            std::fs::read_link(root.join("usr/lib64")).unwrap(),
            PathBuf::from("lib")
        );
        assert!(
            root.join("usr/lib64/ld-linux-x86-64.so.2").exists(),
            "exported generations expose /lib64 through usr/lib64, so the ELF interpreter must resolve there"
        );
    }

    #[test]
    fn test_configure_system_preserves_package_created_service_accounts() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("sysroot");
        let etc = root.join("etc");
        std::fs::create_dir_all(&etc).unwrap();
        seed_minimal_boot_inputs(&root);

        std::fs::write(
            etc.join("passwd"),
            "sshd:x:50:50:sshd PrivSep:/var/lib/sshd:/bin/false\n",
        )
        .unwrap();
        std::fs::write(etc.join("group"), "sshd:x:50:\n").unwrap();

        configure_system(&root).unwrap();

        let passwd = std::fs::read_to_string(etc.join("passwd")).unwrap();
        assert!(passwd.contains("root:x:0:0:root:/root:/bin/bash"));
        assert!(passwd.contains("sshd:x:50:50:sshd PrivSep:/var/lib/sshd:/bin/false"));

        let group = std::fs::read_to_string(etc.join("group")).unwrap();
        assert!(group.contains("wheel:x:97:"));
        assert!(group.contains("sshd:x:50:"));
    }

    #[test]
    fn test_configure_system_fails_without_kernel_boot_input() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("sysroot");
        let efi_dir = root.join("usr/lib/systemd/boot/efi");
        std::fs::create_dir_all(&efi_dir).unwrap();
        std::fs::write(efi_dir.join("systemd-bootx64.efi"), b"efi").unwrap();

        let err = configure_system(&root).unwrap_err();
        assert!(matches!(err, SystemConfigError::ConfigFailed(_)));
        assert!(err.to_string().contains("kernel"));
    }

    #[test]
    fn test_configure_system_shadow_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("sysroot");
        std::fs::create_dir_all(&root).unwrap();
        seed_minimal_boot_inputs(&root);

        configure_system(&root).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let metadata = std::fs::metadata(root.join("etc/shadow")).unwrap();
            let mode = metadata.permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "shadow should have mode 0600, got {mode:o}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_configure_system_tolerates_read_only_machine_id() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("sysroot");
        let etc = root.join("etc");
        std::fs::create_dir_all(&etc).unwrap();
        seed_minimal_boot_inputs(&root);

        let machine_id = etc.join("machine-id");
        std::fs::write(&machine_id, "").unwrap();
        std::fs::set_permissions(&machine_id, std::fs::Permissions::from_mode(0o444)).unwrap();

        configure_system(&root).unwrap();

        let metadata = std::fs::metadata(&machine_id).unwrap();
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(mode, 0o644, "machine-id should be made writable for reruns");
        assert!(std::fs::read_to_string(&machine_id).unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn test_configure_system_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("sysroot");
        std::fs::create_dir_all(&root).unwrap();
        seed_minimal_boot_inputs(&root);

        configure_system(&root).unwrap();

        // Verify symlinks exist
        assert!(root.join("etc/systemd/system/default.target").exists());
        assert!(
            root.join("etc/systemd/system/multi-user.target.wants/systemd-networkd.service")
                .exists()
        );
        assert!(
            root.join("etc/systemd/system/multi-user.target.wants/systemd-resolved.service")
                .exists()
        );
        assert!(
            root.join("etc/systemd/system/getty.target.wants/serial-getty@ttyS0.service")
                .exists()
        );
        assert!(
            root.join("etc/systemd/system/sys-firmware-efi-efivars.mount")
                .exists()
        );

        // Verify symlink targets
        let target = std::fs::read_link(root.join("etc/systemd/system/default.target")).unwrap();
        assert_eq!(
            target.to_str().unwrap(),
            "/usr/lib/systemd/system/multi-user.target"
        );
        let resolv = std::fs::read_link(root.join("etc/resolv.conf")).unwrap();
        assert_eq!(
            resolv.to_str().unwrap(),
            "/run/systemd/resolve/stub-resolv.conf"
        );
        let efivars_mask =
            std::fs::read_link(root.join("etc/systemd/system/sys-firmware-efi-efivars.mount"))
                .unwrap();
        assert_eq!(efivars_mask.to_str().unwrap(), "/dev/null");
    }

    #[cfg(unix)]
    #[test]
    fn test_configure_system_tolerates_preexisting_systemd_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("sysroot");
        let systemd_system = root.join("etc/systemd/system");
        std::fs::create_dir_all(systemd_system.join("multi-user.target.wants")).unwrap();
        std::fs::create_dir_all(systemd_system.join("getty.target.wants")).unwrap();
        seed_minimal_boot_inputs(&root);

        std::os::unix::fs::symlink(
            "/usr/lib/systemd/system/multi-user.target",
            systemd_system.join("default.target"),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            "/usr/lib/systemd/system/systemd-networkd.service",
            systemd_system.join("multi-user.target.wants/systemd-networkd.service"),
        )
        .unwrap();

        configure_system(&root).unwrap();

        let default_target = std::fs::read_link(systemd_system.join("default.target")).unwrap();
        assert_eq!(
            default_target.to_str().unwrap(),
            "/usr/lib/systemd/system/multi-user.target"
        );
        let networkd_target = std::fs::read_link(
            systemd_system.join("multi-user.target.wants/systemd-networkd.service"),
        )
        .unwrap();
        assert_eq!(
            networkd_target.to_str().unwrap(),
            "/usr/lib/systemd/system/systemd-networkd.service"
        );
        let resolved_target = std::fs::read_link(
            systemd_system.join("multi-user.target.wants/systemd-resolved.service"),
        )
        .unwrap();
        assert_eq!(
            resolved_target.to_str().unwrap(),
            "/usr/lib/systemd/system/systemd-resolved.service"
        );
        let serial_getty = std::fs::read_link(
            systemd_system.join("getty.target.wants/serial-getty@ttyS0.service"),
        )
        .unwrap();
        assert_eq!(
            serial_getty.to_str().unwrap(),
            "/usr/lib/systemd/system/serial-getty@.service"
        );
        let efivars_mask =
            std::fs::read_link(systemd_system.join("sys-firmware-efi-efivars.mount")).unwrap();
        assert_eq!(efivars_mask.to_str().unwrap(), "/dev/null");
    }
}
