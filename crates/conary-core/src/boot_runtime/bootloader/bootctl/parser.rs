// crates/conary-core/src/boot_runtime/bootloader/bootctl/parser.rs

use super::types::{
    BOOTCTL_CONTRACT, BootctlAction, BootctlCertificateSource, BootctlEntryTarget,
    BootctlEntryToken, BootctlInstallSource, BootctlInvocation, BootctlJsonMode, BootctlKeySource,
    BootctlOption, BootctlPrintTarget, BootctlTimeout, BootctlTriState,
};
use crate::boot_runtime::BootRuntimeParseError;
use crate::boot_runtime::argv::{
    OptionSpec, only_operand_count, parse_boolean, require_nonempty, scan_gnu,
};

const OPTIONS: &[OptionSpec] = &[
    OptionSpec::flag("print-esp-path", Some('p'), "print-esp-path"),
    OptionSpec::flag("print-esp-path", None, "print-path"),
    OptionSpec::flag("print-boot-path", Some('x'), "print-boot-path"),
    OptionSpec::flag("print-loader-path", None, "print-loader-path"),
    OptionSpec::flag("print-stub-path", None, "print-stub-path"),
    OptionSpec::flag("print-root-device", Some('R'), "print-root-device"),
    OptionSpec::flag("print-efi-architecture", None, "print-efi-architecture"),
    OptionSpec::flag("help", Some('h'), "help"),
    OptionSpec::flag("version", None, "version"),
    OptionSpec::value("esp-path", None, "esp-path"),
    OptionSpec::value("esp-path", None, "path"),
    OptionSpec::value("boot-path", None, "boot-path"),
    OptionSpec::value("root", None, "root"),
    OptionSpec::value("image", None, "image"),
    OptionSpec::value("image-policy", None, "image-policy"),
    OptionSpec::value("install-source", None, "install-source"),
    OptionSpec::value("variables", None, "variables"),
    OptionSpec::flag("no-variables", None, "no-variables"),
    OptionSpec::value("random-seed", None, "random-seed"),
    OptionSpec::flag("no-pager", None, "no-pager"),
    OptionSpec::flag("graceful", None, "graceful"),
    OptionSpec::flag("quiet", Some('q'), "quiet"),
    OptionSpec::value("entry-token", None, "entry-token"),
    OptionSpec::value("make-entry-directory", None, "make-entry-directory"),
    OptionSpec::value("make-entry-directory", None, "make-machine-id-directory"),
    OptionSpec::value("json", None, "json"),
    OptionSpec::flag("all-architectures", None, "all-architectures"),
    OptionSpec::value("efi-description", None, "efi-boot-option-description"),
    OptionSpec::value(
        "efi-description-with-device",
        None,
        "efi-boot-option-description-with-device",
    ),
    OptionSpec::flag("dry-run", None, "dry-run"),
    OptionSpec::value("secure-boot-auto-enroll", None, "secure-boot-auto-enroll"),
    OptionSpec::value("private-key", None, "private-key"),
    OptionSpec::value("private-key-source", None, "private-key-source"),
    OptionSpec::value("certificate", None, "certificate"),
    OptionSpec::value("certificate-source", None, "certificate-source"),
    OptionSpec::value("oldest", None, "oldest"),
    OptionSpec::value("keep-free", None, "keep-free"),
    OptionSpec::value("entry-title", None, "entry-title"),
    OptionSpec::value("entry-version", None, "entry-version"),
    OptionSpec::value("entry-commit", None, "entry-commit"),
    OptionSpec::value("extra", Some('X'), "extra"),
    OptionSpec::value("tries-left", None, "tries-left"),
];

#[derive(Default)]
struct ParseState {
    options: Vec<BootctlOption>,
    information: Option<BootctlAction>,
    print_target: Option<BootctlPrintTarget>,
    root_print_count: usize,
    has_root: bool,
    has_image: bool,
    has_install_source: bool,
    dry_run: bool,
    oldest: bool,
    secure_boot_auto_enroll: bool,
    has_private_key: bool,
    has_certificate: bool,
}

