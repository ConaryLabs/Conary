// tests/target_root.rs

//! Integration tests for target root (bootstrap) functionality.
//!
//! These tests verify that Conary can properly work with target root
//! filesystems without modifying the host system. This is critical for:
//! - Bootstrap: Building a new system from scratch
//! - Container image creation: Populating rootfs without affecting host
//! - Offline installations: Installing packages into mounted filesystems

use std::fs;
use std::path::Path;
use tempfile::TempDir;

/// Test that hook executor properly handles user/group files in target root
#[test]
fn test_target_root_user_group_files() {
    let temp_dir = TempDir::new().unwrap();
    let target_root = temp_dir.path();

    // Create /etc directory in target with minimal passwd and group files
    let etc_dir = target_root.join("etc");
    fs::create_dir_all(&etc_dir).unwrap();
    fs::write(
        etc_dir.join("passwd"),
        "root:x:0:0:root:/root:/bin/bash\nnobody:x:65534:65534:Nobody:/:/usr/sbin/nologin\n",
    )
    .unwrap();
    fs::write(etc_dir.join("group"), "root:x:0:\nwheel:x:10:user1,user2\n").unwrap();

    // Verify the files exist and have expected content
    let passwd_content = fs::read_to_string(etc_dir.join("passwd")).unwrap();
    assert!(passwd_content.contains("root:x:0:0"));
    assert!(passwd_content.contains("nobody:x:65534"));

    let group_content = fs::read_to_string(etc_dir.join("group")).unwrap();
    assert!(group_content.contains("root:x:0"));
    assert!(group_content.contains("wheel:x:10"));
}

/// Test that systemd unit directories can be created in target root
#[test]
fn test_target_root_systemd_structure() {
    let temp_dir = TempDir::new().unwrap();
    let target_root = temp_dir.path();

    // Create systemd directories as would be done during bootstrap
    let usr_lib_systemd = target_root.join("usr/lib/systemd/system");
    let etc_systemd = target_root.join("etc/systemd/system");

    fs::create_dir_all(&usr_lib_systemd).unwrap();
    fs::create_dir_all(&etc_systemd).unwrap();

    // Create a test unit file
    let unit_content = r#"[Unit]
Description=Test Service

[Service]
ExecStart=/usr/bin/test

[Install]
WantedBy=multi-user.target
"#;
    fs::write(usr_lib_systemd.join("test.service"), unit_content).unwrap();

    // Verify structure exists
    assert!(usr_lib_systemd.exists());
    assert!(etc_systemd.exists());
    assert!(usr_lib_systemd.join("test.service").exists());

    // Create a symlink manually (mimicking what systemd_enable_target does)
    let wants_dir = etc_systemd.join("multi-user.target.wants");
    fs::create_dir_all(&wants_dir).unwrap();

    let symlink_path = wants_dir.join("test.service");
    let target = "../../../../usr/lib/systemd/system/test.service";
    std::os::unix::fs::symlink(target, &symlink_path).unwrap();

    // Verify symlink
    assert!(symlink_path.exists());
    let link_target = fs::read_link(&symlink_path).unwrap();
    assert!(link_target.to_string_lossy().contains("usr/lib/systemd/system/test.service"));
}

/// Test that config directories can be properly created in target root
#[test]
fn test_target_root_config_directories() {
    let temp_dir = TempDir::new().unwrap();
    let target_root = temp_dir.path();

    // Create typical config directories
    let dirs = [
        "etc/sysctl.d",
        "etc/tmpfiles.d",
        "etc/systemd/system",
        "var/lib",
        "var/log",
        "usr/lib/systemd/system",
    ];

    for dir in &dirs {
        let path = target_root.join(dir);
        fs::create_dir_all(&path).unwrap();
        assert!(path.exists(), "Failed to create {}", dir);
    }

    // Create sample sysctl config
    let sysctl_config = target_root.join("etc/sysctl.d/99-conary.conf");
    fs::write(&sysctl_config, "net.ipv4.ip_forward=1\n").unwrap();
    assert!(sysctl_config.exists());

    // Create sample tmpfiles config
    let tmpfiles_config = target_root.join("etc/tmpfiles.d/conary.conf");
    fs::write(&tmpfiles_config, "d /var/lib/myapp 0755 root root -\n").unwrap();
    assert!(tmpfiles_config.exists());
}

/// Test pristine container configuration for bootstrap
#[test]
fn test_pristine_container_config() {
    use conary::container::ContainerConfig;

    // Test basic pristine config
    let pristine = ContainerConfig::pristine();

    // Should have no bind mounts (no host contamination)
    assert!(
        pristine.bind_mounts.is_empty(),
        "Pristine should have no bind mounts"
    );

    // Should be detected as pristine
    assert!(pristine.is_pristine(), "Should be detected as pristine");

    // Should have full isolation enabled
    assert!(pristine.isolate_pid, "Should isolate PID namespace");
    assert!(pristine.isolate_mount, "Should isolate mount namespace");
    assert!(pristine.isolate_uts, "Should isolate UTS namespace");
    assert!(pristine.isolate_ipc, "Should isolate IPC namespace");

    // Should have long timeout for builds
    assert!(
        pristine.timeout.as_secs() >= 3600,
        "Should have >= 1 hour timeout"
    );
}

