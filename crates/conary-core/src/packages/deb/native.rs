// conary-core/src/packages/deb/native.rs

use super::*;

impl DebPackage {
    pub(super) fn native_abi_from_control_member(
        name: &str,
        body: &[u8],
    ) -> crate::error::Result<Option<NativeScriptletEntry>> {
        let (control_member, lifecycle, stdin) = match name {
            "config" => (
                DebControlMember::Config,
                NativeLifecyclePath::Config,
                NativeStdinContract::Debconf,
            ),
            "preinst" => (
                DebControlMember::Preinst,
                NativeLifecyclePath::PreInstall,
                NativeStdinContract::None,
            ),
            "postinst" => (
                DebControlMember::Postinst,
                NativeLifecyclePath::PostInstall,
                NativeStdinContract::None,
            ),
            "prerm" => (
                DebControlMember::Prerm,
                NativeLifecyclePath::PreRemove,
                NativeStdinContract::None,
            ),
            "postrm" => (
                DebControlMember::Postrm,
                NativeLifecyclePath::PostRemove,
                NativeStdinContract::None,
            ),
            _ => return Ok(None),
        };

        if body.iter().all(|byte| byte.is_ascii_whitespace()) {
            return Ok(None);
        }

        let (interpreter, interpreter_args) = parse_deb_shebang(name, body)?;
        let order_position = match lifecycle {
            NativeLifecyclePath::PreInstall | NativeLifecyclePath::PreRemove => {
                NativeTransactionPosition::BeforePayload
            }
            NativeLifecyclePath::Config => NativeTransactionPosition::ControlArtifact,
            _ => NativeTransactionPosition::AfterPayload,
        };

        Ok(Some(NativeScriptletEntry {
            id: format!("deb:{name}"),
            format: NativeScriptletFormat::Deb,
            kind: NativeScriptletKind::Executable,
            native_slot: name.to_string(),
            primary_lifecycle: lifecycle,
            lifecycle_paths: Self::deb_lifecycle_paths(control_member),
            interpreter: Some(interpreter),
            interpreter_args,
            body: NativeScriptletBody::from_bytes(body.to_vec()),
            invocation: NativeInvocationContract {
                args: Vec::new(),
                environment: Vec::new(),
                stdin,
                root: NativeRootExpectation::PackageManagerDefault,
            },
            order: NativeTransactionOrder::new(order_position),
            support: NativeScriptletSupport::Parsed,
            metadata: NativeScriptletMetadata::Deb(DebNativeScriptletMetadata {
                control_member,
                maintainer_modes: Self::deb_maintainer_invocations(control_member),
                trigger_declarations: Vec::new(),
            }),
        }))
    }

    fn deb_lifecycle_paths(control_member: DebControlMember) -> Vec<NativeLifecyclePath> {
        match control_member {
            DebControlMember::Config => vec![NativeLifecyclePath::Config],
            DebControlMember::Preinst => vec![
                NativeLifecyclePath::PreInstall,
                NativeLifecyclePath::PreUpgrade,
                NativeLifecyclePath::Abort,
            ],
            DebControlMember::Postinst => vec![
                NativeLifecyclePath::PostInstall,
                NativeLifecyclePath::PostUpgrade,
                NativeLifecyclePath::Trigger,
                NativeLifecyclePath::Abort,
            ],
            DebControlMember::Prerm => vec![
                NativeLifecyclePath::PreRemove,
                NativeLifecyclePath::PreUpgrade,
                NativeLifecyclePath::Abort,
            ],
            DebControlMember::Postrm => vec![
                NativeLifecyclePath::PostRemove,
                NativeLifecyclePath::PostUpgrade,
                NativeLifecyclePath::Purge,
                NativeLifecyclePath::Abort,
            ],
            DebControlMember::Triggers => vec![NativeLifecyclePath::Trigger],
        }
    }

    fn deb_action_arg() -> NativeArgumentContract {
        NativeArgumentContract {
            index: 1,
            name: "action".to_string(),
            value: NativeArgumentValue::Action,
            required: true,
        }
    }

    fn deb_arg(
        index: usize,
        name: &str,
        value: NativeArgumentValue,
        required: bool,
    ) -> NativeArgumentContract {
        NativeArgumentContract {
            index,
            name: name.to_string(),
            value,
            required,
        }
    }

