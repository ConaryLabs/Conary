// crates/conary-core/src/scriptlet/boot_runtime_capture.rs

//! Synchronous capture boundary for boot and kernel maintenance commands.
//!
//! Every proxy request is parsed by `crate::boot_runtime` before a response is
//! returned. Mutation forms become successful deferred work. Information and
//! status forms may run only a staged copy of an existing selected-root
//! provider, inside a fresh selected-root namespace.

use super::lifecycle_bridge::{
    LifecycleBridgeConfig, LifecycleBridgeEndpoint, LifecycleBridgeHandler,
    LifecycleBridgeHandlerError, LifecycleBridgeRequest, LifecycleBridgeResponse,
    LifecycleBridgeSession, executable_bridge_shim,
};
use super::process::{TargetCommandBindMount, TargetRootScript, configure_target_command_boundary};
use super::runtime::apply_sanitized_command_env;
use crate::boot_runtime::{
    BootRuntimeInvocation, InvocationDisposition, parse_boot_runtime_invocation,
};
use crate::child_wait::wait_with_output;
use crate::error::{Error, Result, ScriptletFailureKind};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const DEFAULT_SCRIPTLET_PATH: &str = "/usr/sbin:/usr/bin:/sbin:/bin";

/// Paths declared by the pinned upstream install contracts. Absolute source
/// lifecycle spellings remain covered even when a script replaces `PATH`.
const DECLARED_ENDPOINTS: &[(&str, &str)] = &[
    ("depmod", "/usr/sbin/depmod"),
    ("modprobe", "/usr/sbin/modprobe"),
    ("dracut", "/usr/bin/dracut"),
    ("mkinitcpio", "/usr/bin/mkinitcpio"),
    ("update-initramfs", "/usr/sbin/update-initramfs"),
    ("kernel-install", "/usr/bin/kernel-install"),
    ("installkernel", "/sbin/installkernel"),
    ("dkms", "/usr/sbin/dkms"),
    ("grub-mkconfig", "/usr/sbin/grub-mkconfig"),
    ("grub2-mkconfig", "/usr/sbin/grub2-mkconfig"),
    ("update-grub", "/usr/sbin/update-grub"),
    ("update-grub2", "/usr/sbin/update-grub2"),
    ("bootctl", "/usr/bin/bootctl"),
];

pub(super) struct BootRuntimeCaptureSession {
    bridge: LifecycleBridgeSession,
    invocations: Arc<Mutex<Vec<BootRuntimeInvocation>>>,
}

struct BootRuntimeHandler {
    root: PathBuf,
    environment: Vec<(String, String)>,
    timeout: Duration,
    targets: BTreeMap<String, &'static str>,
    providers: BTreeMap<String, String>,
    command_providers: BTreeMap<&'static str, String>,
    invocations: Arc<Mutex<Vec<BootRuntimeInvocation>>>,
}

struct RoutedHandler {
    boot: Arc<BootRuntimeHandler>,
    fallback: Option<Arc<dyn LifecycleBridgeHandler>>,
}

impl BootRuntimeCaptureSession {
    pub(super) fn stage(
        staged: &mut TargetRootScript,
        root: &Path,
        environment: &[(&str, &str)],
        timeout: Duration,
        fallback: Option<&LifecycleBridgeConfig>,
    ) -> Result<Self> {
        let canonical_root = fs::canonicalize(root).map_err(capture_setup_error)?;
        if canonical_root == Path::new("/") {
            return Err(capture_contract_error(
                "boot runtime capture cannot stage against the host root",
            ));
        }

        let endpoint_contract = endpoint_contract(environment);
        let mut targets = BTreeMap::new();
        let mut providers = BTreeMap::new();
        let mut providers_by_command = BTreeMap::<&'static str, BTreeSet<String>>::new();
        let mut endpoints = Vec::with_capacity(
            endpoint_contract.len() + fallback.map_or(0, |config| config.endpoints().len()),
        );

        for (index, (target, command)) in endpoint_contract.into_iter().enumerate() {
            let guest_target = target.to_string_lossy().into_owned();
            targets.insert(guest_target.clone(), command);
            if let Some(bytes) = existing_provider(&canonical_root, &target)? {
                let directory = format!("boot.runtime.provider.{index}");
                let provider =
                    staged.create_extra_directory_file(&directory, command, &bytes, 0o700)?;
                let provider = guest_path(&canonical_root, &provider)?;
                providers.insert(guest_target.clone(), provider.clone());
                providers_by_command
                    .entry(command)
                    .or_default()
                    .insert(provider);
            }
            endpoints.push(LifecycleBridgeEndpoint::new(
                target,
                executable_bridge_shim(),
            ));
        }

        let command_providers = providers_by_command
            .into_iter()
            .filter_map(|(command, providers)| {
                let mut providers = providers.into_iter();
                let provider = providers.next()?;
                providers.next().is_none().then_some((command, provider))
            })
            .collect();
        let invocations = Arc::new(Mutex::new(Vec::new()));
        let handler = Arc::new(BootRuntimeHandler {
            root: canonical_root,
            environment: environment
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect(),
            timeout,
            targets,
            providers,
            command_providers,
            invocations: Arc::clone(&invocations),
        });
        if let Some(fallback) = fallback {
            endpoints.extend_from_slice(fallback.endpoints());
        }
        let routed = RoutedHandler {
            boot: handler,
            fallback: fallback.map(LifecycleBridgeConfig::shared_handler),
        };
        let mut config = LifecycleBridgeConfig::with_shared_handler(endpoints, Arc::new(routed));
        if let Some(fallback) = fallback {
            config = config.with_io_timeout(fallback.io_timeout());
        }
        let bridge = LifecycleBridgeSession::stage(staged, root, &config)?;
        Ok(Self {
            bridge,
            invocations,
        })
    }