pub fn parse_bootctl(argv: &[&str]) -> Result<BootctlInvocation, BootRuntimeParseError> {
    const COMMAND: &str = "bootctl";
    let scanned = scan_gnu(COMMAND, argv, OPTIONS)?;
    let mut state = ParseState::default();

    for option in scanned.options {
        let value = || option.values[0].clone();
        match option.id {
            "print-esp-path" => {
                set_print_target(&mut state, BootctlPrintTarget::EspPath)?;
                state
                    .options
                    .push(BootctlOption::Print(BootctlPrintTarget::EspPath));
            }
            "print-boot-path" => {
                set_print_target(&mut state, BootctlPrintTarget::BootPath)?;
                state
                    .options
                    .push(BootctlOption::Print(BootctlPrintTarget::BootPath));
            }
            "print-loader-path" => {
                set_print_target(&mut state, BootctlPrintTarget::LoaderPath)?;
                state
                    .options
                    .push(BootctlOption::Print(BootctlPrintTarget::LoaderPath));
            }
            "print-stub-path" => {
                set_print_target(&mut state, BootctlPrintTarget::StubPath)?;
                state
                    .options
                    .push(BootctlOption::Print(BootctlPrintTarget::StubPath));
            }
            "print-root-device" => {
                state.root_print_count += 1;
                let target = if state.root_print_count == 1 {
                    BootctlPrintTarget::RootDevice
                } else {
                    BootctlPrintTarget::RootDisk
                };
                set_print_target(&mut state, target)?;
                state.options.push(BootctlOption::Print(target));
            }
            "print-efi-architecture" => {
                set_print_target(&mut state, BootctlPrintTarget::EfiArchitecture)?;
                state
                    .options
                    .push(BootctlOption::Print(BootctlPrintTarget::EfiArchitecture));
            }
            "help" => {
                state.information.get_or_insert(BootctlAction::Help);
            }
            "version" => {
                state.information.get_or_insert(BootctlAction::Version);
            }
            "esp-path" => state.options.push(BootctlOption::EspPath(value())),
            "boot-path" => state.options.push(BootctlOption::BootPath(value())),
            "root" => {
                let path = value();
                state.has_root = !path.is_empty() && path != "/";
                state.options.push(BootctlOption::Root(path));
            }
            "image" => {
                let path = value();
                state.has_image = !path.is_empty();
                state.options.push(BootctlOption::Image(path));
            }
            "image-policy" => state
                .options
                .push(BootctlOption::ImagePolicy(require_nonempty(
                    COMMAND,
                    &option.spelling,
                    value(),
                    "a systemd image policy",
                )?)),
            "install-source" => {
                let source = match value().as_str() {
                    "auto" => BootctlInstallSource::Auto,
                    "image" => BootctlInstallSource::Image,
                    "host" => BootctlInstallSource::Host,
                    invalid => {
                        return Err(invalid_value(
                            &option.spelling,
                            invalid,
                            "auto, image, or host",
                        ));
                    }
                };
                state.has_install_source = source != BootctlInstallSource::Auto;
                state.options.push(BootctlOption::InstallSource(source));
            }
            "variables" => state.options.push(BootctlOption::Variables(parse_tristate(
                &option.spelling,
                value(),
            )?)),
            "no-variables" => state
                .options
                .push(BootctlOption::Variables(BootctlTriState::Boolean(false))),
            "random-seed" => state.options.push(BootctlOption::RandomSeed(parse_boolean(
                COMMAND,
                &option.spelling,
                value(),
            )?)),
            "no-pager" => state.options.push(BootctlOption::NoPager),
            "graceful" => state.options.push(BootctlOption::Graceful),
            "quiet" => state.options.push(BootctlOption::Quiet),
            "entry-token" => state
                .options
                .push(BootctlOption::EntryToken(parse_entry_token(
                    &option.spelling,
                    value(),
                )?)),
            "make-entry-directory" => state.options.push(BootctlOption::MakeEntryDirectory(
                parse_make_entry_directory(&option.spelling, value())?,
            )),
            "json" => match value().as_str() {
                "pretty" => state
                    .options
                    .push(BootctlOption::Json(BootctlJsonMode::Pretty)),
                "short" => state
                    .options
                    .push(BootctlOption::Json(BootctlJsonMode::Short)),
                "off" => state
                    .options
                    .push(BootctlOption::Json(BootctlJsonMode::Off)),
                "help" => {
                    state.information.get_or_insert(BootctlAction::JsonHelp);
                }
                invalid => {
                    return Err(invalid_value(
                        &option.spelling,
                        invalid,
                        "pretty, short, off, or help",
                    ));
                }
            },
            "all-architectures" => state.options.push(BootctlOption::AllArchitectures),
            "efi-description" => {
                let description = value();
                if description.len() > 255 || description.chars().any(char::is_control) {
                    return Err(invalid_value(
                        &option.spelling,
                        &description,
                        "at most 255 bytes without control characters",
                    ));
                }
                state
                    .options
                    .push(BootctlOption::EfiBootOptionDescription(description));
            }
            "efi-description-with-device" => {
                state
                    .options
                    .push(BootctlOption::EfiBootOptionDescriptionWithDevice(
                        parse_boolean(COMMAND, &option.spelling, value())?,
                    ))
            }
            "dry-run" => {
                state.dry_run = true;
                state.options.push(BootctlOption::DryRun);
            }
            "secure-boot-auto-enroll" => {
                let enabled = parse_boolean(COMMAND, &option.spelling, value())?;
                state.secure_boot_auto_enroll = enabled;
                state
                    .options
                    .push(BootctlOption::SecureBootAutoEnroll(enabled));
            }
            "private-key" => {
                state.has_private_key = true;
                state.options.push(BootctlOption::PrivateKey(value()));
            }
            "private-key-source" => {
                state
                    .options
                    .push(BootctlOption::PrivateKeySource(parse_key_source(
                        &option.spelling,
                        value(),
                    )?))
            }
            "certificate" => {
                state.has_certificate = true;
                state.options.push(BootctlOption::Certificate(value()));
            }
            "certificate-source" => {
                state
                    .options
                    .push(BootctlOption::CertificateSource(parse_certificate_source(
                        &option.spelling,
                        value(),
                    )?))
            }
            "oldest" => {
                state.oldest = parse_boolean(COMMAND, &option.spelling, value())?;
                state.options.push(BootctlOption::Oldest(state.oldest));
            }
            "keep-free" => {
                let raw = value();
                let parsed = if raw.is_empty() {
                    None
                } else {
                    Some(parse_size(&option.spelling, &raw)?)
                };
                state.options.push(BootctlOption::KeepFree(parsed));
            }
            "entry-title" => {
                let raw = value();
                if raw.chars().any(char::is_control) {
                    return Err(invalid_value(
                        &option.spelling,
                        &raw,
                        "a string without control characters",
                    ));
                }
                state
                    .options
                    .push(BootctlOption::EntryTitle((!raw.is_empty()).then_some(raw)));
            }
            "entry-version" => {
                let raw = value();
                if !raw.is_empty()
                    && !raw.chars().all(|character| {
                        character.is_ascii_alphanumeric() || ".-~^".contains(character)
                    })
                {
                    return Err(invalid_value(
                        &option.spelling,
                        &raw,
                        "a systemd version using alphanumerics, '.', '-', '~', or '^'",
                    ));
                }
                state.options.push(BootctlOption::EntryVersion(
                    (!raw.is_empty()).then_some(raw),
                ));
            }
            "entry-commit" => {
                let raw = value();
                let commit = if raw.is_empty() {
                    None
                } else {
                    let value = raw
                        .parse::<u64>()
                        .map_err(|_| invalid_value(&option.spelling, &raw, "1..UINT64_MAX-1"))?;
                    if value == 0 || value == u64::MAX {
                        return Err(invalid_value(&option.spelling, &raw, "1..UINT64_MAX-1"));
                    }
                    Some(value)
                };
                state.options.push(BootctlOption::EntryCommit(commit));
            }
            "extra" => {
                let raw = value();
                if !raw.is_empty() && !resource_path_valid(&raw) {
                    return Err(invalid_value(
                        &option.spelling,
                        &raw,
                        "a path with an ASCII filename safe for a boot entry",
                    ));
                }
                state
                    .options
                    .push(BootctlOption::Extra((!raw.is_empty()).then_some(raw)));
            }
            "tries-left" => {
                let raw = value();
                let tries = if raw.is_empty() {
                    None
                } else {
                    let value = raw
                        .parse::<u32>()
                        .map_err(|_| invalid_value(&option.spelling, &raw, "0..UINT32_MAX-1"))?;
                    if value == u32::MAX {
                        return Err(invalid_value(&option.spelling, &raw, "0..UINT32_MAX-1"));
                    }
                    Some(value)
                };
                state.options.push(BootctlOption::TriesLeft(tries));
            }
            _ => unreachable!("closed bootctl option table"),
        }
    }

    if state.has_root && state.has_image {
        return Err(conflict("--root and --image are mutually exclusive"));
    }
    if state.has_install_source && !state.has_root && !state.has_image {
        return Err(conflict("--install-source requires --root or --image"));
    }
    if state.secure_boot_auto_enroll && (!state.has_private_key || !state.has_certificate) {
        return Err(conflict(
            "--secure-boot-auto-enroll=yes requires --private-key and --certificate",
        ));
    }

    let action = if let Some(action) = state.information {
        action
    } else if let Some(target) = state.print_target {
        if !scanned.operands.is_empty()
            && !(scanned.operands.len() == 1 && scanned.operands[0] == "status")
        {
            return Err(BootRuntimeParseError::UnexpectedOperand {
                command: COMMAND,
                action: "print",
                operand: scanned.operands[0].clone(),
            });
        }
        BootctlAction::Print(target)
    } else {
        parse_action(&scanned.operands, state.oldest)?
    };

    if (state.has_root || state.has_image) && !supports_offline_root(&action) {
        return Err(conflict(
            "--root and --image are unsupported by this bootctl action",
        ));
    }
    if state.dry_run && !matches!(action, BootctlAction::Unlink(_) | BootctlAction::Cleanup) {
        return Err(conflict(
            "--dry-run is only supported by unlink and cleanup",
        ));
    }

    Ok(BootctlInvocation {
        contract: BOOTCTL_CONTRACT,
        action,
        options: state.options,
    })
}

