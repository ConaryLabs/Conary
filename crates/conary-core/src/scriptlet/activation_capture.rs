// conary-core/src/scriptlet/activation_capture.rs

//! Exact observation of runtime-manager argv inside the selected-root boundary.
//!
//! A private mount namespace bind-mounts a small proxy over each target-root
//! provider executable. The proxy records the argv the lifecycle program
//! actually executed, delegates selected-root work through provider-documented
//! no-live flags, and converts live actions into successful deferred work. The
//! parent parses every successful capture with `crate::activation`'s closed
//! grammars before the package transaction may commit.

use super::process::{TargetCommandBindMount, TargetRootScript};
use crate::activation::{
    ActivationExecutableIdentity, RuntimeActivationInvocation, SecurityPolicyInvocationDisposition,
    parse_openrc_activation_invocation, parse_security_policy_invocation,
    parse_systemctl_activation_invocation, systemctl_proxy_shell_scan,
};
use crate::error::{Error, Result, ScriptletFailureKind};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const DEFAULT_SCRIPTLET_PATH: &str = "/usr/sbin:/usr/bin:/sbin:/bin";
const CAPTURE_COMMANDS: &[&str] = &[
    "systemctl",
    "rc-service",
    "restorecon",
    "semanage",
    "setsebool",
    "semodule",
    "apparmor_parser",
    "aa-enforce",
    "aa-complain",
    "aa-disable",
];

pub(super) struct ActivationCaptureSession {
    captures: Vec<CaptureTarget>,
    mounts: Vec<TargetCommandBindMount>,
}

struct CaptureTarget {
    capture_path: PathBuf,
    identities: Vec<ActivationExecutableIdentity>,
}

struct DiscoveredTarget {
    target: PathBuf,
    original: Vec<u8>,
    identities: BTreeMap<String, ActivationExecutableIdentity>,
}

impl ActivationCaptureSession {
    pub(super) fn stage(
        staged: &mut TargetRootScript,
        root: &Path,
        environment: &[(&str, &str)],
    ) -> Result<Self> {
        let targets = activation_targets(root, environment)?;
        let mut captures = Vec::with_capacity(targets.len());
        let mut mounts = Vec::with_capacity(targets.len());
        for (index, target) in targets.into_values().enumerate() {
            let capture_name = format!("activation.capture.{index}");
            let original_name = format!("activation.original.{index}");
            let proxy_name = format!("activation.proxy.{index}");
            let capture_path = staged.create_extra_file(&capture_name, &[], 0o600)?;
            let capture_guest = staged.guest_extra_path(&capture_name);
            staged.create_extra_file(&original_name, &target.original, 0o700)?;
            let original_guest = staged.guest_extra_path(&original_name);
            let proxy = proxy_script(&capture_guest, &original_guest);
            let proxy_path = staged.create_extra_file(&proxy_name, proxy.as_bytes(), 0o700)?;
            mounts.push(TargetCommandBindMount {
                source: proxy_path,
                target: target.target,
            });
            captures.push(CaptureTarget {
                capture_path,
                identities: target.identities.into_values().collect(),
            });
        }

        Ok(Self { captures, mounts })
    }

    pub(super) fn mounts(&self) -> &[TargetCommandBindMount] {
        &self.mounts
    }

