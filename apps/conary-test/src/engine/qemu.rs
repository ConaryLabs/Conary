// conary-test/src/engine/qemu.rs

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::process::{Child, Command};
use tokio::time::{Instant, sleep};

use crate::config::manifest::QemuBoot;
use crate::container::backend::ExecResult;

const DEFAULT_ARTIFACT_BASE_URL: &str = "https://remi.conary.io/test-artifacts";

/// Well-known filename for the conaryOS test SSH private key.
const TEST_SSH_KEY_NAME: &str = "conaryos-test-key";

pub async fn run_qemu_boot(config: &QemuBoot) -> Result<ExecResult> {
    let missing_tools = missing_tools(["curl", "qemu-system-x86_64", "ssh"]).await?;
    if !missing_tools.is_empty() {
        return Ok(skipped_result(format!(
            "qemu boot skipped: missing required tool(s): {}",
            missing_tools.join(", ")
        )));
    }

    let image_path = cache_path_for_image(&config.image)?;
    if !image_path.exists() {
        let url = image_download_url(&config.image);
        if let Err(err) = download_image(&url, &image_path).await {
            return Ok(skipped_result(format!(
                "qemu boot skipped: failed to download {url}: {err:#}"
            )));
        }
    }

    let accel = if Path::new("/dev/kvm").exists() {
        "kvm"
    } else {
        "tcg"
    };

    // Locate UEFI firmware — bootstrap images use GPT + EFI boot
    let ovmf_paths = [
        "/usr/share/edk2/ovmf/OVMF_CODE.fd",
        "/usr/share/OVMF/OVMF_CODE.fd",
        "/usr/share/edk2-ovmf/x64/OVMF_CODE.fd",
        "/usr/share/qemu/OVMF_CODE.fd",
    ];
    let ovmf_code = ovmf_paths.iter().find(|p| Path::new(p).exists());

    let mut qemu = Command::new("qemu-system-x86_64");
    qemu.args([
        "-m",
        &config.memory_mb.to_string(),
        "-drive",
        &format!("file={},format=qcow2", image_path.display()),
        "-netdev",
        &format!("user,id=net0,hostfwd=tcp::{}-:22", config.ssh_port),
        "-device",
        "e1000,netdev=net0",
        "-nographic",
        "-serial",
        "mon:stdio",
        "-accel",
        accel,
    ]);
    // Add UEFI firmware if available (required for GPT/EFI boot images)
    if let Some(fw) = ovmf_code {
        qemu.args(["-bios", fw]);
    }
    qemu.stdin(Stdio::null());
    qemu.stdout(Stdio::piped());
    qemu.stderr(Stdio::piped());

    let mut child = qemu
        .spawn()
        .context("failed to launch qemu-system-x86_64")?;

    if let Err(message) = wait_for_ssh(&mut child, config.ssh_port, config.timeout_seconds).await? {
        let output = stop_qemu(child).await?;
        return Ok(ExecResult {
            exit_code: 1,
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: format!(
                "{message}\n{}",
                String::from_utf8_lossy(&output.stderr).trim()
            )
            .trim()
            .to_string(),
        });
    }

    let key_path = test_ssh_key_path().await.ok();
    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut exit_code = 0;

    for command in &config.commands {
        let result = run_ssh_command(config.ssh_port, command, key_path.as_deref()).await?;
        append_command_output(&mut stdout, &mut stderr, command, &result);
        if result.exit_code != 0 {
            exit_code = result.exit_code;
            break;
        }
    }

    let qemu_output = stop_qemu(child).await?;
    let qemu_stdout = String::from_utf8_lossy(&qemu_output.stdout)
        .trim()
        .to_string();
    let qemu_stderr = String::from_utf8_lossy(&qemu_output.stderr)
        .trim()
        .to_string();
    if !qemu_stdout.is_empty() {
        if !stdout.is_empty() {
            stdout.push('\n');
        }
        stdout.push_str(&qemu_stdout);
    }
    if !qemu_stderr.is_empty() {
        if !stderr.is_empty() {
            stderr.push('\n');
        }
        stderr.push_str(&qemu_stderr);
    }

    if exit_code == 0 {
        for expected in &config.expect_output {
            if !stdout.contains(expected) && !stderr.contains(expected) {
                exit_code = 1;
                if !stderr.is_empty() {
                    stderr.push('\n');
                }
                stderr.push_str(&format!("expected QEMU output to contain \"{expected}\""));
                break;
            }
        }
    }

    Ok(ExecResult {
        exit_code,
        stdout,
        stderr,
    })
}