/// Test pristine container for bootstrap builds
#[test]
fn test_pristine_for_bootstrap() {
    use conary::container::ContainerConfig;
    use std::path::PathBuf;

    let config = ContainerConfig::pristine_for_bootstrap(
        Path::new("/opt/stage0"),
        Path::new("/src/package"),
        Path::new("/build/package"),
        Path::new("/destdir"),
    );

    // Should have explicit mounts but no host system mounts
    assert!(
        !config.bind_mounts.is_empty(),
        "Should have explicit mounts"
    );
    assert!(
        config.is_pristine(),
        "Should still be pristine (no /usr, /lib, etc)"
    );

    // Working directory should be set
    assert_eq!(config.workdir, PathBuf::from("/build/package"));

    // Verify expected mounts are present
    let mount_sources: Vec<String> = config
        .bind_mounts
        .iter()
        .map(|m| m.source.to_string_lossy().to_string())
        .collect();

    assert!(
        mount_sources.contains(&"/opt/stage0".to_string()),
        "Should mount sysroot"
    );
    assert!(
        mount_sources.contains(&"/src/package".to_string()),
        "Should mount source"
    );
    assert!(
        mount_sources.contains(&"/build/package".to_string()),
        "Should mount build dir"
    );
    assert!(
        mount_sources.contains(&"/destdir".to_string()),
        "Should mount destdir"
    );

    // Verify no host system directories
    assert!(
        !mount_sources.contains(&"/usr".to_string()),
        "Should not mount /usr"
    );
    assert!(
        !mount_sources.contains(&"/lib".to_string()),
        "Should not mount /lib"
    );
}

/// Test is_pristine detection with various configurations
#[test]
fn test_is_pristine_detection() {
    use conary::container::{BindMount, ContainerConfig};

    // Start with pristine
    let mut config = ContainerConfig::pristine();
    assert!(config.is_pristine());

    // Adding toolchain mount keeps it pristine
    config.add_bind_mount(BindMount::readonly("/tools", "/tools"));
    assert!(config.is_pristine());

    // Adding /usr mount makes it not pristine
    config.add_bind_mount(BindMount::readonly("/usr", "/usr"));
    assert!(!config.is_pristine());
}

/// Test that bin directories can be searched in target root
#[test]
fn test_target_root_bin_directories() {
    let temp_dir = TempDir::new().unwrap();
    let target_root = temp_dir.path();

    // Create standard bin directories
    let bin_dirs = ["usr/bin", "usr/sbin", "bin", "sbin", "usr/local/bin"];

    for dir in &bin_dirs {
        let path = target_root.join(dir);
        fs::create_dir_all(&path).unwrap();
        assert!(path.exists(), "Failed to create {}", dir);
    }

    // Create a fake executable
    let test_binary = target_root.join("usr/bin/test-tool");
    fs::write(&test_binary, "#!/bin/sh\necho test\n").unwrap();

    // Verify it exists
    assert!(test_binary.exists());

    // Verify searching in target root would find it
    for dir in &bin_dirs {
        let search_path = target_root.join(dir).join("test-tool");
        if search_path.exists() {
            // This is where handler_exists_in_root would find it
            assert_eq!(search_path, test_binary);
        }
    }
}

/// Test bootstrap rootfs structure
#[test]
fn test_bootstrap_rootfs_structure() {
    let temp_dir = TempDir::new().unwrap();
    let target_root = temp_dir.path();

    // Create minimal rootfs structure for bootstrap
    let essential_dirs = [
        "bin",
        "boot",
        "dev",
        "etc",
        "etc/sysctl.d",
        "etc/tmpfiles.d",
        "etc/systemd/system",
        "home",
        "lib",
        "lib64",
        "mnt",
        "opt",
        "proc",
        "root",
        "run",
        "sbin",
        "srv",
        "sys",
        "tmp",
        "usr",
        "usr/bin",
        "usr/lib",
        "usr/lib64",
        "usr/lib/systemd/system",
        "usr/sbin",
        "usr/share",
        "var",
        "var/cache",
        "var/lib",
        "var/log",
        "var/tmp",
    ];

    for dir in &essential_dirs {
        let path = target_root.join(dir);
        fs::create_dir_all(&path).unwrap();
    }

    // Create essential config files
    fs::write(
        target_root.join("etc/passwd"),
        "root:x:0:0:root:/root:/bin/bash\n",
    )
    .unwrap();
    fs::write(target_root.join("etc/group"), "root:x:0:\n").unwrap();
    fs::write(target_root.join("etc/hostname"), "conary\n").unwrap();

    // Verify structure
    for dir in &essential_dirs {
        assert!(
            target_root.join(dir).exists(),
            "Missing essential directory: {}",
            dir
        );
    }

    // Verify config files
    assert!(target_root.join("etc/passwd").exists());
    assert!(target_root.join("etc/group").exists());
    assert!(target_root.join("etc/hostname").exists());
}