fn parse_action(operands: &[String], oldest: bool) -> Result<BootctlAction, BootRuntimeParseError> {
    const COMMAND: &str = "bootctl";
    if operands.is_empty() {
        return Ok(BootctlAction::Status);
    }
    let rest = &operands[1..];
    match operands[0].as_str() {
        "help" => Ok(BootctlAction::Help),
        "status" => {
            exact(COMMAND, "status", rest, 0, "no operands")?;
            Ok(BootctlAction::Status)
        }
        "reboot-to-firmware" => {
            exact_max(COMMAND, "reboot-to-firmware", rest, 1)?;
            if let Some(value) = rest.first() {
                Ok(BootctlAction::RebootToFirmwareSet(parse_boolean(
                    COMMAND,
                    "BOOL",
                    value.clone(),
                )?))
            } else {
                Ok(BootctlAction::RebootToFirmwareQuery)
            }
        }
        "list" => no_args("list", rest, BootctlAction::List),
        "unlink" => {
            exact_max(COMMAND, "unlink", rest, 1)?;
            let id = rest.first().cloned();
            if oldest == id.is_some() {
                return Err(conflict(
                    "unlink requires exactly one of an entry ID or --oldest=yes",
                ));
            }
            Ok(BootctlAction::Unlink(id))
        }
        "link" => one_arg("link", rest).map(BootctlAction::Link),
        "link-auto" => no_args("link-auto", rest, BootctlAction::LinkAuto),
        "cleanup" => no_args("cleanup", rest, BootctlAction::Cleanup),
        "set-default" => one_arg("set-default", rest)
            .and_then(|value| parse_entry_target(&value))
            .map(BootctlAction::SetDefault),
        "set-oneshot" => one_arg("set-oneshot", rest)
            .and_then(|value| parse_entry_target(&value))
            .map(BootctlAction::SetOneShot),
        "set-sysfail" => one_arg("set-sysfail", rest)
            .and_then(|value| parse_entry_target(&value))
            .map(BootctlAction::SetSystemFailure),
        "set-preferred" => one_arg("set-preferred", rest)
            .and_then(|value| parse_entry_target(&value))
            .map(BootctlAction::SetPreferred),
        "set-timeout" => one_arg("set-timeout", rest)
            .and_then(|value| parse_timeout(&value))
            .map(BootctlAction::SetTimeout),
        "set-timeout-oneshot" => one_arg("set-timeout-oneshot", rest)
            .and_then(|value| parse_timeout(&value))
            .map(BootctlAction::SetTimeoutOneShot),
        "install" => no_args("install", rest, BootctlAction::Install),
        "update" => no_args("update", rest, BootctlAction::Update),
        "remove" => no_args("remove", rest, BootctlAction::Remove),
        "is-installed" => no_args("is-installed", rest, BootctlAction::IsInstalled),
        "random-seed" => no_args("random-seed", rest, BootctlAction::RandomSeed),
        "kernel-identify" => one_arg("kernel-identify", rest).map(BootctlAction::KernelIdentify),
        "kernel-inspect" => one_arg("kernel-inspect", rest).map(BootctlAction::KernelInspect),
        action => Err(BootRuntimeParseError::UnknownAction {
            command: COMMAND,
            action: action.to_string(),
        }),
    }
}