    fn deb_old_new_args(required: bool) -> Vec<NativeArgumentContract> {
        vec![
            Self::deb_arg(2, "old-version", NativeArgumentValue::OldVersion, required),
            Self::deb_arg(3, "new-version", NativeArgumentValue::NewVersion, required),
        ]
    }

    fn deb_new_version_arg(index: usize, required: bool) -> NativeArgumentContract {
        Self::deb_arg(
            index,
            "new-version",
            NativeArgumentValue::NewVersion,
            required,
        )
    }

    fn deb_installed_version_arg(index: usize, required: bool) -> NativeArgumentContract {
        Self::deb_arg(
            index,
            "installed-version",
            NativeArgumentValue::InstalledVersion,
            required,
        )
    }

    fn deb_marker_arg(index: usize, marker: &str, required: bool) -> NativeArgumentContract {
        Self::deb_arg(
            index,
            marker,
            NativeArgumentValue::Raw(marker.to_string()),
            required,
        )
    }

    fn deb_package_arg(index: usize, name: &str, required: bool) -> NativeArgumentContract {
        Self::deb_arg(index, name, NativeArgumentValue::PackageName, required)
    }

    fn deb_version_arg(
        index: usize,
        name: &str,
        value: NativeArgumentValue,
        required: bool,
    ) -> NativeArgumentContract {
        Self::deb_arg(index, name, value, required)
    }

    fn deb_invocation(
        mode: DebMaintainerMode,
        mut args: Vec<NativeArgumentContract>,
        lifecycle_paths: Vec<NativeLifecyclePath>,
    ) -> DebMaintainerInvocation {
        let mut full_args = vec![Self::deb_action_arg()];
        full_args.append(&mut args);
        DebMaintainerInvocation {
            mode,
            args: full_args,
            lifecycle_paths,
        }
    }

