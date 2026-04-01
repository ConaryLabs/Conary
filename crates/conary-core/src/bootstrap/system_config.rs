// conary-core/src/bootstrap/system_config.rs

//! Phase 4: System configuration (LFS Chapter 9)
//!
//! Configures the built system for booting: user accounts, networking,
//! fstab, locale, systemd targets, and shell configuration. This phase
//! transforms the collection of built packages into a bootable system.
//!
//! Does NOT include SSH configuration (sshd_config, host keys,
//! authorized_keys) -- that belongs in Tier 2.

use std::fs;
use std::path::Path;
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

/// Configure the final system for booting (LFS Chapter 9).
///
/// Creates essential system configuration files: user accounts, hostname,
/// os-release, fstab, networking, locale, readline, systemd targets, and
/// shell prompt. After this, the system is ready for image generation
/// (Phase 5).
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

    // 1. /etc/passwd -- root with no password, plus nobody
    fs::write(
        etc.join("passwd"),
        "root:x:0:0:root:/root:/bin/bash\nnobody:x:65534:65534:Nobody:/:/sbin/nologin\n",
    )?;
    info!("Created /etc/passwd");

    // 2. /etc/group -- essential groups
    fs::write(
        etc.join("group"),
        "root:x:0:\nwheel:x:10:\ntty:x:5:\nnogroup:x:65534:\n",
    )?;
    info!("Created /etc/group");

    // 3. /etc/shadow -- root with empty password (permits passwordless login)
    fs::write(
        etc.join("shadow"),
        "root::0:0:99999:7:::\nnobody:!:0:0:99999:7:::\n",
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

    // 6. /etc/machine-id -- empty file, systemd generates on first boot
    fs::write(etc.join("machine-id"), "")?;
    info!("Created /etc/machine-id (empty, systemd fills on first boot)");

    // 7. /etc/fstab (LFS 10.2)
    fs::write(
        etc.join("fstab"),
        "# /etc/fstab - conaryOS\n\
         LABEL=CONARY_ROOT  /      ext4  defaults,noatime  0 1\n\
         LABEL=CONARY_ESP   /boot  vfat  defaults,noatime  0 2\n\
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
        use std::os::unix::fs::symlink;

        // default.target -> multi-user.target
        symlink(
            "/usr/lib/systemd/system/multi-user.target",
            systemd_system.join("default.target"),
        )?;

        // Enable systemd-networkd
        symlink(
            "/usr/lib/systemd/system/systemd-networkd.service",
            systemd_system.join("multi-user.target.wants/systemd-networkd.service"),
        )?;

        // Enable serial console for QEMU -nographic
        symlink(
            "/usr/lib/systemd/system/serial-getty@.service",
            systemd_system.join("getty.target.wants/serial-getty@ttyS0.service"),
        )?;
    }
    info!("Created systemd service symlinks (default.target, networkd, serial-getty)");

    // 13. /root/.bashrc -- minimal shell prompt
    let root_home = root.join("root");
    fs::create_dir_all(&root_home)?;
    fs::write(
        root_home.join(".bashrc"),
        "export PS1='[\\u@\\h \\W]\\$ '\n\
         alias ls='ls --color=auto'\n",
    )?;
    info!("Created /root/.bashrc");

    info!("Phase 4 complete: system configuration applied");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(group.contains("wheel:x:10"));

        let hostname = std::fs::read_to_string(root.join("etc/hostname")).unwrap();
        assert_eq!(hostname.trim(), "conaryos");

        let os_release = std::fs::read_to_string(root.join("etc/os-release")).unwrap();
        assert!(os_release.contains("conaryOS"));
        assert!(os_release.contains("conaryos.com"));

        let machine_id = std::fs::read_to_string(root.join("etc/machine-id")).unwrap();
        assert!(machine_id.is_empty());

        let fstab = std::fs::read_to_string(root.join("etc/fstab")).unwrap();
        assert!(fstab.contains("LABEL=CONARY_ROOT"));
        assert!(fstab.contains("LABEL=CONARY_ESP"));
        assert!(fstab.contains("tmpfs"));

        let nsswitch = std::fs::read_to_string(root.join("etc/nsswitch.conf")).unwrap();
        assert!(nsswitch.contains("hosts:"));
        assert!(nsswitch.contains("files dns"));

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
    fn test_configure_system_shadow_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("sysroot");
        std::fs::create_dir_all(&root).unwrap();

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
    fn test_configure_system_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("sysroot");
        std::fs::create_dir_all(&root).unwrap();

        configure_system(&root).unwrap();

        // Verify symlinks exist
        assert!(root.join("etc/systemd/system/default.target").exists());
        assert!(
            root.join("etc/systemd/system/multi-user.target.wants/systemd-networkd.service")
                .exists()
        );
        assert!(
            root.join("etc/systemd/system/getty.target.wants/serial-getty@ttyS0.service")
                .exists()
        );

        // Verify symlink targets
        let target = std::fs::read_link(root.join("etc/systemd/system/default.target")).unwrap();
        assert_eq!(
            target.to_str().unwrap(),
            "/usr/lib/systemd/system/multi-user.target"
        );
    }
}