fn set_print_target(
    state: &mut ParseState,
    target: BootctlPrintTarget,
) -> Result<(), BootRuntimeParseError> {
    let category = |target| match target {
        BootctlPrintTarget::RootDevice | BootctlPrintTarget::RootDisk => "root",
        BootctlPrintTarget::EspPath => "esp",
        BootctlPrintTarget::BootPath => "boot",
        BootctlPrintTarget::LoaderPath => "loader",
        BootctlPrintTarget::StubPath => "stub",
        BootctlPrintTarget::EfiArchitecture => "architecture",
    };
    if let Some(current) = state.print_target
        && category(current) != category(target)
    {
        return Err(conflict("bootctl print selectors are mutually exclusive"));
    }
    state.print_target = Some(target);
    Ok(())
}

fn supports_offline_root(action: &BootctlAction) -> bool {
    matches!(
        action,
        BootctlAction::Status
            | BootctlAction::Print(BootctlPrintTarget::EspPath | BootctlPrintTarget::BootPath)
            | BootctlAction::List
            | BootctlAction::Install
            | BootctlAction::Update
            | BootctlAction::Remove
            | BootctlAction::IsInstalled
            | BootctlAction::RandomSeed
            | BootctlAction::Unlink(_)
            | BootctlAction::Cleanup
    )
}

