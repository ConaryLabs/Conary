// conary-core/src/container/execution.rs

use super::*;

mod process_wait;
mod root_setup;

use process_wait::{
    ChildWaitOutcome, terminate_and_reap, wait_for_child_until, wait_until_readable,
};

fn execution_error(kind: ScriptletFailureKind, message: impl Into<String>) -> Error {
    Error::scriptlet(kind, message)
}

fn sandbox_error(message: impl Into<String>) -> Error {
    execution_error(ScriptletFailureKind::SandboxSetupUnavailable, message)
}

struct ChildExecution<'a> {
    root: &'a Path,
    program: &'a str,
    interpreter_args: &'a [String],
    script_path: Option<&'a Path>,
    args: &'a [String],
    env: &'a [(&'a str, &'a str)],
    userns_sync: Option<UserNamespaceSync>,
    deadline: Instant,
    enforcement: Option<enforcement::PreparedEnforcement<'a>>,
}

struct ForkedChild<'a> {
    stdin_fd: std::os::fd::RawFd,
    stdout_read_fd: std::os::fd::OwnedFd,
    stdout_write_fd: std::os::fd::OwnedFd,
    stderr_read_fd: std::os::fd::OwnedFd,
    stderr_write_fd: std::os::fd::OwnedFd,
    userns_request_read_fd: std::os::fd::OwnedFd,
    userns_ack_write_fd: std::os::fd::OwnedFd,
    execution: ChildExecution<'a>,
}

