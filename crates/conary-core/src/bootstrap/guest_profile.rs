// conary-core/src/bootstrap/guest_profile.rs

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum GuestProfileError {
    #[error("public key not found: {0}")]
    MissingPublicKey(PathBuf),

    #[error("failed to read public key {path}: {reason}")]
    ReadPublicKey { path: PathBuf, reason: String },

    #[error("public key {0} is empty")]
    EmptyPublicKey(PathBuf),

    #[error("sshd.service not found in sysroot at {0}")]
    MissingSshdUnit(PathBuf),

    #[error("ssh-keygen not found in sysroot at {0}")]
    MissingSshKeygen(PathBuf),

    #[error("ssh-keygen failed for {key_type}: {reason}")]
    HostKeygen {
        key_type: &'static str,
        reason: String,
    },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

pub fn apply_guest_profile(
    sysroot: &Path,
    public_key_path: &Path,
) -> Result<(), GuestProfileError> {
    let public_key = load_public_key(public_key_path)?;

    write_test_sshd_config(sysroot)?;
    install_authorized_keys(sysroot, &public_key)?;
    enable_sshd_service(sysroot)?;
    generate_host_keys(sysroot)?;
    write_ssh_tmpfiles_config(sysroot)?;

    Ok(())
}

fn load_public_key(public_key_path: &Path) -> Result<String, GuestProfileError> {
    if !public_key_path.exists() {
        return Err(GuestProfileError::MissingPublicKey(
            public_key_path.to_path_buf(),
        ));
    }

    let public_key =
        fs::read_to_string(public_key_path).map_err(|e| GuestProfileError::ReadPublicKey {
            path: public_key_path.to_path_buf(),
            reason: e.to_string(),
        })?;

    if public_key.trim().is_empty() {
        return Err(GuestProfileError::EmptyPublicKey(
            public_key_path.to_path_buf(),
        ));
    }

    Ok(public_key)
}

fn write_test_sshd_config(sysroot: &Path) -> Result<(), GuestProfileError> {
    let ssh_dir = sysroot.join("etc/ssh");
    fs::create_dir_all(&ssh_dir)?;

    fs::write(
        ssh_dir.join("sshd_config"),
        "\
# conaryOS test-image sshd_config
# This profile is intentionally limited to the bootstrap validation path.

PermitRootLogin prohibit-password
PubkeyAuthentication yes
PasswordAuthentication no
PermitEmptyPasswords no
UsePAM no

HostKey /etc/ssh/ssh_host_rsa_key
HostKey /etc/ssh/ssh_host_ecdsa_key
HostKey /etc/ssh/ssh_host_ed25519_key

SyslogFacility AUTH
LogLevel INFO
Subsystem sftp /usr/libexec/sftp-server
",
    )?;

    Ok(())
}

fn install_authorized_keys(sysroot: &Path, public_key: &str) -> Result<(), GuestProfileError> {
    let ssh_dir = sysroot.join("root/.ssh");
    fs::create_dir_all(&ssh_dir)?;
    fs::write(ssh_dir.join("authorized_keys"), public_key)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(&ssh_dir, fs::Permissions::from_mode(0o700))?;
        fs::set_permissions(
            ssh_dir.join("authorized_keys"),
            fs::Permissions::from_mode(0o600),
        )?;
    }

    Ok(())
}

fn enable_sshd_service(sysroot: &Path) -> Result<(), GuestProfileError> {
    let service_target = Path::new("/usr/lib/systemd/system/sshd.service");
    let service_on_disk = sysroot.join("usr/lib/systemd/system/sshd.service");
    if !service_on_disk.exists() {
        return Err(GuestProfileError::MissingSshdUnit(service_on_disk));
    }

    let wants_dir = sysroot.join("etc/systemd/system/multi-user.target.wants");
    fs::create_dir_all(&wants_dir)?;
    let service_link = wants_dir.join("sshd.service");
    if service_link.exists() || service_link.symlink_metadata().is_ok() {
        fs::remove_file(&service_link)?;
    }

    #[cfg(unix)]
    std::os::unix::fs::symlink(service_target, &service_link)?;
    #[cfg(not(unix))]
    fs::write(&service_link, service_target.display().to_string())?;

    Ok(())
}

