// conary-core/src/container/execution/root_setup.rs

//! Selected-root, namespace, mount, and enforcement setup for sandboxed children.

use super::*;

impl Sandbox {
    /// Set up the container filesystem with bind mounts.
    pub(super) fn setup_container_fs(&self, root: &Path) -> Result<()> {
        for dir in &[
            "dev", "etc", "proc", "sys", "tmp", "usr", "lib", "lib64", "bin", "sbin", "var",
        ] {
            let path = root.join(dir);
            if !path.exists() {
                fs::create_dir_all(&path)?;
            }
        }

        let mut tmp_perms = fs::metadata(root.join("tmp"))?.permissions();
        tmp_perms.set_mode(0o1777);
        fs::set_permissions(root.join("tmp"), tmp_perms)?;

        let dev = root.join("dev");
        for node in &["null", "zero", "urandom", "random"] {
            let path = dev.join(node);
            if !path.exists() {
                File::create(&path)?;
            }
            let mut perms = fs::metadata(&path)?.permissions();
            perms.set_mode(0o666);
            fs::set_permissions(&path, perms)?;
        }

        Ok(())
    }

    /// Configure the isolated child and replace it with the requested process.
    pub(super) fn child_setup_and_execute(&self, execution: ChildExecution<'_>) -> Result<i32> {
        let ChildExecution {
            root,
            program,
            interpreter_args,
            script_path,
            args,
            env,
            userns_sync,
        } = execution;
        let script_in_container = script_path.map(|path| {
            path.strip_prefix(root)
                .map(|relative| Path::new("/").join(relative))
                .unwrap_or_else(|_| path.to_path_buf())
        });
        let namespace_flags: &[(bool, CloneFlags)] = &[
            (self.config.isolate_pid, CloneFlags::CLONE_NEWPID),
            (self.config.isolate_uts, CloneFlags::CLONE_NEWUTS),
            (self.config.isolate_ipc, CloneFlags::CLONE_NEWIPC),
            (self.config.isolate_mount, CloneFlags::CLONE_NEWNS),
            (self.config.isolate_network, CloneFlags::CLONE_NEWNET),
        ];
        let flags = namespace_flags
            .iter()
            .filter(|(enabled, _)| *enabled)
            .fold(CloneFlags::empty(), |acc, (_, flag)| acc | *flag);
        let flags_with_user = sandbox_namespace_flags(flags);
        let mut user_namespace_enabled = false;

        if !flags_with_user.is_empty() {
            if let Err(userns_error) = unshare(flags_with_user) {
                warn!(
                    "User namespace isolation unavailable ({}); continuing with existing namespace isolation",
                    userns_error
                );
                unshare(flags).map_err(|e| sandbox_error(format!("Unshare failed: {e}")))?;
                signal_parent_user_namespace_ready(userns_sync.as_ref(), false)?;
            } else {
                user_namespace_enabled = true;
                signal_parent_user_namespace_ready(userns_sync.as_ref(), true)?;
            }
        }

        if self.config.isolate_pid {
            match fork_process()
                .map_err(|error| sandbox_error(format!("PID namespace fork failed: {error}")))?
            {
                ForkResult::Parent { child } => return wait_for_pid_namespace_init(child),
                ForkResult::Child => {
                    let parent = unsafe { libc::getppid() };
                    set_parent_death_signal(libc::SIGKILL).map_err(|error| {
                        sandbox_error(format!(
                            "failed to bind PID namespace init lifetime to its monitor: {error}"
                        ))
                    })?;
                    if unsafe { libc::getppid() } != parent {
                        return Err(sandbox_error(
                            "PID namespace monitor exited during init setup",
                        ));
                    }
                }
            }
        }

        if self.config.isolate_network
            && let Ok(status) = std::process::Command::new("ip")
                .args(["link", "set", "lo", "up"])
                .status()
            && !status.success()
        {
            debug!("Failed to bring up loopback interface");
        }

        if self.config.isolate_uts && !self.config.hostname.is_empty() {
            let hostname = CString::new(self.config.hostname.as_str()).map_err(|e| {
                execution_error(
                    ScriptletFailureKind::ContractViolation,
                    format!("Invalid hostname: {e}"),
                )
            })?;
            if let Err(err) = sethostname_syscall(&hostname, self.config.hostname.len()) {
                warn!("sethostname failed: {err}");
            }
        }

        if self.config.isolate_mount {
            self.setup_mount_namespace(root, user_namespace_enabled)?;
        }

        self.apply_resource_limits()?;

        if let Some(ref policy) = self.config.capability_policy {
            match enforcement::apply_enforcement(policy) {
                Ok(report) => {
                    for warning in &report.warnings {
                        warn!("Enforcement setup: {warning}");
                    }
                    if report.landlock_applied {
                        debug!(
                            connect_tcp = ?report.connect_tcp_rules,
                            bind_tcp = ?report.bind_tcp_rules,
                            "Landlock filesystem/network enforcement active"
                        );
                    }
                    if report.seccomp_applied {
                        debug!("Seccomp syscall enforcement active");
                    }
                }
                Err(error) => {
                    if policy.mode == EnforcementMode::Enforce {
                        return Err(execution_error(
                            ScriptletFailureKind::EnforcementSetupFailed,
                            format!("Capability enforcement failed: {error}"),
                        ));
                    }
                    warn!("Capability enforcement skipped: {error}");
                }
            }
        }

        std::env::set_current_dir(&self.config.workdir)
            .map_err(|e| sandbox_error(format!("chdir failed: {e}")))?;

        let mut command = Command::new(program);
        command.args(interpreter_args);
        if let Some(script_in_container) = script_in_container.as_ref() {
            command.arg(script_in_container);
        }
        command
            .args(args)
            .stdin(Stdio::null())
            .env_clear()
            .env("HOME", "/root")
            .env("TERM", "dumb")
            .env("LANG", "C.UTF-8")
            .env("SHELL", "/bin/sh");

        if !env.iter().any(|(key, _)| *key == "PATH") {
            command.env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin");
        }
        for (key, value) in env {
            command.env(*key, *value);
        }

        let error = command.exec();
        Err(execution_error(
            ScriptletFailureKind::ProgramUnavailable,
            format!("Exec failed: {error}"),
        ))
    }