    pub(super) fn finish(self) -> Result<Vec<RuntimeActivationInvocation>> {
        let mut invocations = Vec::new();
        for capture in self.captures {
            let bytes = fs::read(&capture.capture_path).map_err(|error| {
                Error::scriptlet(
                    ScriptletFailureKind::ProcessSetupFailed,
                    format!(
                        "failed to read selected-root activation capture {}: {error}",
                        capture.capture_path.display()
                    ),
                )
            })?;
            for record in decode_records(&bytes)? {
                if record.status != 0 {
                    continue;
                }
                let command = Path::new(&record.program)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| capture_error("captured program has no UTF-8 basename"))?;
                if command == "systemctl" {
                    match parse_systemctl_activation_invocation(&record.argv) {
                        Ok(Some(invocation)) => invocations.push(invocation.into()),
                        Ok(None) => {}
                        Err(error) => {
                            return Err(Error::scriptlet(
                                ScriptletFailureKind::ContractViolation,
                                format!(
                                    "captured systemctl invocation is outside the generation activation contract: {error}"
                                ),
                            ));
                        }
                    }
                    continue;
                }
                if command == "rc-service" {
                    match parse_openrc_activation_invocation(&record.argv) {
                        Ok(Some(invocation)) => invocations.push(invocation.into()),
                        Ok(None) => {}
                        Err(error) => {
                            return Err(Error::scriptlet(
                                ScriptletFailureKind::ContractViolation,
                                format!(
                                    "captured rc-service invocation is outside the generation activation contract: {error}"
                                ),
                            ));
                        }
                    }
                    continue;
                }
                let identity = captured_identity(&capture.identities, &record.program, command)?;
                match parse_security_policy_invocation(&record.program, &record.argv, identity) {
                    Ok(Some(SecurityPolicyInvocationDisposition::Generation(invocation))) => {
                        invocations.push(invocation.into());
                    }
                    Ok(Some(SecurityPolicyInvocationDisposition::SelectedRoot) | None) => {}
                    Err(error) => {
                        return Err(Error::scriptlet(
                            ScriptletFailureKind::ContractViolation,
                            format!(
                                "captured security-policy invocation is outside the selected-root/generation contract: {error}"
                            ),
                        ));
                    }
                }
            }
        }
        Ok(invocations)
    }
}

fn activation_targets(
    root: &Path,
    environment: &[(&str, &str)],
) -> Result<BTreeMap<PathBuf, DiscoveredTarget>> {
    let root = fs::canonicalize(root).map_err(|error| {
        Error::scriptlet(
            ScriptletFailureKind::ProcessSetupFailed,
            format!(
                "failed to canonicalize selected root {} for activation capture: {error}",
                root.display()
            ),
        )
    })?;
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
    let mut targets = BTreeMap::new();
    for directory in directories {
        for command in CAPTURE_COMMANDS {
            let candidate = root.join(directory.trim_start_matches('/')).join(command);
            let Ok(metadata) = candidate.metadata() else {
                continue;
            };
            if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
                continue;
            }
            let resolved = fs::canonicalize(&candidate).map_err(|error| {
                Error::scriptlet(
                    ScriptletFailureKind::ProcessSetupFailed,
                    format!(
                        "failed to resolve selected-root activation executable {}: {error}",
                        candidate.display()
                    ),
                )
            })?;
            if !resolved.starts_with(&root) {
                return Err(Error::scriptlet(
                    ScriptletFailureKind::ContractViolation,
                    format!(
                        "selected-root activation executable {} resolves outside {}",
                        candidate.display(),
                        root.display()
                    ),
                ));
            }
            let invoked_path = guest_path(&root, &candidate)?;
            let canonical_path = guest_path(&root, &resolved)?;
            let original = fs::read(&resolved).map_err(|error| {
                Error::scriptlet(
                    ScriptletFailureKind::ProcessSetupFailed,
                    format!(
                        "failed to stage target activation executable {}: {error}",
                        resolved.display()
                    ),
                )
            })?;
            let identity = ActivationExecutableIdentity {
                invoked_path: invoked_path.clone(),
                canonical_path,
                sha256: crate::hash::sha256_prefixed(&original),
            };
            let target = targets
                .entry(resolved.clone())
                .or_insert_with(|| DiscoveredTarget {
                    target: resolved,
                    original,
                    identities: BTreeMap::new(),
                });
            target.identities.insert(invoked_path, identity);
        }
    }
    Ok(targets)
}

