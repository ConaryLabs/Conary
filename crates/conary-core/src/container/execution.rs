// conary-core/src/container/execution.rs

use super::*;

mod root_setup;

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
}

impl Sandbox {
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
                )?;
                drop(userns_request_read_fd);
                drop(userns_ack_write_fd);

                // Wait for child, then read captured output
                let (code, _, _) = self.wait_for_child(child, start)?;

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
                // Child: close read ends of pipes
                drop(stdout_read_fd);
                drop(stderr_read_fd);
                drop(userns_request_read_fd);
                drop(userns_ack_write_fd);

                if unsafe { libc::dup2(stdin_file.as_raw_fd(), 0) } < 0 {
                    eprintln!(
                        "failed to attach sandbox stdin: {}",
                        std::io::Error::last_os_error()
                    );
                    std::process::exit(127);
                }

                // Redirect stdout and stderr to pipe write ends
                let mut stdout_target = match adopt_raw_fd(1) {
                    Ok(fd) => fd,
                    Err(err) => {
                        eprintln!("{err}");
                        std::process::exit(127);
                    }
                };
                let mut stderr_target = match adopt_raw_fd(2) {
                    Ok(fd) => fd,
                    Err(err) => {
                        eprintln!("{err}");
                        std::process::exit(127);
                    }
                };
                let _ = nix::unistd::dup2(&stdout_write_fd, &mut stdout_target);
                let _ = nix::unistd::dup2(&stderr_write_fd, &mut stderr_target);
                // Prevent OwnedFd from closing stdout/stderr when dropped
                std::mem::forget(stdout_target);
                std::mem::forget(stderr_target);
                drop(stdout_write_fd);
                drop(stderr_write_fd);

                // Child: set up namespaces and execute
                let result = self.child_setup_and_execute(ChildExecution {
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
                });

                // Exit with appropriate code
                match result {
                    Ok(code) => std::process::exit(code),
                    Err(err) => {
                        eprintln!("{err}");
                        std::process::exit(127);
                    }
                }
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

        let (stdout_read_fd, stdout_write_fd) = nix::unistd::pipe()
            .map_err(|e| sandbox_error(format!("Failed to create stdout pipe: {e}")))?;
        let (stderr_read_fd, stderr_write_fd) = nix::unistd::pipe()
            .map_err(|e| sandbox_error(format!("Failed to create stderr pipe: {e}")))?;
        let (userns_request_read_fd, userns_request_write_fd) = nix::unistd::pipe()
            .map_err(|e| sandbox_error(format!("Failed to create userns pipe: {e}")))?;
        let (userns_ack_read_fd, userns_ack_write_fd) = nix::unistd::pipe()
            .map_err(|e| sandbox_error(format!("Failed to create userns ack pipe: {e}")))?;

        let start = Instant::now();

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
                )?;
                drop(userns_request_read_fd);
                drop(userns_ack_write_fd);

                let (code, _, _) = self.wait_for_child(child, start)?;

                let mut stdout_str = String::new();
                let mut stdout_file = std::fs::File::from(stdout_read_fd);
                let _ = stdout_file.read_to_string(&mut stdout_str);

                let mut stderr_str = String::new();
                let mut stderr_file = std::fs::File::from(stderr_read_fd);
                let _ = stderr_file.read_to_string(&mut stderr_str);

                Ok((code, stdout_str, stderr_str))
            }
            Ok(ForkResult::Child) => {
                drop(stdout_read_fd);
                drop(stderr_read_fd);
                drop(userns_request_read_fd);
                drop(userns_ack_write_fd);

                if unsafe { libc::dup2(stdin_file.as_raw_fd(), 0) } < 0 {
                    eprintln!(
                        "failed to attach sandbox stdin: {}",
                        std::io::Error::last_os_error()
                    );
                    std::process::exit(127);
                }

                let mut stdout_target = match adopt_raw_fd(1) {
                    Ok(fd) => fd,
                    Err(err) => {
                        eprintln!("{err}");
                        std::process::exit(127);
                    }
                };
                let mut stderr_target = match adopt_raw_fd(2) {
                    Ok(fd) => fd,
                    Err(err) => {
                        eprintln!("{err}");
                        std::process::exit(127);
                    }
                };
                let _ = nix::unistd::dup2(&stdout_write_fd, &mut stdout_target);
                let _ = nix::unistd::dup2(&stderr_write_fd, &mut stderr_target);
                std::mem::forget(stdout_target);
                std::mem::forget(stderr_target);
                drop(stdout_write_fd);
                drop(stderr_write_fd);

                let result = self.child_setup_and_execute(ChildExecution {
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
                });

                match result {
                    Ok(code) => std::process::exit(code),
                    Err(err) => {
                        eprintln!("{err}");
                        std::process::exit(127);
                    }
                }
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
    fn wait_for_child(&self, child: Pid, start: Instant) -> Result<(i32, String, String)> {
        loop {
            // Check timeout
            if start.elapsed() > self.config.timeout {
                // Kill the child
                let _ = kill(child, Signal::SIGKILL);
                return Err(execution_error(
                    ScriptletFailureKind::ScriptTimedOut,
                    format!("Script timed out after {:?}", self.config.timeout),
                ));
            }

            // Non-blocking wait
            match waitpid(child, Some(nix::sys::wait::WaitPidFlag::WNOHANG)) {
                Ok(WaitStatus::Exited(_, code)) => {
                    return Ok((code, String::new(), String::new()));
                }
                Ok(WaitStatus::Signaled(_, sig, _)) => {
                    return Err(execution_error(
                        ScriptletFailureKind::ScriptExited,
                        format!("Script killed by signal {sig:?}"),
                    ));
                }
                Ok(WaitStatus::StillAlive) | Ok(_) => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => {
                    return Err(execution_error(
                        ScriptletFailureKind::ProcessSetupFailed,
                        format!("Wait failed: {e}"),
                    ));
                }
            }
        }
    }

    fn complete_user_namespace_handshake(
        &self,
        child: Pid,
        request_fd: &std::os::fd::OwnedFd,
        ack_fd: &std::os::fd::OwnedFd,
    ) -> Result<()> {
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