    pub(super) fn mounts(&self) -> &[TargetCommandBindMount] {
        self.bridge.mounts()
    }

    pub(super) fn configure_command(&self, command: &mut Command) -> Result<()> {
        self.bridge.configure_command(command)
    }

    pub(super) fn child_spawned(&mut self) {
        self.bridge.child_spawned();
    }

    pub(super) fn finish(self) -> Result<Vec<BootRuntimeInvocation>> {
        let Self {
            bridge,
            invocations,
        } = self;
        bridge.finish()?;
        Ok(std::mem::take(
            &mut *invocations
                .lock()
                .expect("boot runtime invocation collector poisoned"),
        ))
    }
}

impl LifecycleBridgeHandler for RoutedHandler {
    fn handle(
        &self,
        request: &LifecycleBridgeRequest,
    ) -> std::result::Result<LifecycleBridgeResponse, LifecycleBridgeHandlerError> {
        if self.boot.claims(request.command()) {
            return self.boot.handle(request);
        }
        let fallback = self.fallback.as_ref().ok_or_else(|| {
            LifecycleBridgeHandlerError::new("lifecycle bridge request has no typed handler")
        })?;
        fallback.handle(request)
    }
}

impl BootRuntimeHandler {
    fn claims(&self, requested: &[u8]) -> bool {
        let Ok(requested) = std::str::from_utf8(requested) else {
            return false;
        };
        self.targets.contains_key(requested)
            || (!requested.contains('/')
                && DECLARED_ENDPOINTS
                    .iter()
                    .any(|(command, _)| *command == requested))
    }

    fn handle(
        &self,
        request: &LifecycleBridgeRequest,
    ) -> std::result::Result<LifecycleBridgeResponse, LifecycleBridgeHandlerError> {
        let requested = std::str::from_utf8(request.command()).map_err(|_| {
            LifecycleBridgeHandlerError::new("boot runtime command identity is not UTF-8")
        })?;
        let command = self
            .targets
            .get(requested)
            .copied()
            .or_else(|| {
                (!requested.contains('/'))
                    .then(|| {
                        DECLARED_ENDPOINTS
                            .iter()
                            .find_map(|(command, _)| (*command == requested).then_some(*command))
                    })
                    .flatten()
            })
            .ok_or_else(|| {
                LifecycleBridgeHandlerError::new(format!(
                    "unsupported boot runtime command identity {requested:?}"
                ))
            })?;
        let argv = request
            .arguments()
            .iter()
            .map(|argument| {
                std::str::from_utf8(argument)
                    .map(str::to_string)
                    .map_err(|_| {
                        LifecycleBridgeHandlerError::new(format!(
                            "{command}: argv contains non-UTF-8 bytes"
                        ))
                    })
            })
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let invocation = parse_boot_runtime_invocation(command, &argv).map_err(|error| {
            LifecycleBridgeHandlerError::new(format!(
                "{command}: invocation is outside the pinned source grammar: {error}"
            ))
        })?;
        let disposition = invocation.disposition();
        self.invocations
            .lock()
            .map_err(|_| {
                LifecycleBridgeHandlerError::new("boot runtime invocation collector poisoned")
            })?
            .push(invocation);

        if disposition == InvocationDisposition::Mutation {
            return Ok(LifecycleBridgeResponse::new(Vec::new(), Vec::new(), 0));
        }
        let provider = self
            .providers
            .get(requested)
            .or_else(|| self.command_providers.get(command));
        let Some(provider) = provider else {
            return Ok(LifecycleBridgeResponse::new(
                Vec::new(),
                format!("{command}: selected-root provider is unavailable\n").into_bytes(),
                127,
            ));
        };
        self.delegate(provider, &argv)
    }