fn parse_tristate(option: &str, value: String) -> Result<BootctlTriState, BootRuntimeParseError> {
    if value.is_empty() || value == "auto" {
        Ok(BootctlTriState::Auto)
    } else {
        parse_boolean("bootctl", option, value).map(BootctlTriState::Boolean)
    }
}

fn parse_make_entry_directory(
    option: &str,
    value: String,
) -> Result<BootctlTriState, BootRuntimeParseError> {
    if value == "auto" {
        Ok(BootctlTriState::Auto)
    } else {
        parse_boolean("bootctl", option, value).map(BootctlTriState::Boolean)
    }
}

fn parse_entry_token(
    option: &str,
    value: String,
) -> Result<BootctlEntryToken, BootRuntimeParseError> {
    match value.as_str() {
        "auto" => Ok(BootctlEntryToken::Auto),
        "machine-id" => Ok(BootctlEntryToken::MachineId),
        "os-id" => Ok(BootctlEntryToken::OsId),
        "os-image-id" => Ok(BootctlEntryToken::OsImageId),
        _ => {
            if let Some(literal) = value.strip_prefix("literal:")
                && filename_valid(literal)
            {
                return Ok(BootctlEntryToken::Literal(literal.to_string()));
            }
            Err(invalid_value(
                option,
                &value,
                "auto, machine-id, os-id, os-image-id, or literal:FILENAME",
            ))
        }
    }
}

