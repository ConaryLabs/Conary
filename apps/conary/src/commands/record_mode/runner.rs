// apps/conary/src/commands/record_mode/runner.rs

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, bail};
use conary_core::container::{BindMount, ContainerConfig, Sandbox};

#[derive(Debug, Clone)]
pub(crate) struct RecordCommandRequest {
    pub(crate) source_root: PathBuf,
    pub(crate) work_root: PathBuf,
    pub(crate) install_root: PathBuf,
    pub(crate) command: Vec<String>,
    pub(crate) unsafe_host: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct RecordSandboxPlan {
    pub(crate) unsafe_host: bool,
    pub(crate) cwd: String,
    pub(crate) network_isolated: bool,
    pub(crate) mounts: Vec<(PathBuf, String, bool)>,
    pub(crate) env: Vec<(String, String)>,
}

impl RecordSandboxPlan {
    #[cfg(test)]
    fn env_value(&self, key: &str) -> Option<&str> {
        self.env
            .iter()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, value)| value.as_str())
    }

    #[cfg(test)]
    fn has_mount(&self, source: &std::path::Path, target: &str, writable: bool) -> bool {
        self.mounts
            .iter()
            .any(|(candidate, mount_target, mount_writable)| {
                candidate == source && mount_target == target && *mount_writable == writable
            })
    }
}