fn proxy_script(capture: &Path, original: &Path) -> String {
    let capture = shell_quote(&capture.to_string_lossy());
    let original = shell_quote(&original.to_string_lossy());
    let systemctl_scan = systemctl_proxy_shell_scan();
    format!(
        "#!/bin/sh\n\
         umask 077\n\
         conary_record() {{\n\
             conary_status=\"$1\"\n\
             shift\n\
             printf '%s\\0' \"$0\" \"$conary_status\" \"$#\" \"$@\" >> {capture} || exit 125\n\
         }}\n\
         conary_program=${{0##*/}}\n\
         case \"$conary_program\" in\n\
             restorecon)\n\
                 {original} \"$@\"\n\
                 conary_status=$?\n\
                 conary_record \"$conary_status\" \"$@\"\n\
                 exit \"$conary_status\"\n\
                 ;;\n\
             semanage)\n\
                 case \"${{1-}}\" in\n\
                     fcontext)\n\
                         conary_record 0 \"$@\"\n\
                         shift\n\
                         {original} fcontext -N \"$@\"\n\
                         exit $?\n\
                         ;;\n\
                     *)\n\
                         conary_record 0 \"$@\"\n\
                         exit 0\n\
                         ;;\n\
                 esac\n\
                 ;;\n\
             semodule)\n\
                 {original} -N \"$@\"\n\
                 conary_status=$?\n\
                 conary_record \"$conary_status\" \"$@\"\n\
                 exit \"$conary_status\"\n\
                 ;;\n\
             setsebool)\n\
                 conary_record 0 \"$@\"\n\
                 exit 0\n\
                 ;;\n\
             apparmor_parser)\n\
                 {original} -Q \"$@\"\n\
                 conary_status=$?\n\
                 conary_record \"$conary_status\" \"$@\"\n\
                 exit \"$conary_status\"\n\
                 ;;\n\
             aa-enforce|aa-complain|aa-disable)\n\
                 {original} --no-reload \"$@\"\n\
                 conary_status=$?\n\
                 conary_record \"$conary_status\" \"$@\"\n\
                 exit \"$conary_status\"\n\
                 ;;\n\
             rc-service)\n\
                 {original} --dry-run \"$@\"\n\
                 conary_status=$?\n\
                 conary_record \"$conary_status\" \"$@\"\n\
                 exit \"$conary_status\"\n\
                 ;;\n\
             systemctl)\n\
                 conary_record 0 \"$@\"\n\
                 ;;\n\
             *)\n\
                 exit 125\n\
                 ;;\n\
         esac\n\
         {systemctl_scan}\
         SYSTEMD_OFFLINE=1 {original} \"$@\"\n"
    )
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CapturedCommandRecord {
    program: String,
    status: i32,
    argv: Vec<String>,
}

fn decode_records(bytes: &[u8]) -> Result<Vec<CapturedCommandRecord>> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    if bytes.last() != Some(&0) {
        return Err(capture_error(
            "activation capture ended without a NUL terminator",
        ));
    }
    let fields = bytes[..bytes.len() - 1].split(|byte| *byte == 0);
    let mut fields = fields.peekable();
    let mut records = Vec::new();
    while let Some(program) = fields.next() {
        let program = std::str::from_utf8(program)
            .map_err(|_| capture_error("activation capture program is not UTF-8"))?
            .to_string();
        let status = fields
            .next()
            .ok_or_else(|| capture_error("activation capture record has no status"))?;
        let status = std::str::from_utf8(status)
            .map_err(|_| capture_error("activation capture status is not UTF-8"))?
            .parse::<i32>()
            .map_err(|_| capture_error("activation capture status is not an integer"))?;
        let count = fields
            .next()
            .ok_or_else(|| capture_error("activation capture record has no argument count"))?;
        let count = std::str::from_utf8(count)
            .map_err(|_| capture_error("activation capture argument count is not UTF-8"))?
            .parse::<usize>()
            .map_err(|_| capture_error("activation capture argument count is not an integer"))?;
        let mut argv = Vec::with_capacity(count);
        for _ in 0..count {
            let value = fields
                .next()
                .ok_or_else(|| capture_error("activation capture record is truncated"))?;
            argv.push(
                std::str::from_utf8(value)
                    .map_err(|_| capture_error("activation capture argv is not UTF-8"))?
                    .to_string(),
            );
        }
        records.push(CapturedCommandRecord {
            program,
            status,
            argv,
        });
    }
    Ok(records)
}

fn guest_path(root: &Path, host_path: &Path) -> Result<String> {
    let relative = host_path.strip_prefix(root).map_err(|_| {
        capture_error(&format!(
            "activation executable {} is outside selected root {}",
            host_path.display(),
            root.display()
        ))
    })?;
    Ok(format!("/{}", relative.display()))
}