    fn delegate(
        &self,
        provider: &str,
        argv: &[String],
    ) -> std::result::Result<LifecycleBridgeResponse, LifecycleBridgeHandlerError> {
        let mut command = Command::new(provider);
        command
            .args(argv)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let environment = self
            .environment
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect::<Vec<_>>();
        apply_sanitized_command_env(&mut command, &environment);
        configure_target_command_boundary(&mut command, &self.root).map_err(|error| {
            LifecycleBridgeHandlerError::new(format!(
                "cannot establish selected-root boot provider boundary: {error}"
            ))
        })?;
        let mut child = command.spawn().map_err(|error| {
            LifecycleBridgeHandlerError::new(format!(
                "cannot spawn selected-root boot provider {provider}: {error}"
            ))
        })?;
        let output = wait_with_output(&mut child, self.timeout).map_err(|error| {
            LifecycleBridgeHandlerError::new(format!(
                "selected-root boot provider wait failed: {error}"
            ))
        })?;
        if output.timed_out {
            return Ok(LifecycleBridgeResponse::new(
                output.stdout,
                output.stderr,
                124,
            ));
        }
        let status = output.status.ok_or_else(|| {
            LifecycleBridgeHandlerError::new(
                "selected-root boot provider returned no terminal status",
            )
        })?;
        let exit_code = status.code().map_or_else(
            || {
                status
                    .signal()
                    .map_or(255, |signal| (128 + signal).clamp(0, 255) as u8)
            },
            |code| code.clamp(0, 255) as u8,
        );
        Ok(LifecycleBridgeResponse::new(
            output.stdout,
            output.stderr,
            exit_code,
        ))
    }
}

fn endpoint_contract(environment: &[(&str, &str)]) -> BTreeMap<PathBuf, &'static str> {
    let mut endpoints = DECLARED_ENDPOINTS
        .iter()
        .map(|(command, target)| (PathBuf::from(*target), *command))
        .collect::<BTreeMap<_, _>>();
    let effective_path = environment
        .iter()
        .rev()
        .find_map(|(key, value)| (*key == "PATH").then_some(*value))
        .unwrap_or(DEFAULT_SCRIPTLET_PATH);
    let directories = effective_path
        .split(':')
        .chain(DEFAULT_SCRIPTLET_PATH.split(':'))
        .filter(|directory| directory.starts_with('/'))
        .collect::<BTreeSet<_>>();
    for directory in directories {
        for &(command, _) in DECLARED_ENDPOINTS {
            endpoints.insert(Path::new(directory).join(command), command);
        }
    }
    endpoints
}

fn existing_provider(root: &Path, guest_target: &Path) -> Result<Option<Vec<u8>>> {
    let target = root.join(guest_target.strip_prefix("/").map_err(|_| {
        capture_contract_error(format!(
            "boot runtime endpoint must be absolute: {}",
            guest_target.display()
        ))
    })?);
    match fs::symlink_metadata(&target) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(capture_setup_error(error)),
    }
    let resolved = fs::canonicalize(&target).map_err(capture_setup_error)?;
    if !resolved.starts_with(root) {
        return Err(capture_contract_error(format!(
            "selected-root boot runtime endpoint {} resolves outside {}",
            target.display(),
            root.display()
        )));
    }
    let metadata = fs::metadata(&resolved).map_err(capture_setup_error)?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return Ok(None);
    }
    fs::read(&resolved).map(Some).map_err(capture_setup_error)
}

fn guest_path(root: &Path, host_path: &Path) -> Result<String> {
    let relative = host_path.strip_prefix(root).map_err(|_| {
        capture_contract_error(format!(
            "staged boot provider {} is outside selected root {}",
            host_path.display(),
            root.display()
        ))
    })?;
    Ok(format!("/{}", relative.display()))
}

fn capture_setup_error(error: impl std::fmt::Display) -> Error {
    Error::scriptlet(
        ScriptletFailureKind::ProcessSetupFailed,
        format!("cannot prepare boot runtime capture: {error}"),
    )
}