pub(crate) fn sandbox_plan(request: &RecordCommandRequest) -> Result<RecordSandboxPlan> {
    if request.command.is_empty() {
        bail!("record command cannot be empty");
    }
    let source_date_epoch = std::env::var("SOURCE_DATE_EPOCH").unwrap_or_else(|_| "0".to_string());

    if request.unsafe_host {
        let install = request.install_root.to_string_lossy().to_string();
        return Ok(RecordSandboxPlan {
            unsafe_host: true,
            cwd: request.source_root.to_string_lossy().to_string(),
            network_isolated: false,
            mounts: Vec::new(),
            env: vec![
                ("DESTDIR".to_string(), install.clone()),
                ("CONARY_DESTDIR".to_string(), install),
                (
                    "CONARY_WORKDIR".to_string(),
                    request.work_root.to_string_lossy().to_string(),
                ),
                ("SOURCE_DATE_EPOCH".to_string(), source_date_epoch),
            ],
        });
    }

    Ok(RecordSandboxPlan {
        unsafe_host: false,
        cwd: "/conary/source".to_string(),
        network_isolated: true,
        mounts: vec![
            (
                request.source_root.clone(),
                "/conary/source".to_string(),
                true,
            ),
            (request.work_root.clone(), "/conary/work".to_string(), true),
            (
                request.install_root.clone(),
                "/conary/destdir".to_string(),
                true,
            ),
        ],
        env: vec![
            ("DESTDIR".to_string(), "/conary/destdir".to_string()),
            ("CONARY_DESTDIR".to_string(), "/conary/destdir".to_string()),
            ("CONARY_WORKDIR".to_string(), "/conary/work".to_string()),
            ("SOURCE_DATE_EPOCH".to_string(), source_date_epoch),
        ],
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordCommandOutcome {
    pub(crate) exit_code: i32,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

pub(crate) fn run_record_command(request: &RecordCommandRequest) -> Result<RecordCommandOutcome> {
    let plan = sandbox_plan(request)?;
    if plan.unsafe_host {
        let mut command = std::process::Command::new(&request.command[0]);
        command.args(&request.command[1..]);
        command.current_dir(&request.source_root);
        for (key, value) in &plan.env {
            command.env(key, value);
        }
        let output = command.output()?;
        return Ok(RecordCommandOutcome {
            exit_code: output.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    let mut config = ContainerConfig::default().for_untrusted();
    config.timeout = Duration::from_secs(3600);
    config.workdir = PathBuf::from(&plan.cwd);
    config.isolate_network = plan.network_isolated;
    for (source, target, writable) in &plan.mounts {
        let mount = if *writable {
            BindMount::writable(source, target)
        } else {
            BindMount::readonly(source, target)
        };
        config.bind_mounts.push(mount);
    }
    let env = plan
        .env
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect::<Vec<_>>();
    let command = render_command_for_shell(&request.command);
    let mut sandbox = Sandbox::new(config);
    let (exit_code, stdout, stderr) = sandbox.execute("/bin/sh", &command, &[], env.as_slice())?;
    Ok(RecordCommandOutcome {
        exit_code,
        stdout,
        stderr,
    })
}

fn render_command_for_shell(command: &[String]) -> String {
    command
        .iter()
        .map(|arg| shell_quote_for_execution(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Quote a command argument for the temporary `/bin/sh` execution wrapper.
///
/// `$` remains unquoted so `$CONARY_DESTDIR` can expand inside the recording
/// sandbox. Do not use this helper for generated recipe text.
fn shell_quote_for_execution(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | '=' | '$'))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_config_mounts_exact_record_roots_and_exports_destdir() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let work = temp.path().join("work");
        let install = temp.path().join("destdir");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        std::fs::create_dir_all(&install).unwrap();

        let plan = sandbox_plan(&RecordCommandRequest {
            source_root: source.clone(),
            work_root: work.clone(),
            install_root: install.clone(),
            command: vec!["/bin/sh".to_string(), "-c".to_string(), "true".to_string()],
            unsafe_host: false,
        })
        .unwrap();

        assert!(plan.network_isolated);
        assert_eq!(plan.cwd, "/conary/source");
        assert_eq!(plan.env_value("DESTDIR"), Some("/conary/destdir"));
        assert_eq!(plan.env_value("CONARY_DESTDIR"), Some("/conary/destdir"));
        assert_eq!(plan.env_value("CONARY_WORKDIR"), Some("/conary/work"));
        assert!(plan.env_value("SOURCE_DATE_EPOCH").is_some());
        assert!(plan.has_mount(&source, "/conary/source", true));
        assert!(plan.has_mount(&work, "/conary/work", true));
        assert!(plan.has_mount(&install, "/conary/destdir", true));
    }

    #[test]
    fn unsafe_host_plan_is_explicit_and_still_scoped() {
        let temp = tempfile::tempdir().unwrap();
        let request = RecordCommandRequest {
            source_root: temp.path().join("source"),
            work_root: temp.path().join("work"),
            install_root: temp.path().join("destdir"),
            command: vec!["true".to_string()],
            unsafe_host: true,
        };
        std::fs::create_dir_all(&request.source_root).unwrap();
        std::fs::create_dir_all(&request.work_root).unwrap();
        std::fs::create_dir_all(&request.install_root).unwrap();

        let plan = sandbox_plan(&request).unwrap();
        assert!(plan.unsafe_host);
        assert_eq!(
            plan.env_value("DESTDIR"),
            Some(request.install_root.to_str().unwrap())
        );
        assert_eq!(
            plan.env_value("CONARY_DESTDIR"),
            Some(request.install_root.to_str().unwrap())
        );
        assert_eq!(
            plan.env_value("CONARY_WORKDIR"),
            Some(request.work_root.to_str().unwrap())
        );
        assert!(plan.env_value("SOURCE_DATE_EPOCH").is_some());
    }

    #[test]
    fn shell_quote_preserves_destdir_expansion() {
        assert_eq!(
            shell_quote_for_execution("$CONARY_DESTDIR/usr/bin"),
            "$CONARY_DESTDIR/usr/bin"
        );
        assert_eq!(
            render_command_for_shell(&["make install".to_string()]),
            "'make install'"
        );
    }

    #[test]
    fn unsafe_host_runner_executes_with_record_environment() {
        let temp = tempfile::tempdir().unwrap();
        let request = RecordCommandRequest {
            source_root: temp.path().join("source"),
            work_root: temp.path().join("work"),
            install_root: temp.path().join("destdir"),
            command: vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "printf '%s' \"$CONARY_DESTDIR\"".to_string(),
            ],
            unsafe_host: true,
        };
        std::fs::create_dir_all(&request.source_root).unwrap();
        std::fs::create_dir_all(&request.work_root).unwrap();
        std::fs::create_dir_all(&request.install_root).unwrap();

        let outcome = run_record_command(&request).unwrap();

        assert_eq!(outcome.exit_code, 0);
        assert_eq!(outcome.stdout, request.install_root.to_string_lossy());
    }
}