    fn deb_maintainer_invocations(
        control_member: DebControlMember,
    ) -> Vec<DebMaintainerInvocation> {
        match control_member {
            DebControlMember::Config => vec![
                Self::deb_invocation(
                    DebMaintainerMode::Configure,
                    vec![Self::deb_installed_version_arg(2, false)],
                    vec![NativeLifecyclePath::Config],
                ),
                Self::deb_invocation(
                    DebMaintainerMode::Reconfigure,
                    vec![Self::deb_installed_version_arg(2, false)],
                    vec![NativeLifecyclePath::Config],
                ),
            ],
            DebControlMember::Preinst => vec![
                Self::deb_invocation(
                    DebMaintainerMode::Install,
                    Self::deb_old_new_args(false),
                    vec![NativeLifecyclePath::PreInstall],
                ),
                Self::deb_invocation(
                    DebMaintainerMode::Upgrade,
                    Self::deb_old_new_args(true),
                    vec![NativeLifecyclePath::PreUpgrade],
                ),
                Self::deb_invocation(
                    DebMaintainerMode::AbortUpgrade,
                    vec![Self::deb_new_version_arg(2, true)],
                    vec![NativeLifecyclePath::Abort],
                ),
            ],
            DebControlMember::Postinst => vec![
                Self::deb_invocation(
                    DebMaintainerMode::Configure,
                    vec![Self::deb_arg(
                        2,
                        "most-recently-configured-version",
                        NativeArgumentValue::MostRecentlyConfiguredVersion,
                        true,
                    )],
                    vec![
                        NativeLifecyclePath::PostInstall,
                        NativeLifecyclePath::PostUpgrade,
                    ],
                ),
                Self::deb_invocation(
                    DebMaintainerMode::Triggered,
                    vec![Self::deb_arg(
                        2,
                        "trigger-names",
                        NativeArgumentValue::TriggerNames,
                        true,
                    )],
                    vec![NativeLifecyclePath::Trigger],
                ),
                Self::deb_invocation(
                    DebMaintainerMode::AbortUpgrade,
                    vec![Self::deb_new_version_arg(2, true)],
                    vec![NativeLifecyclePath::Abort],
                ),
                Self::deb_invocation(
                    DebMaintainerMode::AbortRemove,
                    Vec::new(),
                    vec![NativeLifecyclePath::Abort],
                ),
                Self::deb_invocation(
                    DebMaintainerMode::AbortRemove,
                    vec![
                        Self::deb_marker_arg(2, "in-favour", true),
                        Self::deb_package_arg(3, "package", true),
                        Self::deb_new_version_arg(4, true),
                    ],
                    vec![NativeLifecyclePath::Abort],
                ),
                Self::deb_invocation(
                    DebMaintainerMode::AbortDeconfigure,
                    vec![
                        Self::deb_marker_arg(2, "in-favour", true),
                        Self::deb_arg(
                            3,
                            "failed-install-package",
                            NativeArgumentValue::InstallingPackageName,
                            true,
                        ),
                        Self::deb_version_arg(
                            4,
                            "failed-install-version",
                            NativeArgumentValue::InstallingPackageVersion,
                            true,
                        ),
                        Self::deb_arg(
                            5,
                            "removing",
                            NativeArgumentValue::ConflictingPackageMarker,
                            false,
                        ),
                        Self::deb_arg(
                            6,
                            "conflicting-package",
                            NativeArgumentValue::ConflictingPackageName,
                            false,
                        ),
                        Self::deb_version_arg(
                            7,
                            "conflicting-version",
                            NativeArgumentValue::ConflictingPackageVersion,
                            false,
                        ),
                    ],
                    vec![NativeLifecyclePath::Abort],
                ),
            ],
            DebControlMember::Prerm => vec![
                Self::deb_invocation(
                    DebMaintainerMode::Remove,
                    Vec::new(),
                    vec![NativeLifecyclePath::PreRemove],
                ),
                Self::deb_invocation(
                    DebMaintainerMode::Remove,
                    vec![
                        Self::deb_marker_arg(2, "in-favour", true),
                        Self::deb_package_arg(3, "package", true),
                        Self::deb_new_version_arg(4, true),
                    ],
                    vec![NativeLifecyclePath::PreRemove],
                ),
                Self::deb_invocation(
                    DebMaintainerMode::Upgrade,
                    vec![Self::deb_new_version_arg(2, true)],
                    vec![NativeLifecyclePath::PreUpgrade],
                ),
                Self::deb_invocation(
                    DebMaintainerMode::Deconfigure,
                    vec![
                        Self::deb_marker_arg(2, "in-favour", true),
                        Self::deb_arg(
                            3,
                            "package-being-installed",
                            NativeArgumentValue::InstallingPackageName,
                            true,
                        ),
                        Self::deb_version_arg(
                            4,
                            "package-being-installed-version",
                            NativeArgumentValue::InstallingPackageVersion,
                            true,
                        ),
                        Self::deb_arg(
                            5,
                            "removing",
                            NativeArgumentValue::ConflictingPackageMarker,
                            false,
                        ),
                        Self::deb_arg(
                            6,
                            "conflicting-package",
                            NativeArgumentValue::ConflictingPackageName,
                            false,
                        ),
                        Self::deb_version_arg(
                            7,
                            "conflicting-version",
                            NativeArgumentValue::ConflictingPackageVersion,
                            false,
                        ),
                    ],
                    vec![NativeLifecyclePath::Abort],
                ),
                Self::deb_invocation(
                    DebMaintainerMode::FailedUpgrade,
                    Self::deb_old_new_args(true),
                    vec![NativeLifecyclePath::Abort],
                ),
            ],
            DebControlMember::Postrm => vec![
                Self::deb_invocation(
                    DebMaintainerMode::Remove,
                    Vec::new(),
                    vec![NativeLifecyclePath::PostRemove],
                ),
                Self::deb_invocation(
                    DebMaintainerMode::Purge,
                    Vec::new(),
                    vec![NativeLifecyclePath::Purge],
                ),
                Self::deb_invocation(
                    DebMaintainerMode::Upgrade,
                    vec![Self::deb_new_version_arg(2, true)],
                    vec![NativeLifecyclePath::PostUpgrade],
                ),
                Self::deb_invocation(
                    DebMaintainerMode::Disappear,
                    vec![
                        Self::deb_arg(
                            2,
                            "overwriter-package",
                            NativeArgumentValue::PackageName,
                            true,
                        ),
                        Self::deb_arg(
                            3,
                            "overwriter-version",
                            NativeArgumentValue::NewVersion,
                            true,
                        ),
                    ],
                    vec![NativeLifecyclePath::PostRemove],
                ),
                Self::deb_invocation(
                    DebMaintainerMode::FailedUpgrade,
                    Self::deb_old_new_args(true),
                    vec![NativeLifecyclePath::Abort],
                ),
                Self::deb_invocation(
                    DebMaintainerMode::AbortInstall,
                    Self::deb_old_new_args(false),
                    vec![NativeLifecyclePath::Abort],
                ),
                Self::deb_invocation(
                    DebMaintainerMode::AbortUpgrade,
                    Self::deb_old_new_args(true),
                    vec![NativeLifecyclePath::Abort],
                ),
            ],
            DebControlMember::Triggers => Vec::new(),
        }
    }