fn parse_key_source(
    option: &str,
    value: String,
) -> Result<BootctlKeySource, BootRuntimeParseError> {
    if value == "file" {
        Ok(BootctlKeySource::File)
    } else if let Some(engine) = value.strip_prefix("engine:") {
        Ok(BootctlKeySource::Engine(engine.to_string()))
    } else if let Some(provider) = value.strip_prefix("provider:") {
        Ok(BootctlKeySource::Provider(provider.to_string()))
    } else {
        Err(invalid_value(
            option,
            &value,
            "file, engine:ENGINE, or provider:PROVIDER",
        ))
    }
}

fn parse_certificate_source(
    option: &str,
    value: String,
) -> Result<BootctlCertificateSource, BootRuntimeParseError> {
    if value == "file" {
        Ok(BootctlCertificateSource::File)
    } else if let Some(provider) = value.strip_prefix("provider:") {
        Ok(BootctlCertificateSource::Provider(provider.to_string()))
    } else {
        Err(invalid_value(option, &value, "file or provider:PROVIDER"))
    }
}

fn parse_entry_target(value: &str) -> Result<BootctlEntryTarget, BootRuntimeParseError> {
    match value {
        "" => Ok(BootctlEntryTarget::Clear),
        "@current" => Ok(BootctlEntryTarget::Current),
        "@oneshot" => Ok(BootctlEntryTarget::OneShot),
        "@default" => Ok(BootctlEntryTarget::Default),
        "@sysfail" => Ok(BootctlEntryTarget::SystemFailure),
        "@saved" => Ok(BootctlEntryTarget::Saved),
        invalid if invalid.starts_with('@') => Err(invalid_value(
            "ID",
            invalid,
            "a declared @ target or entry ID",
        )),
        id => Ok(BootctlEntryTarget::Id(id.to_string())),
    }
}

fn parse_timeout(value: &str) -> Result<BootctlTimeout, BootRuntimeParseError> {
    match value {
        "" => Ok(BootctlTimeout::Clear),
        "menu-force" => Ok(BootctlTimeout::MenuForce),
        "menu-hidden" => Ok(BootctlTimeout::MenuHidden),
        "menu-disabled" => Ok(BootctlTimeout::MenuDisabled),
        "infinity" => Ok(BootctlTimeout::Duration(value.to_string())),
        duration if duration_expression_valid(duration) => {
            Ok(BootctlTimeout::Duration(duration.to_string()))
        }
        invalid => Err(invalid_value(
            "SECONDS",
            invalid,
            "menu-force, menu-hidden, menu-disabled, infinity, or a duration",
        )),
    }
}