fn skipped_result(message: String) -> ExecResult {
    ExecResult {
        exit_code: 0,
        stdout: message,
        stderr: String::new(),
    }
}

fn append_command_output(
    stdout: &mut String,
    stderr: &mut String,
    command: &str,
    result: &ExecResult,
) {
    if !stdout.is_empty() {
        stdout.push('\n');
    }
    stdout.push_str("$ ");
    stdout.push_str(command);
    if !result.stdout.is_empty() {
        stdout.push('\n');
        stdout.push_str(result.stdout.trim_end());
    }

    if !result.stderr.is_empty() {
        if !stderr.is_empty() {
            stderr.push('\n');
        }
        stderr.push_str("$ ");
        stderr.push_str(command);
        stderr.push('\n');
        stderr.push_str(result.stderr.trim_end());
    }
}

async fn missing_tools<const N: usize>(tools: [&str; N]) -> Result<Vec<String>> {
    #[cfg(test)]
    if let Some(override_tools) = test_missing_tools_override().lock().unwrap().clone() {
        return Ok(override_tools);
    }

    let mut missing = Vec::new();
    for tool in tools {
        let status = Command::new("sh")
            .args(["-lc", &format!("command -v {tool} >/dev/null 2>&1")])
            .status()
            .await
            .with_context(|| format!("failed to probe for required tool {tool}"))?;
        if !status.success() {
            missing.push(tool.to_string());
        }
    }
    Ok(missing)
}

fn cache_dir() -> PathBuf {
    if let Ok(path) = std::env::var("XDG_CACHE_HOME") {
        return PathBuf::from(path).join("conary-test");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".cache").join("conary-test");
    }
    PathBuf::from("/tmp/conary-test-cache")
}

fn cache_path_for_image(image: &str) -> Result<PathBuf> {
    let dir = cache_dir();
    fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create QEMU cache dir {}", dir.display()))?;
    Ok(dir.join(image_filename(image)))
}

fn image_filename(image: &str) -> String {
    if image.contains("://") {
        let tail = image.rsplit('/').next().unwrap_or("image.qcow2");
        return tail.to_string();
    }
    // Strip path components to prevent directory traversal (e.g. "../../tmp/owned").
    let basename = Path::new(image)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("image.qcow2");
    if basename.ends_with(".qcow2") {
        basename.to_string()
    } else {
        format!("{basename}.qcow2")
    }
}

fn image_download_url(image: &str) -> String {
    if image.contains("://") {
        image.to_string()
    } else {
        format!("{}/{}", DEFAULT_ARTIFACT_BASE_URL, image_filename(image))
    }
}

async fn download_image(url: &str, path: &Path) -> Result<()> {
    let file_name = path
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("image.qcow2");
    let partial = path.with_file_name(format!("{file_name}.partial"));
    let status = Command::new("curl")
        .args(["-fsSL", url, "-o"])
        .arg(&partial)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .with_context(|| format!("failed to invoke curl for {url}"))?;
    if !status.success() {
        return Err(anyhow::anyhow!("curl exited with status {status}"));
    }
    fs::rename(&partial, path).with_context(|| {
        format!(
            "failed to move downloaded image from {} to {}",
            partial.display(),
            path.display()
        )
    })?;
    Ok(())
}