    pub(super) fn native_abi_from_triggers_file(
        body: &[u8],
    ) -> crate::error::Result<NativeScriptletEntry> {
        let declarations = super::triggers::parse(body)?;

        Ok(NativeScriptletEntry {
            id: "deb:triggers".to_string(),
            format: NativeScriptletFormat::Deb,
            kind: NativeScriptletKind::ControlArtifact,
            native_slot: "triggers".to_string(),
            primary_lifecycle: NativeLifecyclePath::Trigger,
            lifecycle_paths: vec![NativeLifecyclePath::Trigger],
            interpreter: None,
            interpreter_args: Vec::new(),
            body: NativeScriptletBody::from_bytes(body.to_vec()),
            invocation: NativeInvocationContract {
                args: Vec::new(),
                environment: Vec::new(),
                stdin: NativeStdinContract::None,
                root: NativeRootExpectation::PackageManagerDefault,
            },
            order: NativeTransactionOrder::new(NativeTransactionPosition::ControlArtifact),
            support: NativeScriptletSupport::Parsed,
            metadata: NativeScriptletMetadata::Deb(DebNativeScriptletMetadata {
                control_member: DebControlMember::Triggers,
                maintainer_modes: Vec::new(),
                trigger_declarations: declarations,
            }),
        })
    }
}

fn parse_deb_shebang(
    control_member: &str,
    body: &[u8],
) -> crate::error::Result<(String, Vec<String>)> {
    let first_line = body.split(|byte| *byte == b'\n').next().unwrap_or(body);
    let shebang = first_line.strip_prefix(b"#!").ok_or_else(|| {
        crate::error::Error::InitError(format!(
            "Invalid DEBIAN/{control_member}: maintainer script has no shebang"
        ))
    })?;
    if shebang.contains(&b'\0') || shebang.ends_with(b"\r") {
        return Err(crate::error::Error::InitError(format!(
            "Invalid DEBIAN/{control_member}: maintainer script has a malformed shebang"
        )));
    }

    let shebang = trim_ascii_space(shebang);
    let interpreter_end = shebang
        .iter()
        .position(|byte| matches!(byte, b' ' | b'\t'))
        .unwrap_or(shebang.len());
    let interpreter_bytes = &shebang[..interpreter_end];
    let optional_arg_bytes = trim_ascii_space(&shebang[interpreter_end..]);
    let interpreter = std::str::from_utf8(interpreter_bytes).map_err(|error| {
        crate::error::Error::InitError(format!(
            "Invalid DEBIAN/{control_member}: maintainer interpreter is not UTF-8: {error}"
        ))
    })?;
    if !interpreter.starts_with('/') {
        return Err(crate::error::Error::InitError(format!(
            "Invalid DEBIAN/{control_member}: maintainer interpreter '{interpreter}' is not absolute"
        )));
    }
    let interpreter_args = if optional_arg_bytes.is_empty() {
        Vec::new()
    } else {
        vec![
            std::str::from_utf8(optional_arg_bytes)
                .map_err(|error| {
                    crate::error::Error::InitError(format!(
                        "Invalid DEBIAN/{control_member}: shebang optional argument is not UTF-8: {error}"
                    ))
                })?
                .to_string(),
        ]
    };

    Ok((interpreter.to_string(), interpreter_args))
}

fn trim_ascii_space(mut bytes: &[u8]) -> &[u8] {
    while bytes
        .first()
        .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
    {
        bytes = &bytes[1..];
    }
    while bytes
        .last()
        .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
    {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}