    fn setup_mount_namespace(&self, root: &Path, user_namespace_enabled: bool) -> Result<()> {
        mount::<str, str, str, str>(None, "/", None, MsFlags::MS_PRIVATE | MsFlags::MS_REC, None)
            .map_err(|e| sandbox_error(format!("mount --make-rprivate failed: {e}")))?;

        for bind_mount in &self.config.bind_mounts {
            if !bind_mount.source.exists() {
                debug!(
                    "Skipping bind mount, source doesn't exist: {:?}",
                    bind_mount.source
                );
                continue;
            }

            let target = root.join(
                bind_mount
                    .target
                    .strip_prefix("/")
                    .unwrap_or(&bind_mount.target),
            );
            if bind_mount.source.is_dir() {
                fs::create_dir_all(&target)?;
            } else {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                if !target.exists() {
                    File::create(&target)?;
                }
            }

            mount::<Path, Path, str, str>(
                Some(&bind_mount.source),
                &target,
                None,
                MsFlags::MS_BIND,
                None,
            )
            .map_err(|e| {
                debug!(
                    "Bind mount {:?} -> {:?} failed: {}",
                    bind_mount.source, target, e
                );
                sandbox_error(format!("Bind mount failed: {e}"))
            })?;

            if !bind_mount.writable
                && let Err(error) = mount::<Path, Path, str, str>(
                    None,
                    &target,
                    None,
                    MsFlags::MS_REMOUNT | MsFlags::MS_BIND | MsFlags::MS_RDONLY,
                    None,
                )
            {
                if bind_mount.target == Path::new("/etc/resolv.conf")
                    && self.try_fallback_readonly_copy(&bind_mount.source, &target)?
                {
                    continue;
                }
                self.handle_readonly_remount_failure(&target, error)?;
            }
        }

        if user_namespace_enabled {
            self.chroot_into(root)?;
            return Ok(());
        }

        if let Err(error) = self.try_pivot_root(root) {
            if self.is_enforce_mode() {
                return Err(sandbox_error(format!(
                    "pivot_root failed ({error}) and chroot fallback is not allowed in Enforce mode"
                )));
            }
            warn!(
                "pivot_root failed ({error}), falling back to chroot. \
                 This is less secure -- chroot can be escaped by a privileged process."
            );
            self.chroot_into(root)?;
        }

        Ok(())
    }