fn capture_contract_error(message: impl Into<String>) -> Error {
    Error::scriptlet(ScriptletFailureKind::ContractViolation, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boot_runtime::{BootRuntimeInvocation, DepmodAction};
    use crate::scriptlet::test_support::materialized_root;
    use crate::scriptlet::{PackageFormat, ScriptletExecutor};

    #[test]
    fn target_contract_includes_declared_paths_and_effective_path() {
        let endpoints = endpoint_contract(&[("PATH", "/custom/bin")]);
        assert_eq!(
            endpoints.get(Path::new("/usr/sbin/depmod")),
            Some(&"depmod")
        );
        assert_eq!(
            endpoints.get(Path::new("/custom/bin/bootctl")),
            Some(&"bootctl")
        );
    }

    #[test]
    fn staging_rejects_a_missing_target_below_a_symlink_escape() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("usr")).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("usr/sbin")).unwrap();
        let mut staged = TargetRootScript::stage(root.path(), b"").unwrap();
        let error = BootRuntimeCaptureSession::stage(
            &mut staged,
            root.path(),
            &[],
            Duration::from_secs(1),
            None,
        )
        .err()
        .expect("boot capture must reject a staging symlink escape")
        .to_string();
        assert!(error.contains("escapes selected root"), "{error}");
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    }

    #[test]
    fn missing_and_existing_mutation_helpers_are_intercepted_in_the_mount_namespace() {
        const TEST_NAME: &str = concat!(
            "scriptlet::boot_runtime_capture::tests::",
            "missing_and_existing_mutation_helpers_are_intercepted_in_the_mount_namespace"
        );
        let Some(missing_root) = materialized_root(TEST_NAME, &["/bin/sh"]) else {
            return;
        };
        let missing = executor(missing_root.path());
        run_script(&missing, b"/usr/sbin/depmod -a\n").unwrap();
        assert!(!missing_root.path().join("usr/sbin/depmod").exists());
        assert!(matches!(
            missing.take_boot_runtime_invocations().as_slice(),
            [BootRuntimeInvocation::Depmod(invocation)]
                if matches!(invocation.action, DepmodAction::Generate { .. })
        ));

        let existing_root =
            materialized_root(TEST_NAME, &["/bin/sh"]).expect("namespace child selected root");
        let helper = existing_root.path().join("usr/sbin/depmod");
        fs::create_dir_all(helper.parent().unwrap()).unwrap();
        let original = b"#!/bin/sh\ntouch /original-depmod-executed\nexit 99\n";
        fs::write(&helper, original).unwrap();
        set_executable(&helper);
        let existing = executor(existing_root.path());
        run_script(&existing, b"/usr/sbin/depmod -a\n").unwrap();
        assert!(
            !existing_root
                .path()
                .join("original-depmod-executed")
                .exists()
        );
        assert_eq!(fs::read(&helper).unwrap(), original);
        assert_eq!(existing.take_boot_runtime_invocations().len(), 1);
    }

    #[test]
    fn parser_proven_information_delegates_with_the_provider_status() {
        let Some(root) = materialized_root(
            concat!(
                "scriptlet::boot_runtime_capture::tests::",
                "parser_proven_information_delegates_with_the_provider_status"
            ),
            &["/bin/sh"],
        ) else {
            return;
        };
        let helper = root.path().join("usr/sbin/depmod");
        fs::create_dir_all(helper.parent().unwrap()).unwrap();
        fs::write(&helper, b"#!/bin/sh\necho provider-help\nexit 23\n").unwrap();
        set_executable(&helper);
        let executor = executor(root.path());
        run_script(
            &executor,
            b"set +e\noutput=$(/usr/sbin/depmod --help)\nstatus=$?\n\
              test \"$output\" = provider-help\n\
              test \"$status\" -eq 23\n",
        )
        .unwrap();
        assert_eq!(executor.take_boot_runtime_invocations().len(), 1);
    }

    #[test]
    fn exact_native_argv_uses_the_same_boot_capture_boundary() {
        let Some(root) = materialized_root(
            concat!(
                "scriptlet::boot_runtime_capture::tests::",
                "exact_native_argv_uses_the_same_boot_capture_boundary"
            ),
            &["/bin/sh"],
        ) else {
            return;
        };
        let helper = root.path().join("usr/sbin/depmod");
        fs::create_dir_all(helper.parent().unwrap()).unwrap();
        fs::write(
            &helper,
            b"#!/bin/sh\ntouch /native-original-executed\nexit 91\n",
        )
        .unwrap();
        set_executable(&helper);
        let executor = executor(root.path());
        executor
            .execute_native_command(
                "post-install",
                &["/usr/sbin/depmod".to_string(), "-a".to_string()],
                &[],
            )
            .unwrap();
        assert!(!root.path().join("native-original-executed").exists());
        assert_eq!(executor.take_boot_runtime_invocations().len(), 1);
    }

    fn executor(root: &Path) -> ScriptletExecutor {
        ScriptletExecutor::new(root, "boot-capture-fixture", "1", PackageFormat::Deb)
    }

    fn run_script(executor: &ScriptletExecutor, script: &[u8]) -> Result<()> {
        executor.execute_in_target(
            super::super::process::ScriptletProcess {
                phase: "post-install",
                interpreter: "/bin/sh",
                interpreter_args: &[],
                args: &[],
                env: &[],
                stdin: &[],
            },
            script,
        )
    }

    fn set_executable(path: &Path) {
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(path, permissions).unwrap();
    }
}