fn captured_identity(
    identities: &[ActivationExecutableIdentity],
    invoked_program: &str,
    command: &str,
) -> Result<ActivationExecutableIdentity> {
    if invoked_program.starts_with('/')
        && let Some(identity) = identities
            .iter()
            .find(|identity| identity.invoked_path == invoked_program)
    {
        return Ok(identity.clone());
    }
    let candidates = identities
        .iter()
        .filter(|identity| {
            Path::new(&identity.invoked_path)
                .file_name()
                .and_then(|name| name.to_str())
                == Some(command)
        })
        .collect::<Vec<_>>();
    match candidates.as_slice() {
        [identity] => Ok((*identity).clone()),
        [] => Err(capture_error(&format!(
            "captured provider program '{invoked_program}' has no staged executable identity"
        ))),
        _ => Err(capture_error(&format!(
            "captured provider program '{invoked_program}' has ambiguous staged executable identities"
        ))),
    }
}

fn capture_error(message: &str) -> Error {
    Error::scriptlet(ScriptletFailureKind::ContractViolation, message.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activation::SystemdActivationAction;

    #[test]
    fn nul_capture_preserves_empty_arguments_and_record_boundaries() {
        let bytes = b"/usr/bin/systemctl\x000\x002\0restart\0demo.service\0/usr/sbin/setsebool\x000\x003\0-P\0\0worker_enabled=on\0";
        assert_eq!(
            decode_records(bytes).unwrap(),
            vec![
                CapturedCommandRecord {
                    program: "/usr/bin/systemctl".to_string(),
                    status: 0,
                    argv: vec!["restart".to_string(), "demo.service".to_string()],
                },
                CapturedCommandRecord {
                    program: "/usr/sbin/setsebool".to_string(),
                    status: 0,
                    argv: vec![
                        "-P".to_string(),
                        String::new(),
                        "worker_enabled=on".to_string(),
                    ],
                },
            ]
        );
    }

    #[test]
    fn malformed_capture_fails_closed() {
        assert!(decode_records(b"/usr/bin/systemctl\x000\x001\0restart").is_err());
        assert!(decode_records(b"/usr/bin/systemctl\x000\0two\0restart\0demo.service\0").is_err());
        assert!(decode_records(b"/usr/bin/systemctl\x000\x002\0restart\0").is_err());
    }

    #[test]
    fn captured_runtime_argv_uses_the_typed_parser() {
        let bytes = b"/usr/bin/systemctl\x000\x003\0--system\0restart\0demo.service\0";
        let records = decode_records(bytes).unwrap();
        let intent = parse_systemctl_activation_invocation(&records[0].argv)
            .unwrap()
            .unwrap();
        assert_eq!(intent.action, SystemdActivationAction::Restart);
        assert_eq!(intent.units, vec!["demo.service"]);
    }

    #[test]
    fn proxy_delegates_offline_work_and_suppresses_only_closed_runtime_verbs() {
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;

        let temp = tempfile::tempdir().unwrap();
        let capture = temp.path().join("capture");
        let calls = temp.path().join("calls");
        let original = temp.path().join("systemctl.original");
        fs::write(
            &original,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 17\n",
                calls.display()
            ),
        )
        .unwrap();
        let proxy_path = temp.path().join("systemctl");
        let proxy = proxy_script(&capture, &original);
        fs::write(&proxy_path, &proxy).unwrap();
        for path in [&original, &proxy_path] {
            let mut permissions = fs::metadata(path).unwrap().permissions();
            permissions.set_mode(0o700);
            fs::set_permissions(path, permissions).unwrap();
        }

        let runtime = Command::new("/bin/sh")
            .arg(&proxy_path)
            .args(["restart", "demo.service"])
            .status()
            .unwrap();
        assert!(runtime.success());
        assert!(!calls.exists(), "runtime verb reached target systemctl");

        let ambiguous = Command::new("/bin/sh")
            .arg(&proxy_path)
            .args(["--future-option", "restart", "demo.service"])
            .status()
            .unwrap();
        assert!(ambiguous.success());
        assert!(
            !calls.exists(),
            "ambiguous pre-verb option reached target systemctl"
        );
        assert!(matches!(
            parse_systemctl_activation_invocation(&[
                "--future-option".to_string(),
                "restart".to_string(),
                "demo.service".to_string(),
            ]),
            Err(crate::activation::SystemdActivationParseError::UnsupportedOption { .. })
        ));

        let offline = Command::new("/bin/sh")
            .arg(&proxy_path)
            .arg("daemon-reload")
            .status()
            .unwrap();
        assert_eq!(offline.code(), Some(17));
        assert_eq!(fs::read_to_string(&calls).unwrap(), "daemon-reload\n");
        assert_eq!(
            decode_records(&fs::read(&capture).unwrap()).unwrap(),
            vec![
                CapturedCommandRecord {
                    program: proxy_path.display().to_string(),
                    status: 0,
                    argv: vec!["restart".to_string(), "demo.service".to_string()],
                },
                CapturedCommandRecord {
                    program: proxy_path.display().to_string(),
                    status: 0,
                    argv: vec![
                        "--future-option".to_string(),
                        "restart".to_string(),
                        "demo.service".to_string(),
                    ],
                },
                CapturedCommandRecord {
                    program: proxy_path.display().to_string(),
                    status: 0,
                    argv: vec!["daemon-reload".to_string()],
                },
            ]
        );
        assert!(proxy.contains("reload-or-try-restart"));
        assert!(!proxy.contains("grep"));
        assert!(!proxy.contains("sed"));
    }

    #[test]
    fn openrc_proxy_uses_target_dry_run_and_captures_exact_runtime_argv() {
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;

        let temp = tempfile::tempdir().unwrap();
        let capture = temp.path().join("capture");
        let calls = temp.path().join("calls");
        let original = temp.path().join("rc-service.original");
        fs::write(
            &original,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 0\n",
                calls.display()
            ),
        )
        .unwrap();
        let proxy_path = temp.path().join("rc-service");
        fs::write(&proxy_path, proxy_script(&capture, &original)).unwrap();
        for path in [&original, &proxy_path] {
            let mut permissions = fs::metadata(path).unwrap().permissions();
            permissions.set_mode(0o700);
            fs::set_permissions(path, permissions).unwrap();
        }

        assert!(
            Command::new("/bin/sh")
                .arg(&proxy_path)
                .args(["--ifstarted", "sshd", "restart"])
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(
            fs::read_to_string(calls).unwrap(),
            "--dry-run --ifstarted sshd restart\n"
        );
        let record = decode_records(&fs::read(capture).unwrap())
            .unwrap()
            .remove(0);
        assert_eq!(
            record.argv,
            ["--ifstarted", "sshd", "restart"].map(str::to_string)
        );
        assert!(
            parse_openrc_activation_invocation(&record.argv)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn provider_proxy_runs_only_documented_selected_root_half() {
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;

        let temp = tempfile::tempdir().unwrap();
        let calls = temp.path().join("calls");
        let original = temp.path().join("provider.original");
        fs::write(
            &original,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 0\n",
                calls.display()
            ),
        )
        .unwrap();
        let mut permissions = fs::metadata(&original).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&original, permissions).unwrap();

        let semodule_capture = temp.path().join("semodule.capture");
        let semodule = temp.path().join("semodule");
        fs::write(&semodule, proxy_script(&semodule_capture, &original)).unwrap();
        let mut permissions = fs::metadata(&semodule).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&semodule, permissions).unwrap();
        assert!(
            Command::new("/bin/sh")
                .arg(&semodule)
                .args(["-i", "/usr/share/selinux/demo.pp"])
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(
            fs::read_to_string(&calls).unwrap(),
            "-N -i /usr/share/selinux/demo.pp\n"
        );
        assert_eq!(
            decode_records(&fs::read(&semodule_capture).unwrap()).unwrap()[0].argv,
            vec!["-i", "/usr/share/selinux/demo.pp"]
        );

        let setsebool_capture = temp.path().join("setsebool.capture");
        let setsebool = temp.path().join("setsebool");
        fs::write(&setsebool, proxy_script(&setsebool_capture, &original)).unwrap();
        let mut permissions = fs::metadata(&setsebool).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&setsebool, permissions).unwrap();
        assert!(
            Command::new("/bin/sh")
                .arg(&setsebool)
                .args(["-P", "demo_enabled", "on"])
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(
            fs::read_to_string(&calls).unwrap(),
            "-N -i /usr/share/selinux/demo.pp\n",
            "setsebool reached the live provider binary"
        );
        assert_eq!(
            decode_records(&fs::read(&setsebool_capture).unwrap()).unwrap()[0].argv,
            vec!["-P", "demo_enabled", "on"]
        );
    }
}