    fn is_enforce_mode(&self) -> bool {
        self.config
            .capability_policy
            .as_ref()
            .is_some_and(|policy| policy.mode == EnforcementMode::Enforce)
    }

    pub(in crate::container) fn try_fallback_readonly_copy(
        &self,
        source: &Path,
        target: &Path,
    ) -> Result<bool> {
        match umount2(target, MntFlags::MNT_DETACH) {
            Ok(()) | Err(nix::errno::Errno::EINVAL) => {}
            Err(error) => {
                return Err(sandbox_error(format!(
                    "failed to detach bind mount for {}: {error}",
                    target.display()
                )));
            }
        }

        fs::copy(source, target).map_err(|error| {
            sandbox_error(format!(
                "failed to copy {} into sandbox: {error}",
                source.display()
            ))
        })?;

        let mut permissions = fs::metadata(target)?.permissions();
        permissions.set_mode(0o444);
        fs::set_permissions(target, permissions)?;
        warn!(
            "Falling back to copied read-only {} inside sandbox",
            target.display()
        );
        Ok(true)
    }

    pub(in crate::container) fn handle_readonly_remount_failure(
        &self,
        target: &Path,
        error: nix::errno::Errno,
    ) -> Result<()> {
        let message = format!("read-only remount failed for {}: {error}", target.display());
        if self.is_enforce_mode() {
            return Err(execution_error(
                ScriptletFailureKind::EnforcementSetupFailed,
                message,
            ));
        }
        warn!("{message}");
        Ok(())
    }

    fn chroot_into(&self, root: &Path) -> Result<()> {
        let root_string = root.to_string_lossy().into_owned();
        let root_cstr = CString::new(root_string)
            .map_err(|e| sandbox_error(format!("Invalid root path: {e}")))?;
        chroot_syscall(&root_cstr).map_err(|e| sandbox_error(format!("chroot failed: {e}")))?;
        chdir_syscall(c"/")
            .map_err(|e| sandbox_error(format!("chdir after chroot failed: {e}")))?;
        Ok(())
    }

    fn try_pivot_root(&self, root: &Path) -> Result<()> {
        mount::<Path, Path, str, str>(Some(root), root, None, MsFlags::MS_BIND, None)
            .map_err(|e| sandbox_error(format!("bind mount for pivot_root: {e}")))?;

        let old_root = root.join(".old_root");
        std::fs::create_dir_all(&old_root)
            .map_err(|e| sandbox_error(format!("create old_root dir: {e}")))?;
        nix::unistd::pivot_root(root, &old_root)
            .map_err(|e| sandbox_error(format!("pivot_root failed: {e}")))?;
        std::env::set_current_dir("/")
            .map_err(|e| sandbox_error(format!("chdir / after pivot_root: {e}")))?;
        nix::mount::umount2(
            &std::path::PathBuf::from("/.old_root"),
            nix::mount::MntFlags::MNT_DETACH,
        )
        .map_err(|e| sandbox_error(format!("umount old_root: {e}")))?;
        let _ = std::fs::remove_dir("/.old_root");
        Ok(())
    }

    pub(super) fn apply_resource_limits(&self) -> Result<()> {
        set_rlimit(libc::RLIMIT_AS, self.config.memory_limit, "RLIMIT_AS");
        set_rlimit(libc::RLIMIT_CPU, self.config.cpu_time_limit, "RLIMIT_CPU");
        set_rlimit(
            libc::RLIMIT_FSIZE,
            self.config.file_size_limit,
            "RLIMIT_FSIZE",
        );
        set_rlimit(libc::RLIMIT_NPROC, self.config.nproc_limit, "RLIMIT_NPROC");
        Ok(())
    }
}

fn wait_for_pid_namespace_init(child: Pid) -> Result<i32> {
    loop {
        match waitpid(child, None) {
            Ok(WaitStatus::Exited(_, code)) => return Ok(code),
            Ok(WaitStatus::Signaled(_, signal, _)) => return Ok(128 + signal as i32),
            Ok(_) => {}
            Err(nix::errno::Errno::EINTR) => {}
            Err(error) => {
                return Err(sandbox_error(format!(
                    "failed to wait for PID namespace init: {error}"
                )));
            }
        }
    }
}