fn duration_expression_valid(value: &str) -> bool {
    const USEC_INFINITY: u64 = u64::MAX;
    const UNITS: &[(&str, u64)] = &[
        ("seconds", 1_000_000),
        ("second", 1_000_000),
        ("sec", 1_000_000),
        ("s", 1_000_000),
        ("minutes", 60_000_000),
        ("minute", 60_000_000),
        ("min", 60_000_000),
        ("months", 2_629_800_000_000),
        ("month", 2_629_800_000_000),
        ("M", 2_629_800_000_000),
        ("msec", 1_000),
        ("ms", 1_000),
        ("m", 60_000_000),
        ("hours", 3_600_000_000),
        ("hour", 3_600_000_000),
        ("hr", 3_600_000_000),
        ("h", 3_600_000_000),
        ("days", 86_400_000_000),
        ("day", 86_400_000_000),
        ("d", 86_400_000_000),
        ("weeks", 604_800_000_000),
        ("week", 604_800_000_000),
        ("w", 604_800_000_000),
        ("years", 31_557_600_000_000),
        ("year", 31_557_600_000_000),
        ("y", 31_557_600_000_000),
        ("usec", 1),
        ("us", 1),
        ("μs", 1),
        ("µs", 1),
    ];

    let mut rest = trim_systemd_whitespace(value);
    if let Some(after) = rest.strip_prefix("infinity") {
        return trim_systemd_whitespace(after).is_empty();
    }
    if rest.is_empty() {
        return false;
    }

    let mut total = 0_u64;
    let mut parsed_any = false;
    loop {
        rest = trim_systemd_whitespace(rest);
        if rest.is_empty() {
            return parsed_any;
        }
        if rest.starts_with('-') {
            return false;
        }

        let unsigned = rest.strip_prefix('+').unwrap_or(rest);
        let integer_len = unsigned.bytes().take_while(u8::is_ascii_digit).count();
        if integer_len == 0 {
            return false;
        }
        let Ok(integer) = unsigned[..integer_len].parse::<u64>() else {
            return false;
        };
        if integer > i64::MAX as u64 {
            return false;
        }
        rest = &unsigned[integer_len..];

        let fraction = if let Some(after_dot) = rest.strip_prefix('.') {
            let digits = after_dot.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 {
                return false;
            }
            rest = &after_dot[digits..];
            Some(&after_dot[..digits])
        } else {
            None
        };

        let unit_input = trim_systemd_whitespace(rest);
        let (multiplier, after_unit) = UNITS
            .iter()
            .find_map(|(suffix, multiplier)| {
                unit_input
                    .strip_prefix(suffix)
                    .map(|after| (*multiplier, after))
            })
            .unwrap_or((1_000_000, unit_input));
        if after_unit.len() == unit_input.len()
            && rest.len() == unit_input.len()
            && !after_unit.is_empty()
        {
            return false;
        }

        if integer >= USEC_INFINITY / multiplier {
            return false;
        }
        let integer_part = integer * multiplier;
        if integer_part >= USEC_INFINITY - total {
            return false;
        }
        total += integer_part;

        if let Some(digits) = fraction {
            let mut place = multiplier / 10;
            for digit in digits.bytes() {
                let part = u64::from(digit - b'0') * place;
                if part >= USEC_INFINITY - total {
                    return false;
                }
                total += part;
                place /= 10;
            }
        }

        parsed_any = true;
        rest = after_unit;
    }
}