impl Sandbox {
    fn prepare_enforcement(&self) -> Result<Option<enforcement::PreparedEnforcement<'_>>> {
        self.config
            .capability_policy
            .as_ref()
            .map(enforcement::prepare_enforcement)
            .transpose()
            .map_err(|error| {
                execution_error(
                    ScriptletFailureKind::EnforcementSetupFailed,
                    format!("Capability enforcement preparation failed: {error}"),
                )
            })
    }

    /// Create a new sandbox with the given configuration
    pub fn new(config: ContainerConfig) -> Self {
        Self { config }
    }

    /// Create a sandbox with default configuration
    pub fn with_defaults() -> Self {
        Self::new(ContainerConfig::default())
    }

    /// Create a strict sandbox with maximum isolation
    pub fn strict() -> Self {
        Self::new(ContainerConfig::strict())
    }

    /// Execute a script in the sandbox
    ///
    /// Returns the exit code and captured output.
    pub fn execute(
        &mut self,
        interpreter: &str,
        script_content: &str,
        args: &[String],
        env: &[(&str, &str)],
    ) -> Result<(i32, String, String)> {
        self.execute_with_interpreter_args_and_stdin(
            interpreter,
            &[],
            script_content,
            args,
            env,
            &[],
        )
    }

    /// Execute a script with exact non-interactive standard input.
    pub fn execute_with_stdin(
        &mut self,
        interpreter: &str,
        script_content: &str,
        args: &[String],
        env: &[(&str, &str)],
        stdin: &[u8],
    ) -> Result<(i32, String, String)> {
        self.execute_with_interpreter_args_and_stdin(
            interpreter,
            &[],
            script_content,
            args,
            env,
            stdin,
        )
    }

    /// Execute a script with the source package's exact interpreter argument
    /// vector and non-interactive standard input.
    pub fn execute_with_interpreter_args_and_stdin(
        &mut self,
        interpreter: &str,
        interpreter_args: &[String],
        script_content: &str,
        args: &[String],
        env: &[(&str, &str)],
        stdin: &[u8],
    ) -> Result<(i32, String, String)> {
        self.execute_bytes_with_interpreter_args_and_stdin(
            interpreter,
            interpreter_args,
            script_content.as_bytes(),
            args,
            env,
            stdin,
        )
    }

    /// Execute exact source-package script bytes with interpreter arguments
    /// and non-interactive standard input.
    pub fn execute_bytes_with_interpreter_args_and_stdin(
        &mut self,
        interpreter: &str,
        interpreter_args: &[String],
        script_content: &[u8],
        args: &[String],
        env: &[(&str, &str)],
        stdin: &[u8],
    ) -> Result<(i32, String, String)> {
        // Check if we can use namespace isolation
        let can_isolate = isolation_available();

        if can_isolate && self.config.isolate_mount {
            self.execute_isolated(
                interpreter,
                interpreter_args,
                script_content,
                args,
                env,
                stdin,
            )
        } else {
            // If isolation is required (hermetic/network isolated) but unavailable, FAIL.
            // Do not fall back to unsafe execution for hermetic builds.
            if self.config.isolate_network || self.config.is_pristine() {
                return Err(sandbox_error(
                    "Protected sandboxing or hermetic build requires namespace isolation, but it is not available on this system. \
                     (Root privileges or unprivileged user namespaces required)".to_string()
                ));
            }

            // Refuse to fall back when running as root -- execute_limited provides no
            // isolation, so untrusted scriptlets would get full root access.
            if Uid::effective().is_root() {
                return Err(sandbox_error(
                    "Namespace isolation unavailable while running as root \
                     — refusing to execute scriptlet without sandboxing"
                        .to_string(),
                ));
            }

            // Fall back to simple resource-limited execution
            if self.config.isolate_mount {
                warn!("Namespace isolation not available, falling back to resource limits only");
            }
            self.execute_limited(
                interpreter,
                interpreter_args,
                script_content,
                args,
                env,
                stdin,
            )
        }
    }

    /// Execute a command directly in the sandbox without a shell wrapper.
    ///
    /// This is important for seccomp-enforced flows where spawning an
    /// intermediate shell can require extra syscalls that the declared
    /// capability profile does not permit.
    pub fn execute_command(
        &mut self,
        program: &str,
        args: &[String],
        env: &[(&str, &str)],
    ) -> Result<(i32, String, String)> {
        self.execute_command_with_stdin(program, args, env, &[])
    }

    /// Execute a command with exact non-interactive standard input.
    pub fn execute_command_with_stdin(
        &mut self,
        program: &str,
        args: &[String],
        env: &[(&str, &str)],
        stdin: &[u8],
    ) -> Result<(i32, String, String)> {
        let can_isolate = isolation_available();

        if can_isolate && self.config.isolate_mount {
            self.execute_isolated_command(program, args, env, stdin)
        } else {
            if self.config.isolate_network || self.config.is_pristine() {
                return Err(sandbox_error(
                    "Protected sandboxing or hermetic build requires namespace isolation, but it is not available on this system. \
                     (Root privileges or unprivileged user namespaces required)".to_string()
                ));
            }

            if Uid::effective().is_root() {
                return Err(sandbox_error(
                    "Namespace isolation unavailable while running as root \
                     — refusing to execute scriptlet without sandboxing"
                        .to_string(),
                ));
            }

            if self.config.isolate_mount {
                warn!("Namespace isolation not available, falling back to resource limits only");
            }
            self.execute_limited_command(program, args, env, stdin)
        }
    }

    /// Execute with full namespace isolation (requires root)
    fn execute_isolated(
        &mut self,
        interpreter: &str,
        interpreter_args: &[String],
        script_content: &[u8],
        args: &[String],
        env: &[(&str, &str)],
        stdin: &[u8],
    ) -> Result<(i32, String, String)> {
        // Create temporary root directory for the container
        let root_dir = TempDir::new()?;

        // Set up the container filesystem
        self.setup_container_fs(root_dir.path())?;

        let script_path = root_dir.path().join("script.sh");
        write_executable_script_bytes(&script_path, script_content)?;
        prepare_user_namespace_entrypoint(root_dir.path(), &script_path)?;
        let stdin_path = root_dir.path().join(".conary-stdin");
        fs::write(&stdin_path, stdin)?;
        let stdin_file = File::open(&stdin_path)?;
        let prepared_enforcement = self.prepare_enforcement()?;

        // Set up pipes before fork to capture child stdout/stderr
        let (stdout_read_fd, stdout_write_fd) = nix::unistd::pipe()
            .map_err(|e| sandbox_error(format!("Failed to create stdout pipe: {e}")))?;
        let (stderr_read_fd, stderr_write_fd) = nix::unistd::pipe()
            .map_err(|e| sandbox_error(format!("Failed to create stderr pipe: {e}")))?;
        let (userns_request_read_fd, userns_request_write_fd) = nix::unistd::pipe()
            .map_err(|e| sandbox_error(format!("Failed to create userns pipe: {e}")))?;
        let (userns_ack_read_fd, userns_ack_write_fd) = nix::unistd::pipe()
            .map_err(|e| sandbox_error(format!("Failed to create userns ack pipe: {e}")))?;

        // Fork and execute in isolated namespaces
        let start = Instant::now();
        let deadline = start.checked_add(self.config.timeout).unwrap_or(start);

        match fork_process() {
            Ok(ForkResult::Parent { child }) => {
                // Parent: close write ends of pipes (dropping OwnedFd closes them)
                drop(stdout_write_fd);
                drop(stderr_write_fd);
                drop(userns_request_write_fd);
                drop(userns_ack_read_fd);

                self.complete_user_namespace_handshake(
                    child,
                    &userns_request_read_fd,
                    &userns_ack_write_fd,
                    deadline,
                )?;
                drop(userns_request_read_fd);
                drop(userns_ack_write_fd);

                // Wait for child, then read captured output
                let (code, _, _) = self.wait_for_child(child, deadline)?;

                // Read stdout from pipe
                let mut stdout_str = String::new();
                let mut stdout_file = std::fs::File::from(stdout_read_fd);
                let _ = stdout_file.read_to_string(&mut stdout_str);

                // Read stderr from pipe
                let mut stderr_str = String::new();
                let mut stderr_file = std::fs::File::from(stderr_read_fd);
                let _ = stderr_file.read_to_string(&mut stderr_str);

                Ok((code, stdout_str, stderr_str))
            }
            Ok(ForkResult::Child) => {
                self.run_forked_child(ForkedChild {
                    stdin_fd: stdin_file.as_raw_fd(),
                    stdout_read_fd,
                    stdout_write_fd,
                    stderr_read_fd,
                    stderr_write_fd,
                    userns_request_read_fd,
                    userns_ack_write_fd,
                    execution: ChildExecution {
                        root: root_dir.path(),
                        program: interpreter,
                        interpreter_args,
                        script_path: Some(&script_path),
                        args,
                        env,
                        userns_sync: Some(UserNamespaceSync {
                            request_fd: userns_request_write_fd,
                            ack_fd: userns_ack_read_fd,
                        }),
                        deadline,
                        enforcement: prepared_enforcement,
                    },
                });
            }
            Err(e) => {
                drop(stdout_read_fd);
                drop(stdout_write_fd);
                drop(stderr_read_fd);
                drop(stderr_write_fd);
                drop(userns_request_read_fd);
                drop(userns_request_write_fd);
                drop(userns_ack_read_fd);
                drop(userns_ack_write_fd);
                Err(sandbox_error(format!("Fork failed: {}", e)))
            }
        }
    }

    fn execute_isolated_command(
        &mut self,
        program: &str,
        args: &[String],
        env: &[(&str, &str)],
        stdin: &[u8],
    ) -> Result<(i32, String, String)> {
        let root_dir = TempDir::new()?;
        self.setup_container_fs(root_dir.path())?;
        prepare_user_namespace_root(root_dir.path())?;
        let stdin_path = root_dir.path().join(".conary-stdin");
        fs::write(&stdin_path, stdin)?;
        let stdin_file = File::open(&stdin_path)?;
        let prepared_enforcement = self.prepare_enforcement()?;

        let (stdout_read_fd, stdout_write_fd) = nix::unistd::pipe()
            .map_err(|e| sandbox_error(format!("Failed to create stdout pipe: {e}")))?;
        let (stderr_read_fd, stderr_write_fd) = nix::unistd::pipe()
            .map_err(|e| sandbox_error(format!("Failed to create stderr pipe: {e}")))?;
        let (userns_request_read_fd, userns_request_write_fd) = nix::unistd::pipe()
            .map_err(|e| sandbox_error(format!("Failed to create userns pipe: {e}")))?;
        let (userns_ack_read_fd, userns_ack_write_fd) = nix::unistd::pipe()
            .map_err(|e| sandbox_error(format!("Failed to create userns ack pipe: {e}")))?;

        let start = Instant::now();
        let deadline = start.checked_add(self.config.timeout).unwrap_or(start);

        match fork_process() {
            Ok(ForkResult::Parent { child }) => {
                drop(stdout_write_fd);
                drop(stderr_write_fd);
                drop(userns_request_write_fd);
                drop(userns_ack_read_fd);

                self.complete_user_namespace_handshake(
                    child,
                    &userns_request_read_fd,
                    &userns_ack_write_fd,
                    deadline,
                )?;
                drop(userns_request_read_fd);
                drop(userns_ack_write_fd);

                let (code, _, _) = self.wait_for_child(child, deadline)?;

                let mut stdout_str = String::new();
                let mut stdout_file = std::fs::File::from(stdout_read_fd);
                let _ = stdout_file.read_to_string(&mut stdout_str);

                let mut stderr_str = String::new();
                let mut stderr_file = std::fs::File::from(stderr_read_fd);
                let _ = stderr_file.read_to_string(&mut stderr_str);

                Ok((code, stdout_str, stderr_str))
            }
            Ok(ForkResult::Child) => {
                self.run_forked_child(ForkedChild {
                    stdin_fd: stdin_file.as_raw_fd(),
                    stdout_read_fd,
                    stdout_write_fd,
                    stderr_read_fd,
                    stderr_write_fd,
                    userns_request_read_fd,
                    userns_ack_write_fd,
                    execution: ChildExecution {
                        root: root_dir.path(),
                        program,
                        interpreter_args: &[],
                        script_path: None,
                        args,
                        env,
                        userns_sync: Some(UserNamespaceSync {
                            request_fd: userns_request_write_fd,
                            ack_fd: userns_ack_read_fd,
                        }),
                        deadline,
                        enforcement: prepared_enforcement,
                    },
                });
            }
            Err(e) => {
                drop(stdout_read_fd);
                drop(stdout_write_fd);
                drop(stderr_read_fd);
                drop(stderr_write_fd);
                drop(userns_request_read_fd);
                drop(userns_request_write_fd);
                drop(userns_ack_read_fd);
                drop(userns_ack_write_fd);
                Err(sandbox_error(format!("Fork failed: {}", e)))
            }
        }
    }

    /// Execute with just resource limits (no namespace isolation)
    fn execute_limited(
        &self,
        interpreter: &str,
        interpreter_args: &[String],
        script_content: &[u8],
        args: &[String],
        env: &[(&str, &str)],
        stdin: &[u8],
    ) -> Result<(i32, String, String)> {
        let temp_dir = TempDir::new()?;
        let script_path = temp_dir.path().join("script.sh");
        write_executable_script_bytes(&script_path, script_content)?;

        // Apply resource limits before exec
        self.apply_resource_limits()?;
        let stdin_file = prepared_stdin(stdin)?;

        let mut cmd = Command::new(interpreter);
        cmd.args(interpreter_args)
            .arg(&script_path)
            .args(args)
            .stdin(Stdio::from(stdin_file))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_clear()
            .env("HOME", "/root")
            .env("TERM", "dumb")
            .env("LANG", "C.UTF-8")
            .env("SHELL", "/bin/sh");

        // Set PATH fallback only if the caller didn't provide one.
        // Bootstrap builds need the toolchain PATH to take precedence.
        let has_custom_path = env.iter().any(|(k, _)| *k == "PATH");
        if !has_custom_path {
            cmd.env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin");
        }

        for (key, value) in env {
            cmd.env(*key, *value);
        }

        let mut child = cmd.spawn().map_err(|e| {
            execution_error(
                ScriptletFailureKind::ProcessSetupFailed,
                format!("Failed to spawn: {e}"),
            )
        })?;

        let outcome = wait_with_output(&mut child, self.config.timeout)?;
        if outcome.timed_out {
            Err(execution_error(
                ScriptletFailureKind::ScriptTimedOut,
                format!("Script timed out after {:?}", self.config.timeout),
            ))
        } else {
            let code = outcome
                .status
                .expect("child wait helper must return a status when not timed out")
                .code()
                .unwrap_or(-1);
            Ok((
                code,
                String::from_utf8_lossy(&outcome.stdout).into_owned(),
                String::from_utf8_lossy(&outcome.stderr).into_owned(),
            ))
        }
    }

    fn execute_limited_command(
        &self,
        program: &str,
        args: &[String],
        env: &[(&str, &str)],
        stdin: &[u8],
    ) -> Result<(i32, String, String)> {
        self.apply_resource_limits()?;
        let stdin_file = prepared_stdin(stdin)?;

        let mut cmd = Command::new(program);
        cmd.args(args)
            .stdin(Stdio::from(stdin_file))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_clear()
            .env("HOME", "/root")
            .env("TERM", "dumb")
            .env("LANG", "C.UTF-8")
            .env("SHELL", "/bin/sh");

        let has_custom_path = env.iter().any(|(k, _)| *k == "PATH");
        if !has_custom_path {
            cmd.env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin");
        }

        for (key, value) in env {
            cmd.env(*key, *value);
        }

        let mut child = cmd.spawn().map_err(|e| {
            execution_error(
                ScriptletFailureKind::ProcessSetupFailed,
                format!("Failed to spawn: {e}"),
            )
        })?;

        let outcome = wait_with_output(&mut child, self.config.timeout)?;
        if outcome.timed_out {
            Err(execution_error(
                ScriptletFailureKind::ScriptTimedOut,
                format!("Script timed out after {:?}", self.config.timeout),
            ))
        } else {
            let code = outcome
                .status
                .expect("child wait helper must return a status when not timed out")
                .code()
                .unwrap_or(-1);
            Ok((
                code,
                String::from_utf8_lossy(&outcome.stdout).into_owned(),
                String::from_utf8_lossy(&outcome.stderr).into_owned(),
            ))
        }
    }

    /// Wait for child process with timeout
    fn wait_for_child(&self, child: Pid, deadline: Instant) -> Result<(i32, String, String)> {
        match wait_for_child_until(child, deadline) {
            Ok(ChildWaitOutcome::Exited(code)) => Ok((code, String::new(), String::new())),
            Ok(ChildWaitOutcome::Signaled(signal)) => Err(execution_error(
                ScriptletFailureKind::ScriptExited,
                format!("Script killed by signal {signal:?}"),
            )),
            Ok(ChildWaitOutcome::TimedOut) => Err(execution_error(
                ScriptletFailureKind::ScriptTimedOut,
                format!("Script timed out after {:?}", self.config.timeout),
            )),
            Err(error) => Err(execution_error(
                ScriptletFailureKind::ProcessSetupFailed,
                format!("Wait failed: {error}"),
            )),
        }
    }

    fn complete_user_namespace_handshake(
        &self,
        child: Pid,
        request_fd: &std::os::fd::OwnedFd,
        ack_fd: &std::os::fd::OwnedFd,
        deadline: Instant,
    ) -> Result<()> {
        let result =
            self.complete_user_namespace_handshake_inner(child, request_fd, ack_fd, deadline);
        if let Err(error) = result {
            if let Err(cleanup_error) = terminate_and_reap(child) {
                return Err(execution_error(
                    ScriptletFailureKind::ProcessSetupFailed,
                    format!(
                        "{error}; additionally failed to terminate and reap sandbox child: {cleanup_error}"
                    ),
                ));
            }
            return Err(error);
        }
        Ok(())
    }

    fn complete_user_namespace_handshake_inner(
        &self,
        child: Pid,
        request_fd: &std::os::fd::OwnedFd,
        ack_fd: &std::os::fd::OwnedFd,
        deadline: Instant,
    ) -> Result<()> {
        if !wait_until_readable(request_fd.as_fd(), deadline).map_err(|error| {
            sandbox_error(format!("User namespace handshake poll failed: {error}"))
        })? {
            return Err(execution_error(
                ScriptletFailureKind::ScriptTimedOut,
                format!("Sandbox setup timed out after {:?}", self.config.timeout),
            ));
        }
        let mut message = [0_u8; 1];
        let bytes_read = nix::unistd::read(request_fd, &mut message)
            .map_err(|e| sandbox_error(format!("User namespace handshake failed: {e}")))?;
        if bytes_read == 0 {
            return Ok(());
        }

        match message[0] {
            b'U' => {
                configure_user_namespace_root_mapping_for_pid(
                    child,
                    sandbox_host_uid(Uid::effective().as_raw()),
                    sandbox_host_gid(Gid::effective().as_raw()),
                )?;
            }
            b'N' => {}
            other => {
                return Err(sandbox_error(format!(
                    "Unexpected user namespace handshake message: {other}"
                )));
            }
        }

        nix::unistd::write(ack_fd, b"O")
            .map_err(|e| sandbox_error(format!("User namespace handshake ack failed: {e}")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_timeout_terminates_and_reaps_child() {
        let sandbox = Sandbox::new(ContainerConfig::minimal(Duration::from_millis(20)));
        let (request_read, request_write) = nix::unistd::pipe().expect("request pipe");
        let (ack_read, ack_write) = nix::unistd::pipe().expect("ack pipe");

        // SAFETY: the child closes owned descriptors, then performs only
        // async-signal-safe pause/_exit calls.
        match unsafe { nix::unistd::fork() }.expect("test fork should succeed") {
            ForkResult::Child => {
                drop(request_read);
                drop(ack_write);
                drop(ack_read);
                loop {
                    unsafe { libc::pause() };
                }
            }
            ForkResult::Parent { child } => {
                drop(request_write);
                drop(ack_read);
                let error = sandbox
                    .complete_user_namespace_handshake(
                        child,
                        &request_read,
                        &ack_write,
                        Instant::now() + Duration::from_millis(20),
                    )
                    .expect_err("silent child must hit the handshake deadline");
                assert!(error.to_string().contains("Sandbox setup timed out"));
                assert_eq!(
                    waitpid(child, Some(nix::sys::wait::WaitPidFlag::WNOHANG)),
                    Err(nix::errno::Errno::ECHILD)
                );
            }
        }
    }
}