fn generate_host_keys(sysroot: &Path) -> Result<(), GuestProfileError> {
    let ssh_keygen = sysroot.join("usr/bin/ssh-keygen");
    if !ssh_keygen.exists() {
        return Err(GuestProfileError::MissingSshKeygen(ssh_keygen));
    }

    let ssh_dir = sysroot.join("etc/ssh");
    fs::create_dir_all(&ssh_dir)?;

    for (key_type, key_file) in [
        ("rsa", "ssh_host_rsa_key"),
        ("ecdsa", "ssh_host_ecdsa_key"),
        ("ed25519", "ssh_host_ed25519_key"),
    ] {
        let key_path = ssh_dir.join(key_file);
        if key_path.exists() {
            continue;
        }

        let output = Command::new(&ssh_keygen)
            .args(["-t", key_type, "-f"])
            .arg(&key_path)
            .args(["-N", "", "-q"])
            .output()
            .map_err(GuestProfileError::Io)?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(GuestProfileError::HostKeygen {
                key_type,
                reason: stderr.trim().to_string(),
            });
        }
    }

    Ok(())
}

fn write_ssh_tmpfiles_config(sysroot: &Path) -> Result<(), GuestProfileError> {
    let tmpfiles_dir = sysroot.join("usr/lib/tmpfiles.d");
    fs::create_dir_all(&tmpfiles_dir)?;

    fs::write(
        tmpfiles_dir.join("conary-guest-ssh.conf"),
        "\
# Normalize SSH-critical ownership inside the guest. The bootstrap sysroot is
# assembled unprivileged on the host, so systemd-tmpfiles needs to fix these
# paths back to root ownership before sshd can use them reliably.
d /root 0755 root root -
d /root/.ssh 0700 root root -
z /root/.ssh/authorized_keys 0600 root root -
d /etc/ssh 0755 root root -
z /etc/ssh/sshd_config 0644 root root -
z /etc/ssh/ssh_host_rsa_key 0600 root root -
z /etc/ssh/ssh_host_rsa_key.pub 0644 root root -
z /etc/ssh/ssh_host_ecdsa_key 0600 root root -
z /etc/ssh/ssh_host_ecdsa_key.pub 0644 root root -
z /etc/ssh/ssh_host_ed25519_key 0600 root root -
z /etc/ssh/ssh_host_ed25519_key.pub 0644 root root -
d /var/lib/sshd 0700 root root -
",
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn write_fake_ssh_keygen(sysroot: &Path) {
        let bin_dir = sysroot.join("usr/bin");
        fs::create_dir_all(&bin_dir).unwrap();

        let script_path = bin_dir.join("ssh-keygen");
        fs::write(
            &script_path,
            r#"#!/usr/bin/env bash
set -euo pipefail

key_path=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    -f)
      key_path="$2"
      shift 2
      ;;
    *)
      shift
      ;;
  esac
done

mkdir -p "$(dirname "$key_path")"
printf 'fake-private-key\n' > "$key_path"
printf 'ssh-ed25519 AAAATESTKEY generated-by-test\n' > "$key_path.pub"
"#,
        )
        .unwrap();

        let mut perms = fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms).unwrap();
    }

    fn write_fake_sshd_service(sysroot: &Path) {
        let unit_dir = sysroot.join("usr/lib/systemd/system");
        fs::create_dir_all(&unit_dir).unwrap();
        fs::write(
            unit_dir.join("sshd.service"),
            "[Unit]\nDescription=OpenSSH Daemon\n",
        )
        .unwrap();
    }

    fn host_public_key_file() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("selfhost_ed25519.pub"),
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAITestKey host@test\n",
        )
        .unwrap();
        dir
    }

    #[test]
    fn guest_profile_writes_authorized_keys_from_host_public_key() {
        let sysroot = tempfile::tempdir().unwrap();
        write_fake_ssh_keygen(sysroot.path());
        write_fake_sshd_service(sysroot.path());
        let host_key_dir = host_public_key_file();
        let public_key = host_key_dir.path().join("selfhost_ed25519.pub");

        apply_guest_profile(sysroot.path(), &public_key).unwrap();

        let authorized_keys = sysroot.path().join("root/.ssh/authorized_keys");
        let installed = fs::read_to_string(&authorized_keys).unwrap();
        let expected = fs::read_to_string(&public_key).unwrap();
        assert_eq!(installed, expected);
    }

    #[test]
    fn guest_profile_does_not_write_reusable_operator_private_key() {
        let sysroot = tempfile::tempdir().unwrap();
        write_fake_ssh_keygen(sysroot.path());
        write_fake_sshd_service(sysroot.path());
        let host_key_dir = host_public_key_file();
        let public_key = host_key_dir.path().join("selfhost_ed25519.pub");

        apply_guest_profile(sysroot.path(), &public_key).unwrap();

        assert!(
            !sysroot.path().join("root/.ssh/id_ed25519").exists(),
            "guest profile must not bake the operator/test private key into the image"
        );
    }

    #[test]
    fn guest_profile_owns_test_posture_sshd_config() {
        let sysroot = tempfile::tempdir().unwrap();
        write_fake_ssh_keygen(sysroot.path());
        write_fake_sshd_service(sysroot.path());
        let host_key_dir = host_public_key_file();
        let public_key = host_key_dir.path().join("selfhost_ed25519.pub");

        apply_guest_profile(sysroot.path(), &public_key).unwrap();

        let sshd_config = fs::read_to_string(sysroot.path().join("etc/ssh/sshd_config")).unwrap();
        assert!(sshd_config.contains("PermitRootLogin prohibit-password"));
        assert!(sshd_config.contains("PubkeyAuthentication yes"));
        assert!(sshd_config.contains("PasswordAuthentication no"));
        assert!(sshd_config.contains("PermitEmptyPasswords no"));
        assert!(sshd_config.contains("UsePAM no"));
    }

    #[test]
    fn guest_profile_enables_sshd_service() {
        let sysroot = tempfile::tempdir().unwrap();
        write_fake_ssh_keygen(sysroot.path());
        write_fake_sshd_service(sysroot.path());
        let host_key_dir = host_public_key_file();
        let public_key = host_key_dir.path().join("selfhost_ed25519.pub");

        apply_guest_profile(sysroot.path(), &public_key).unwrap();

        let wants_link = sysroot
            .path()
            .join("etc/systemd/system/multi-user.target.wants/sshd.service");
        assert!(wants_link.symlink_metadata().is_ok());
        let link_target = fs::read_link(wants_link).unwrap();
        assert_eq!(
            link_target,
            PathBuf::from("/usr/lib/systemd/system/sshd.service")
        );
    }

    #[test]
    fn guest_profile_writes_tmpfiles_rules_for_ssh_ownership() {
        let sysroot = tempfile::tempdir().unwrap();
        write_fake_ssh_keygen(sysroot.path());
        write_fake_sshd_service(sysroot.path());
        let host_key_dir = host_public_key_file();
        let public_key = host_key_dir.path().join("selfhost_ed25519.pub");

        apply_guest_profile(sysroot.path(), &public_key).unwrap();

        let tmpfiles = fs::read_to_string(
            sysroot
                .path()
                .join("usr/lib/tmpfiles.d/conary-guest-ssh.conf"),
        )
        .unwrap();
        assert!(tmpfiles.contains("d /root 0755 root root -"));
        assert!(tmpfiles.contains("d /root/.ssh 0700 root root -"));
        assert!(tmpfiles.contains("z /root/.ssh/authorized_keys 0600 root root -"));
        assert!(tmpfiles.contains("z /etc/ssh/ssh_host_ed25519_key 0600 root root -"));
        assert!(tmpfiles.contains("d /var/lib/sshd 0700 root root -"));
    }
}