fn parse_size(option: &str, value: &str) -> Result<u64, BootRuntimeParseError> {
    let mut rest = value;
    let mut total = 0_u128;
    let mut previous_rank = 8_usize;
    let mut parsed_any = false;
    loop {
        rest = trim_systemd_whitespace(rest);
        if rest.is_empty() {
            return if parsed_any {
                Ok(total as u64)
            } else {
                Err(invalid_value(option, value, "a systemd IEC byte size"))
            };
        }

        let unsigned = rest.strip_prefix('+').unwrap_or(rest);
        let integer_len = unsigned.bytes().take_while(u8::is_ascii_digit).count();
        if integer_len == 0 {
            return Err(invalid_value(option, value, "a systemd IEC byte size"));
        }
        let integer = unsigned[..integer_len]
            .parse::<u64>()
            .map_err(|_| invalid_value(option, value, "a systemd IEC byte size"))?;
        rest = &unsigned[integer_len..];

        let fraction = if let Some(after_dot) = rest.strip_prefix('.') {
            let digits = after_dot.bytes().take_while(u8::is_ascii_digit).count();
            let fraction = if digits == 0 {
                0.0
            } else {
                let integer_fraction = after_dot[..digits]
                    .parse::<u64>()
                    .map_err(|_| invalid_value(option, value, "a systemd IEC byte size"))?;
                let mut fraction = integer_fraction as f64;
                for _ in 0..digits {
                    fraction /= 10.0;
                }
                fraction
            };
            rest = &after_dot[digits..];
            fraction
        } else {
            0.0
        };

        rest = trim_systemd_whitespace(rest);
        let suffix = rest
            .chars()
            .next()
            .filter(|character| "EPTGMKB".contains(*character));
        let (rank, factor) = match suffix {
            Some('E') => (7, 1024_u128.pow(6)),
            Some('P') => (6, 1024_u128.pow(5)),
            Some('T') => (5, 1024_u128.pow(4)),
            Some('G') => (4, 1024_u128.pow(3)),
            Some('M') => (3, 1024_u128.pow(2)),
            Some('K') => (2, 1024_u128),
            Some('B') => (1, 1),
            None => (0, 1),
            _ => unreachable!(),
        };
        if rank >= previous_rank {
            return Err(invalid_value(option, value, "descending IEC size units"));
        }
        previous_rank = rank;
        let rounded_integer = integer
            .checked_add(if fraction > 0.0 { 1 } else { 0 })
            .ok_or_else(|| invalid_value(option, value, "a u64 IEC byte size"))?;
        if u128::from(rounded_integer) > u128::from(u64::MAX) / factor {
            return Err(invalid_value(option, value, "a u64 IEC byte size"));
        }
        let component = u128::from(integer) * factor + (fraction * factor as f64) as u128;
        total = total
            .checked_add(component)
            .ok_or_else(|| invalid_value(option, value, "a u64 IEC byte size"))?;
        if total > u64::MAX as u128 {
            return Err(invalid_value(option, value, "a u64 IEC byte size"));
        }
        if suffix.is_some() {
            rest = &rest[1..];
        } else if !rest.is_empty() {
            return Err(invalid_value(option, value, "a systemd IEC byte size"));
        }
        parsed_any = true;
    }
}

fn trim_systemd_whitespace(value: &str) -> &str {
    value.trim_start_matches([' ', '\t', '\n', '\r'])
}

fn filename_valid(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value.len() <= 255
        && !value.contains('/')
        && !value.chars().any(char::is_control)
}

fn resource_path_valid(value: &str) -> bool {
    value
        .rsplit('/')
        .next()
        .is_some_and(|name| filename_valid(name) && name.is_ascii())
}

fn exact(
    command: &'static str,
    action: &'static str,
    operands: &[String],
    count: usize,
    expected: &'static str,
) -> Result<(), BootRuntimeParseError> {
    only_operand_count(command, action, operands, count, count, expected)
}

fn exact_max(
    command: &'static str,
    action: &'static str,
    operands: &[String],
    max: usize,
) -> Result<(), BootRuntimeParseError> {
    only_operand_count(command, action, operands, 0, max, "fewer operands")
}

fn no_args(
    action: &'static str,
    operands: &[String],
    result: BootctlAction,
) -> Result<BootctlAction, BootRuntimeParseError> {
    exact("bootctl", action, operands, 0, "no operands")?;
    Ok(result)
}

fn one_arg(action: &'static str, operands: &[String]) -> Result<String, BootRuntimeParseError> {
    exact("bootctl", action, operands, 1, "one operand")?;
    Ok(operands[0].clone())
}

fn invalid_value(option: &str, value: &str, expected: &'static str) -> BootRuntimeParseError {
    BootRuntimeParseError::InvalidOptionValue {
        command: "bootctl",
        option: option.to_string(),
        value: value.to_string(),
        expected,
    }
}

fn conflict(detail: &'static str) -> BootRuntimeParseError {
    BootRuntimeParseError::ConflictingArguments {
        command: "bootctl",
        detail,
    }
}

#[cfg(test)]
mod tests;
