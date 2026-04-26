// conary-test/src/engine/qemu.rs

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use tokio::process::{Child, Command};
use tokio::time::{Instant, sleep};

use crate::config::manifest::{QemuBoot, QemuGuestCopy};
use crate::container::backend::ExecResult;

const DEFAULT_ARTIFACT_BASE_URL: &str = "https://remi.conary.io/test-artifacts";
const GUEST_CONARY_STAGING_PATH: &str = "/tmp/conary-host";
const GUEST_CONARY_INSTALL_PATH: &str = "/usr/bin/conary";

/// Well-known filename for the conaryOS test SSH private key.
const TEST_SSH_KEY_NAME: &str = "conaryos-test-key";

pub async fn run_qemu_boot(config: &QemuBoot) -> Result<ExecResult> {
    let required_tools = required_qemu_tools(config);
    let missing_tools = missing_tools(&required_tools).await?;
    if !missing_tools.is_empty() {
        return Ok(skipped_result(format!(
            "qemu boot skipped: missing required tool(s): {}",
            missing_tools.join(", ")
        )));
    }

    let image_path = match resolve_qemu_image_path(config) {
        Ok(path) => path,
        Err(err) => {
            return Ok(ExecResult {
                exit_code: 1,
                stdout: String::new(),
                stderr: err.to_string(),
            });
        }
    };
    if config.local_image_path.is_none() && !image_path.exists() {
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
    qemu.args(qemu_image_args(&image_path));
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
    let command_timeout = Duration::from_secs(config.timeout_seconds);

    if config.stage_conary {
        match stage_conary_binary(
            config.ssh_port,
            key_path.as_deref(),
            command_timeout,
            &mut stdout,
            &mut stderr,
        )
        .await
        {
            Ok(stage_exit_code) => exit_code = stage_exit_code,
            Err(err) => {
                exit_code = 1;
                if !stderr.is_empty() {
                    stderr.push('\n');
                }
                stderr.push_str(&format!("failed to stage host conary binary: {err:#}"));
            }
        }
    }

    if exit_code == 0 {
        for command in &config.commands {
            let result = run_ssh_command(
                config.ssh_port,
                command,
                key_path.as_deref(),
                command_timeout,
            )
            .await?;
            append_command_output(&mut stdout, &mut stderr, command, &result);
            if result.exit_code != 0 {
                exit_code = result.exit_code;
                break;
            }
        }
    }

    if exit_code == 0 {
        for copy in &config.copy_from_guest {
            let result =
                copy_file_from_guest(config.ssh_port, copy, key_path.as_deref(), command_timeout)
                    .await?;
            append_guest_copy_output(&mut stdout, &mut stderr, copy, &result);
            if result.exit_code != 0 {
                exit_code = result.exit_code;
                break;
            }
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

fn required_qemu_tools(config: &QemuBoot) -> Vec<&'static str> {
    let mut tools = vec!["qemu-system-x86_64", "ssh"];
    if config.local_image_path.is_none() {
        tools.push("curl");
    }
    if !config.copy_from_guest.is_empty() {
        tools.push("scp");
    }
    if config.stage_conary && !tools.contains(&"scp") {
        tools.push("scp");
    }
    tools
}

fn qemu_image_args(image_path: &Path) -> [String; 3] {
    [
        "-snapshot".to_string(),
        "-drive".to_string(),
        format!("file={},format=qcow2", image_path.display()),
    ]
}

fn resolve_qemu_image_path(config: &QemuBoot) -> Result<PathBuf> {
    if let Some(local_path) = &config.local_image_path {
        let path = PathBuf::from(local_path);
        if !path.is_file() {
            anyhow::bail!("local QEMU image path does not exist: {}", path.display());
        }
        return Ok(path);
    }

    cache_path_for_image(&config.image)
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

fn append_guest_copy_output(
    stdout: &mut String,
    stderr: &mut String,
    copy: &QemuGuestCopy,
    result: &ExecResult,
) {
    let command = format!("scp root@127.0.0.1:{} {}", copy.source, copy.dest);
    append_command_output(stdout, stderr, &command, result);
}

fn append_host_copy_output(
    stdout: &mut String,
    stderr: &mut String,
    source: &Path,
    dest: &str,
    result: &ExecResult,
) {
    let command = format!("scp {} root@127.0.0.1:{dest}", source.display());
    append_command_output(stdout, stderr, &command, result);
}

async fn missing_tools(tools: &[&str]) -> Result<Vec<String>> {
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

        let probe = run_ssh_command(
            ssh_port,
            "true",
            key_path.as_deref(),
            Duration::from_secs(5),
        )
        .await?;
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
    timeout: Duration,
) -> Result<ExecResult> {
    let remote = format!("sh -lc {}", shell_quote(command));
    let mut ssh = Command::new("ssh");
    if let Some(key) = key_path {
        ssh.args(["-i", &key.display().to_string()]);
    } else {
        ssh.args(["-o", "BatchMode=yes"]);
    }
    ssh.args([
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
    ]);

    run_command_with_timeout(ssh, timeout, &format!("SSH command `{command}`")).await
}

async fn copy_file_from_guest(
    ssh_port: u16,
    copy: &QemuGuestCopy,
    key_path: Option<&Path>,
    timeout: Duration,
) -> Result<ExecResult> {
    let dest = Path::new(&copy.dest);
    prepare_guest_copy_destination(dest)?;

    let remote = format!("root@127.0.0.1:{}", copy.source);
    let mut scp = Command::new("scp");
    if let Some(key) = key_path {
        scp.args(["-i", &key.display().to_string()]);
    } else {
        scp.args(["-o", "BatchMode=yes"]);
    }
    scp.args([
        "-o",
        "StrictHostKeyChecking=no",
        "-o",
        "UserKnownHostsFile=/dev/null",
        "-o",
        "LogLevel=ERROR",
        "-o",
        "ConnectTimeout=10",
        "-P",
        &ssh_port.to_string(),
        &remote,
    ])
    .arg(dest);

    run_command_with_timeout(
        scp,
        timeout,
        &format!("copy {} from QEMU guest", copy.source),
    )
    .await
}

async fn copy_file_to_guest(
    ssh_port: u16,
    source: &Path,
    dest: &str,
    key_path: Option<&Path>,
    timeout: Duration,
) -> Result<ExecResult> {
    let remote = format!("root@127.0.0.1:{dest}");
    let mut scp = Command::new("scp");
    if let Some(key) = key_path {
        scp.args(["-i", &key.display().to_string()]);
    } else {
        scp.args(["-o", "BatchMode=yes"]);
    }
    scp.args([
        "-o",
        "StrictHostKeyChecking=no",
        "-o",
        "UserKnownHostsFile=/dev/null",
        "-o",
        "LogLevel=ERROR",
        "-o",
        "ConnectTimeout=10",
        "-P",
        &ssh_port.to_string(),
    ])
    .arg(source)
    .arg(&remote);

    run_command_with_timeout(
        scp,
        timeout,
        &format!("copy host conary binary {} to QEMU guest", source.display()),
    )
    .await
}

async fn run_command_with_timeout(
    mut command: Command,
    timeout: Duration,
    description: &str,
) -> Result<ExecResult> {
    command.kill_on_drop(true);
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let child = command
        .spawn()
        .with_context(|| format!("failed to start {description}"))?;

    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(output) => {
            let output = output.with_context(|| format!("failed to wait for {description}"))?;
            Ok(ExecResult {
                exit_code: output.status.code().unwrap_or(1),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            })
        }
        Err(_) => Ok(ExecResult {
            exit_code: 124,
            stdout: String::new(),
            stderr: format!("{description} timed out after {}s", timeout.as_secs()),
        }),
    }
}

struct PreparedHostConary {
    path: PathBuf,
    temp_dir: PathBuf,
}

impl Drop for PreparedHostConary {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.temp_dir);
    }
}

fn prepare_host_conary_for_guest() -> Result<PreparedHostConary> {
    let source = crate::paths::host_conary_binary()?;
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system time before unix epoch")?
        .as_nanos();
    let temp_dir = std::env::temp_dir().join(format!(
        "conary-test-qemu-conary-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_dir)
        .with_context(|| format!("failed to create {}", temp_dir.display()))?;
    let path = temp_dir.join("conary");
    fs::copy(&source, &path)
        .with_context(|| format!("failed to stage host conary binary {}", source.display()))?;

    // Debug builds are large; strip a temporary copy before sending it over SSH.
    let _ = std::process::Command::new("strip").arg(&path).status();

    Ok(PreparedHostConary { path, temp_dir })
}

async fn stage_conary_binary(
    ssh_port: u16,
    key_path: Option<&Path>,
    timeout: Duration,
    stdout: &mut String,
    stderr: &mut String,
) -> Result<i32> {
    let staged = prepare_host_conary_for_guest()?;
    let copy = copy_file_to_guest(
        ssh_port,
        &staged.path,
        GUEST_CONARY_STAGING_PATH,
        key_path,
        timeout,
    )
    .await?;
    append_host_copy_output(
        stdout,
        stderr,
        &staged.path,
        GUEST_CONARY_STAGING_PATH,
        &copy,
    );
    if copy.exit_code != 0 {
        return Ok(copy.exit_code);
    }

    let install_command = format!(
        "install -m 755 {GUEST_CONARY_STAGING_PATH} {GUEST_CONARY_INSTALL_PATH} && rm -f {GUEST_CONARY_STAGING_PATH} && {GUEST_CONARY_INSTALL_PATH} --version"
    );
    let install = run_ssh_command(ssh_port, &install_command, key_path, timeout).await?;
    append_command_output(stdout, stderr, &install_command, &install);
    Ok(install.exit_code)
}

fn prepare_guest_copy_destination(dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create QEMU guest-copy destination directory {}",
                parent.display()
            )
        })?;
    }
    Ok(())
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

    #[test]
    fn test_local_image_path_requires_existing_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let missing = dir.path().join("missing.qcow2");
        let config = QemuBoot {
            image: "local-image".to_string(),
            local_image_path: Some(missing.display().to_string()),
            stage_conary: false,
            copy_from_guest: Vec::new(),
            memory_mb: 1024,
            timeout_seconds: 120,
            ssh_port: 2222,
            commands: vec!["true".to_string()],
            expect_output: Vec::new(),
        };

        let err = resolve_qemu_image_path(&config).unwrap_err();

        assert!(
            err.to_string()
                .contains("local QEMU image path does not exist")
        );
        assert!(err.to_string().contains("missing.qcow2"));
    }

    #[test]
    fn test_prepare_guest_copy_destination_creates_parent_directory() {
        let dir = tempfile::TempDir::new().unwrap();
        let dest = dir.path().join("nested/generated/image.qcow2");
        assert!(!dest.parent().unwrap().exists());

        prepare_guest_copy_destination(&dest).unwrap();

        assert!(dest.parent().unwrap().is_dir());
    }

    #[test]
    fn test_prepare_guest_copy_destination_allows_current_dir_destination() {
        prepare_guest_copy_destination(Path::new("image.qcow2")).unwrap();
    }

    #[test]
    fn test_qemu_image_args_use_snapshot_overlay() {
        let args = qemu_image_args(Path::new("/tmp/minimal-boot-v2.qcow2"));

        assert_eq!(args[0], "-snapshot");
        assert_eq!(args[1], "-drive");
        assert_eq!(args[2], "file=/tmp/minimal-boot-v2.qcow2,format=qcow2");
    }

    #[test]
    fn test_required_qemu_tools_include_scp_when_staging_conary() {
        let config = QemuBoot {
            image: "minimal-boot-v2".to_string(),
            local_image_path: None,
            stage_conary: true,
            copy_from_guest: Vec::new(),
            memory_mb: 1024,
            timeout_seconds: 120,
            ssh_port: 2222,
            commands: vec!["conary --version".to_string()],
            expect_output: Vec::new(),
        };

        assert!(required_qemu_tools(&config).contains(&"scp"));
    }

    #[tokio::test]
    async fn test_command_timeout_returns_failure_instead_of_hanging() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 5"]);

        let result =
            run_command_with_timeout(command, Duration::from_millis(10), "slow command").await;

        let result = result.expect("timeout result");
        assert_eq!(result.exit_code, 124);
        assert!(result.stderr.contains("slow command timed out"));
    }
}