async fn test_ssh_key_path() -> Result<PathBuf> {
    let dir = cache_dir();
    fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create cache dir {}", dir.display()))?;
    let key_path = dir.join(TEST_SSH_KEY_NAME);
    if key_path.exists() {
        return Ok(key_path);
    }
    let url = format!("{DEFAULT_ARTIFACT_BASE_URL}/{TEST_SSH_KEY_NAME}");
    download_image(&url, &key_path)
        .await
        .with_context(|| format!("failed to download test SSH key from {url}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("failed to set permissions on {}", key_path.display()))?;
    }
    Ok(key_path)
}

async fn wait_for_ssh(
    child: &mut Child,
    ssh_port: u16,
    timeout_seconds: u64,
) -> Result<std::result::Result<(), String>> {
    let key_path = test_ssh_key_path().await.ok();
    let deadline = Instant::now() + Duration::from_secs(timeout_seconds);
    let mut last_error = "ssh not ready".to_string();

    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().context("failed to poll qemu process")? {
            return Ok(Err(format!(
                "qemu exited before SSH became ready: {status}"
            )));
        }

        let probe = run_ssh_command(ssh_port, "true", key_path.as_deref()).await?;
        if probe.exit_code == 0 {
            return Ok(Ok(()));
        }

        last_error = if probe.stderr.trim().is_empty() {
            format!("ssh probe failed with exit code {}", probe.exit_code)
        } else {
            probe.stderr.trim().to_string()
        };
        sleep(Duration::from_secs(1)).await;
    }

    Ok(Err(format!(
        "timed out waiting for SSH on port {ssh_port}: {last_error}"
    )))
}

async fn run_ssh_command(
    ssh_port: u16,
    command: &str,
    key_path: Option<&Path>,
) -> Result<ExecResult> {
    let remote = format!("sh -lc {}", shell_quote(command));
    let mut ssh = Command::new("ssh");
    if let Some(key) = key_path {
        ssh.args(["-i", &key.display().to_string()]);
    } else {
        ssh.args(["-o", "BatchMode=yes"]);
    }
    let output = ssh
        .args([
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "UserKnownHostsFile=/dev/null",
            "-o",
            "LogLevel=ERROR",
            "-o",
            "ConnectTimeout=2",
            "-p",
            &ssh_port.to_string(),
            "root@127.0.0.1",
            &remote,
        ])
        .output()
        .await
        .with_context(|| format!("failed to run SSH command: {command}"))?;

    Ok(ExecResult {
        exit_code: output.status.code().unwrap_or(1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

async fn stop_qemu(mut child: Child) -> Result<std::process::Output> {
    let _ = child.start_kill();
    child
        .wait_with_output()
        .await
        .context("failed to collect QEMU process output")
}

fn shell_quote(input: &str) -> String {
    let escaped = input.replace('\'', r#"'\''"#);
    format!("'{escaped}'")
}

#[cfg(test)]
fn test_missing_tools_override() -> &'static std::sync::Mutex<Option<Vec<String>>> {
    use std::sync::{Mutex, OnceLock};

    static OVERRIDE: OnceLock<Mutex<Option<Vec<String>>>> = OnceLock::new();
    OVERRIDE.get_or_init(|| Mutex::new(None))
}

#[cfg(test)]
pub(crate) fn set_missing_tools_override_for_tests(tools: Option<Vec<String>>) {
    *test_missing_tools_override().lock().unwrap() = tools;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_image_filename_appends_qcow2() {
        assert_eq!(image_filename("minimal-boot-v1"), "minimal-boot-v1.qcow2");
        assert_eq!(
            image_filename("minimal-boot-v1.qcow2"),
            "minimal-boot-v1.qcow2"
        );
    }

    #[test]
    fn test_image_download_url_uses_default_base() {
        assert_eq!(
            image_download_url("minimal-boot-v1"),
            "https://remi.conary.io/test-artifacts/minimal-boot-v1.qcow2"
        );
        assert_eq!(
            image_download_url("https://example.com/custom.qcow2"),
            "https://example.com/custom.qcow2"
        );
    }

    #[test]
    fn test_image_filename_uses_url_basename() {
        assert_eq!(
            image_filename("https://example.com/test-artifacts/minimal-boot-v1.qcow2"),
            "minimal-boot-v1.qcow2"
        );
    }

    #[test]
    fn test_image_filename_strips_path_traversal() {
        // Path traversal attempts should be stripped to just the filename.
        assert_eq!(image_filename("../../tmp/owned"), "owned.qcow2");
        assert_eq!(image_filename("../../../etc/passwd"), "passwd.qcow2");
        assert_eq!(image_filename("subdir/image.qcow2"), "image.qcow2");
    }

    #[test]
    fn test_shell_quote_escapes_single_quotes() {
        assert_eq!(shell_quote("uname -r"), "'uname -r'");
        assert_eq!(shell_quote("printf 'hello'"), r#"'printf '\''hello'\'''"#);
    }
}
