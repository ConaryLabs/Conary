// crates/conary-core/src/boot_runtime/initramfs/dracut/parser.rs

use super::types::{
    DRACUT_CONTRACT, DracutAction, DracutInvocation, DracutOption, HostOnlyMode, NvmfNbftMode,
};
use crate::boot_runtime::BootRuntimeParseError;
use crate::boot_runtime::argv::{OptionSpec, only_operand_count, require_nonempty, scan_gnu};

const OPTIONS: &[OptionSpec] = &[
    OptionSpec::value("kver", None, "kver"),
    OptionSpec::value("add", Some('a'), "add"),
    OptionSpec::value("force-add", None, "force-add"),
    OptionSpec::value("add-drivers", None, "add-drivers"),
    OptionSpec::value("force-drivers", None, "force-drivers"),
    OptionSpec::value("omit-drivers", None, "omit-drivers"),
    OptionSpec::value("modules", Some('m'), "modules"),
    OptionSpec::value("omit", Some('o'), "omit"),
    OptionSpec::value("drivers", Some('d'), "drivers"),
    OptionSpec::value("filesystems", None, "filesystems"),
    OptionSpec::value("install", Some('I'), "install"),
    OptionSpec::value("install-optional", None, "install-optional"),
    OptionSpec::value("fwdir", None, "fwdir"),
    OptionSpec::value("libdirs", None, "libdirs"),
    OptionSpec::value("fscks", None, "fscks"),
    OptionSpec::value("add-fstab", None, "add-fstab"),
    OptionSpec::value("mount", None, "mount"),
    OptionSpec::value("device", None, "device"),
    OptionSpec::value("device", None, "add-device"),
    OptionSpec::flag("nofscks", None, "nofscks"),
    OptionSpec::flag("ro-mnt", None, "ro-mnt"),
    OptionSpec::value("kmoddir", Some('k'), "kmoddir"),
    OptionSpec::value("conf", Some('c'), "conf"),
    OptionSpec::value("confdir", None, "confdir"),
    OptionSpec::value("add-confdir", None, "add-confdir"),
    OptionSpec::value("tmpdir", None, "tmpdir"),
    OptionSpec::value("sysroot", Some('r'), "sysroot"),
    OptionSpec::value("stdlog", Some('L'), "stdlog"),
    OptionSpec::value("compress", None, "compress"),
    OptionSpec::value("compress-level", None, "compress-level"),
    OptionSpec::value("squash-compressor", None, "squash-compressor"),
    OptionSpec::value("prefix", None, "prefix"),
    OptionSpec::value("rebuild", None, "rebuild"),
    OptionSpec::flag("force", Some('f'), "force"),
    OptionSpec::flag("kernel-only", None, "kernel-only"),
    OptionSpec::flag("no-kernel", None, "no-kernel"),
    OptionSpec::flag("print-cmdline", None, "print-cmdline"),
    OptionSpec::value("kernel-cmdline", None, "kernel-cmdline"),
    OptionSpec::flag("strip", None, "strip"),
    OptionSpec::flag("aggressive-strip", None, "aggressive-strip"),
    OptionSpec::flag("nostrip", None, "nostrip"),
    OptionSpec::flag("hardlink", None, "hardlink"),
    OptionSpec::flag("nohardlink", None, "nohardlink"),
    OptionSpec::flag("noprefix", None, "noprefix"),
    OptionSpec::flag("mdadmconf", None, "mdadmconf"),
    OptionSpec::flag("nomdadmconf", None, "nomdadmconf"),
    OptionSpec::flag("lvmconf", None, "lvmconf"),
    OptionSpec::flag("nolvmconf", None, "nolvmconf"),
    OptionSpec::value("nvmf-nbft-mode", None, "nvmf-nbft-mode"),
    OptionSpec::flag("debug", None, "debug"),
    OptionSpec::flag("profile", None, "profile"),
    OptionSpec::value("sshkey", None, "sshkey"),
    OptionSpec::value("logfile", None, "logfile"),
    OptionSpec::flag("verbose", Some('v'), "verbose"),
    OptionSpec::flag("quiet", Some('q'), "quiet"),
    OptionSpec::flag("local", Some('l'), "local"),
    OptionSpec::flag("hostonly", Some('H'), "hostonly"),
    OptionSpec::flag("hostonly", None, "host-only"),
    OptionSpec::flag("no-hostonly", Some('N'), "no-hostonly"),
    OptionSpec::flag("no-hostonly", None, "no-host-only"),
    OptionSpec::value("hostonly-mode", None, "hostonly-mode"),
    OptionSpec::flag("hostonly-cmdline", None, "hostonly-cmdline"),
    OptionSpec::flag("no-hostonly-cmdline", None, "no-hostonly-cmdline"),
    OptionSpec::flag(
        "no-hostonly-default-device",
        None,
        "no-hostonly-default-device",
    ),
    OptionSpec::value("hostonly-nics", None, "hostonly-nics"),
    OptionSpec::value("persistent-policy", None, "persistent-policy"),
    OptionSpec::flag("fstab", None, "fstab"),
    OptionSpec::flag("help", Some('h'), "help"),
    OptionSpec::flag("bzip2", None, "bzip2"),
    OptionSpec::flag("lzma", None, "lzma"),
    OptionSpec::flag("xz", None, "xz"),
    OptionSpec::flag("lzo", None, "lzo"),
    OptionSpec::flag("lz4", None, "lz4"),
    OptionSpec::flag("zstd", None, "zstd"),
    OptionSpec::flag("no-compress", None, "no-compress"),
    OptionSpec::flag("gzip", None, "gzip"),
    OptionSpec::flag("enhanced-cpio", None, "enhanced-cpio"),
    OptionSpec::flag("list-modules", None, "list-modules"),
    OptionSpec::flag("show-modules", Some('M'), "show-modules"),
    OptionSpec::flag("keep", None, "keep"),
    OptionSpec::flag("printsize", None, "printsize"),
    OptionSpec::flag("printconfig", None, "printconfig"),
    OptionSpec::flag("regenerate-all", None, "regenerate-all"),
    OptionSpec::flag("parallel", Some('p'), "parallel"),
    OptionSpec::flag("noimageifnotneeded", None, "noimageifnotneeded"),
    OptionSpec::flag("early-microcode", None, "early-microcode"),
    OptionSpec::flag("no-early-microcode", None, "no-early-microcode"),
    OptionSpec::flag("reproducible", None, "reproducible"),
    OptionSpec::flag("no-reproducible", None, "no-reproducible"),
    OptionSpec::value("loginstall", None, "loginstall"),
    OptionSpec::flag("uefi", None, "uefi"),
    OptionSpec::flag("ukify", None, "ukify"),
    OptionSpec::flag("no-uefi", None, "no-uefi"),
    OptionSpec::flag("no-ukify", None, "no-ukify"),
    OptionSpec::value("uefi-stub", None, "uefi-stub"),
    OptionSpec::value("uefi-splash-image", None, "uefi-splash-image"),
    OptionSpec::value("kernel-image", None, "kernel-image"),
    OptionSpec::value("sbat", None, "sbat"),
    OptionSpec::flag("no-hostonly-i18n", None, "no-hostonly-i18n"),
    OptionSpec::flag("hostonly-i18n", None, "hostonly-i18n"),
    OptionSpec::flag("no-machineid", None, "no-machineid"),
    OptionSpec::flag("version", None, "version"),
    OptionSpec::value("remove", None, "remove"),
    OptionSpec::pair("include", Some('i'), "include"),
];

pub fn parse_dracut(argv: &[&str]) -> Result<DracutInvocation, BootRuntimeParseError> {
    const COMMAND: &str = "dracut";
    for token in argv {
        if token.starts_with("--include=") || (token.starts_with("-i") && *token != "-i") {
            return Err(BootRuntimeParseError::UnknownOption {
                command: COMMAND,
                option: (*token).to_string(),
            });
        }
    }
    let scanned = scan_gnu(COMMAND, argv, OPTIONS)?;
    only_operand_count(
        COMMAND,
        "build",
        &scanned.operands,
        0,
        2,
        "at most an output path and kernel version",
    )?;

    let mut options = Vec::new();
    let mut information = None;
    let mut status = None;
    let mut rebuild = None;
    let mut regenerate_all = false;
    let mut explicit_kernel = None;

    for option in scanned.options {
        let value = || option.values[0].clone();
        let required = |expected| require_nonempty(COMMAND, &option.spelling, value(), expected);
        match option.id {
            "kver" => {
                let version = required("a kernel version")?;
                explicit_kernel = Some(version.clone());
                options.push(DracutOption::KernelVersion(version));
            }
            "add" => options.push(DracutOption::AddModules(required("a module list")?)),
            "force-add" => {
                options.push(DracutOption::ForceAddModules(required("a module list")?));
            }
            "add-drivers" => {
                options.push(DracutOption::AddDrivers(required("a driver list")?));
            }
            "force-drivers" => {
                options.push(DracutOption::ForceDrivers(required("a driver list")?));
            }
            "omit-drivers" => {
                options.push(DracutOption::OmitDrivers(required("a driver list")?));
            }
            "modules" => options.push(DracutOption::Modules(required("a module list")?)),
            "omit" => options.push(DracutOption::OmitModules(required("a module list")?)),
            "drivers" => options.push(DracutOption::Drivers(required("a driver list")?)),
            "filesystems" => {
                options.push(DracutOption::Filesystems(required("a filesystem list")?));
            }
            "install" => options.push(DracutOption::Install(required("a path list")?)),
            "install-optional" => {
                options.push(DracutOption::InstallOptional(required("a path list")?));
            }
            "fwdir" => {
                options.push(DracutOption::FirmwareDirectory(required(
                    "a directory list",
                )?));
            }
            "libdirs" => {
                options.push(DracutOption::LibraryDirectories(required(
                    "a directory list",
                )?));
            }
            "fscks" => options.push(DracutOption::Fscks(required("an fsck list")?)),
            "add-fstab" => options.push(DracutOption::AddFstab(required("an fstab path")?)),
            "mount" => options.push(DracutOption::Mount(required("a mount specification")?)),
            "device" => options.push(DracutOption::AddDevice(required("a device")?)),
            "nofscks" => options.push(DracutOption::NoFscks),
            "ro-mnt" => options.push(DracutOption::ReadOnlyMount),
            "kmoddir" => options.push(DracutOption::KernelModuleDirectory(required(
                "a kernel module directory",
            )?)),
            "conf" => options.push(DracutOption::Config(required("a configuration path")?)),
            "confdir" => {
                options.push(DracutOption::ConfigDirectory(required("a directory")?));
            }
            "add-confdir" => {
                options.push(DracutOption::AddConfigDirectory(required("a directory")?));
            }
            "tmpdir" => {
                options.push(DracutOption::TemporaryDirectory(required("a directory")?));
            }
            "sysroot" => options.push(DracutOption::Sysroot(required("a sysroot path")?)),
            "stdlog" => {
                let raw = value();
                let level = raw
                    .parse::<u8>()
                    .map_err(|_| invalid(&option.spelling, &raw, "0-6"))?;
                if level > 6 {
                    return Err(invalid(&option.spelling, &raw, "0-6"));
                }
                options.push(DracutOption::StandardLogLevel(level));
            }
            "compress" => options.push(DracutOption::Compressor(required(
                "a compressor program and optional arguments",
            )?)),
            "compress-level" => {
                options.push(DracutOption::CompressionLevel(required(
                    "a compression level",
                )?));
            }
            "squash-compressor" => options.push(DracutOption::SquashCompressor(required(
                "a squash compressor and optional arguments",
            )?)),
            "prefix" => options.push(DracutOption::Prefix(required("an initramfs prefix")?)),
            "rebuild" => rebuild = Some(required("an existing initramfs image")?),
            "force" => options.push(DracutOption::Force),
            "kernel-only" => options.push(DracutOption::KernelOnly),
            "no-kernel" => options.push(DracutOption::NoKernel),
            "print-cmdline" => {
                status.get_or_insert(DracutAction::PrintCommandLine);
            }
            "kernel-cmdline" => {
                options.push(DracutOption::KernelCommandLine(required(
                    "kernel arguments",
                )?));
            }
            "strip" => options.push(DracutOption::Strip),
            "aggressive-strip" => options.push(DracutOption::AggressiveStrip),
            "nostrip" => options.push(DracutOption::NoStrip),
            "hardlink" => options.push(DracutOption::Hardlink),
            "nohardlink" => options.push(DracutOption::NoHardlink),
            "noprefix" => options.push(DracutOption::NoPrefix),
            "mdadmconf" => options.push(DracutOption::MdadmConfig),
            "nomdadmconf" => options.push(DracutOption::NoMdadmConfig),
            "lvmconf" => options.push(DracutOption::LvmConfig),
            "nolvmconf" => options.push(DracutOption::NoLvmConfig),
            "nvmf-nbft-mode" => options.push(DracutOption::NvmfNbftMode(match value().as_str() {
                "match" => NvmfNbftMode::Match,
                "nbft" => NvmfNbftMode::Nbft,
                "static" => NvmfNbftMode::Static,
                invalid => {
                    return Err(invalid_value(
                        &option.spelling,
                        invalid,
                        "match, nbft, or static",
                    ));
                }
            })),
            "debug" => options.push(DracutOption::Debug),
            "profile" => options.push(DracutOption::Profile),
            "sshkey" => options.push(DracutOption::SshKey(required("an SSH key path")?)),
            "logfile" => options.push(DracutOption::LogFile(required("a log path")?)),
            "verbose" => options.push(DracutOption::Verbose),
            "quiet" => options.push(DracutOption::Quiet),
            "local" => options.push(DracutOption::Local),
            "hostonly" => options.push(DracutOption::HostOnly),
            "no-hostonly" => options.push(DracutOption::NoHostOnly),
            "hostonly-mode" => options.push(DracutOption::HostOnlyMode(match value().as_str() {
                "sloppy" => HostOnlyMode::Sloppy,
                "strict" => HostOnlyMode::Strict,
                invalid => {
                    return Err(invalid_value(&option.spelling, invalid, "sloppy or strict"));
                }
            })),
            "hostonly-cmdline" => options.push(DracutOption::HostOnlyCommandLine),
            "no-hostonly-cmdline" => options.push(DracutOption::NoHostOnlyCommandLine),
            "no-hostonly-default-device" => {
                options.push(DracutOption::NoHostOnlyDefaultDevice);
            }
            "hostonly-nics" => {
                options.push(DracutOption::HostOnlyNics(required(
                    "a network interface list",
                )?));
            }
            "persistent-policy" => options.push(DracutOption::PersistentPolicy(required(
                "mapper or a /dev/disk policy directory name",
            )?)),
            "fstab" => options.push(DracutOption::Fstab),
            "help" => {
                information.get_or_insert(DracutAction::Help);
            }
            "bzip2" => options.push(DracutOption::Bzip2),
            "lzma" => options.push(DracutOption::Lzma),
            "xz" => options.push(DracutOption::Xz),
            "lzo" => options.push(DracutOption::Lzo),
            "lz4" => options.push(DracutOption::Lz4),
            "zstd" => options.push(DracutOption::Zstd),
            "no-compress" => options.push(DracutOption::NoCompression),
            "gzip" => options.push(DracutOption::Gzip),
            "enhanced-cpio" => options.push(DracutOption::EnhancedCpio),
            "list-modules" => {
                status.get_or_insert(DracutAction::ListModules);
            }
            "show-modules" => options.push(DracutOption::ShowModules),
            "keep" => options.push(DracutOption::Keep),
            "printsize" => options.push(DracutOption::PrintSize),
            "printconfig" => {
                status.get_or_insert(DracutAction::PrintConfiguration);
            }
            "regenerate-all" => regenerate_all = true,
            "parallel" => options.push(DracutOption::Parallel),
            "noimageifnotneeded" => options.push(DracutOption::NoImageIfNotNeeded),
            "early-microcode" => options.push(DracutOption::EarlyMicrocode),
            "no-early-microcode" => options.push(DracutOption::NoEarlyMicrocode),
            "reproducible" => options.push(DracutOption::Reproducible),
            "no-reproducible" => options.push(DracutOption::NoReproducible),
            "loginstall" => {
                options.push(DracutOption::LogInstall(required("a log directory")?));
            }
            "uefi" => options.push(DracutOption::Uefi),
            "ukify" => options.push(DracutOption::Ukify),
            "no-uefi" => options.push(DracutOption::NoUefi),
            "no-ukify" => options.push(DracutOption::NoUkify),
            "uefi-stub" => options.push(DracutOption::UefiStub(required("a UEFI stub path")?)),
            "uefi-splash-image" => options.push(DracutOption::UefiSplashImage(required(
                "a UEFI splash image path",
            )?)),
            "kernel-image" => {
                options.push(DracutOption::KernelImage(required("a kernel image path")?));
            }
            "sbat" => options.push(DracutOption::Sbat(required("an SBAT data path")?)),
            "no-hostonly-i18n" => options.push(DracutOption::NoHostOnlyI18n),
            "hostonly-i18n" => options.push(DracutOption::HostOnlyI18n),
            "no-machineid" => options.push(DracutOption::NoMachineId),
            "version" => {
                information.get_or_insert(DracutAction::Version);
            }
            "remove" => options.push(DracutOption::Remove(required("a path list")?)),
            "include" => {
                let source = option.values[0].clone();
                let target = option.values[1].clone();
                if source.is_empty() || target.is_empty() {
                    return Err(invalid_value(
                        &option.spelling,
                        "",
                        "non-empty source and target paths",
                    ));
                }
                options.push(DracutOption::Include { source, target });
            }
            _ => unreachable!("closed dracut option table"),
        }
    }

    let positional_output = scanned.operands.first().cloned();
    let positional_kernel = scanned.operands.get(1).cloned();
    let kernel_version = explicit_kernel.or(positional_kernel);
    let action = if let Some(action) = information {
        action
    } else if let Some(action) = status {
        if let Some(operand) = &positional_output {
            return Err(BootRuntimeParseError::UnexpectedOperand {
                command: COMMAND,
                action: "status",
                operand: operand.clone(),
            });
        }
        action
    } else if regenerate_all {
        if positional_output.is_some() || rebuild.is_some() {
            return Err(BootRuntimeParseError::ConflictingArguments {
                command: COMMAND,
                detail: "--regenerate-all cannot be combined with positional output or --rebuild",
            });
        }
        DracutAction::RegenerateAll
    } else if let Some(input) = rebuild {
        DracutAction::Rebuild {
            input,
            output: positional_output,
            kernel_version,
        }
    } else {
        DracutAction::Build {
            output: positional_output,
            kernel_version,
        }
    };

    Ok(DracutInvocation {
        contract: DRACUT_CONTRACT,
        action,
        options,
    })
}

fn invalid(option: &str, value: &str, expected: &'static str) -> BootRuntimeParseError {
    invalid_value(option, value, expected)
}

fn invalid_value(option: &str, value: &str, expected: &'static str) -> BootRuntimeParseError {
    BootRuntimeParseError::InvalidOptionValue {
        command: "dracut",
        option: option.to_string(),
        value: value.to_string(),
        expected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_value_options_aliases_and_include_pair() {
        let parsed = parse_dracut(&[
            "--kver=6.12",
            "-a",
            "base",
            "--force-add=crypt",
            "--add-drivers",
            "nvme",
            "--force-drivers=xhci",
            "--omit-drivers=floppy",
            "-m",
            "systemd",
            "-o",
            "network",
            "-d",
            "virtio",
            "--filesystems=ext4",
            "-I/bin/sh",
            "--install-optional=/optional",
            "--fwdir=/firmware",
            "--libdirs=/lib",
            "--fscks=fsck.ext4",
            "--add-fstab=/etc/fstab",
            "--mount=/dev/root /",
            "--device=/dev/sda",
            "--add-device=/dev/sdb",
            "-k/lib/modules",
            "-c/etc/dracut.conf",
            "--confdir=/etc/dracut.conf.d",
            "--add-confdir=/vendor",
            "--tmpdir=/tmp",
            "-r/sysroot",
            "-L6",
            "--compress=zstd",
            "--compress-level=10",
            "--squash-compressor=xz",
            "--prefix=/prefix",
            "--kernel-cmdline=quiet",
            "--nvmf-nbft-mode=match",
            "--sshkey=/key",
            "--logfile=/log",
            "--hostonly-mode=strict",
            "--hostonly-nics=eth0",
            "--persistent-policy=by-uuid",
            "--loginstall=/loginstall",
            "--uefi-stub=/stub",
            "--uefi-splash-image=/splash",
            "--kernel-image=/vmlinuz",
            "--sbat=/sbat",
            "--remove=/old",
            "--include",
            "/source",
            "/target",
            "/output.img",
        ])
        .unwrap();
        assert!(matches!(
            parsed.action,
            DracutAction::Build {
                output: Some(ref output),
                kernel_version: Some(ref kernel),
            } if output == "/output.img" && kernel == "6.12"
        ));
        assert_eq!(parsed.options.len(), 44);
    }

    #[test]
    fn covers_all_flag_options() {
        let parsed = parse_dracut(&[
            "--nofscks",
            "--ro-mnt",
            "-f",
            "--kernel-only",
            "--no-kernel",
            "--strip",
            "--aggressive-strip",
            "--nostrip",
            "--hardlink",
            "--nohardlink",
            "--noprefix",
            "--mdadmconf",
            "--nomdadmconf",
            "--lvmconf",
            "--nolvmconf",
            "--debug",
            "--profile",
            "-v",
            "-q",
            "-l",
            "-H",
            "--host-only",
            "-N",
            "--no-host-only",
            "--hostonly-cmdline",
            "--no-hostonly-cmdline",
            "--no-hostonly-default-device",
            "--fstab",
            "--bzip2",
            "--lzma",
            "--xz",
            "--lzo",
            "--lz4",
            "--zstd",
            "--no-compress",
            "--gzip",
            "--enhanced-cpio",
            "-M",
            "--keep",
            "--printsize",
            "-p",
            "--noimageifnotneeded",
            "--early-microcode",
            "--no-early-microcode",
            "--reproducible",
            "--no-reproducible",
            "--uefi",
            "--ukify",
            "--no-uefi",
            "--no-ukify",
            "--no-hostonly-i18n",
            "--hostonly-i18n",
            "--no-machineid",
        ])
        .unwrap();
        assert_eq!(parsed.options.len(), 53);
    }

    #[test]
    fn covers_information_status_and_mutation_actions() {
        assert_eq!(parse_dracut(&["-h"]).unwrap().action, DracutAction::Help);
        assert_eq!(
            parse_dracut(&["--version"]).unwrap().action,
            DracutAction::Version
        );
        assert_eq!(
            parse_dracut(&["--list-modules"]).unwrap().action,
            DracutAction::ListModules
        );
        assert_eq!(
            parse_dracut(&["--print-cmdline"]).unwrap().action,
            DracutAction::PrintCommandLine
        );
        assert_eq!(
            parse_dracut(&["--printconfig"]).unwrap().action,
            DracutAction::PrintConfiguration
        );
        assert_eq!(
            parse_dracut(&["--regenerate-all"]).unwrap().action,
            DracutAction::RegenerateAll
        );
        assert!(matches!(
            parse_dracut(&["--rebuild=/old.img", "/new.img", "6.12"])
                .unwrap()
                .action,
            DracutAction::Rebuild { .. }
        ));
    }

    #[test]
    fn enforces_exact_include_and_closed_value_grammars() {
        assert!(parse_dracut(&["--include=/source", "/target"]).is_err());
        assert!(parse_dracut(&["-i/source", "/target"]).is_err());
        assert!(parse_dracut(&["--include", "/source"]).is_err());
        assert!(parse_dracut(&["--stdlog=7"]).is_err());
        assert!(parse_dracut(&["--hostonly-mode=fast"]).is_err());
        assert!(parse_dracut(&["--nvmf-nbft-mode=auto"]).is_err());
        assert!(parse_dracut(&["--future"]).is_err());
        assert!(parse_dracut(&["one", "two", "three"]).is_err());
    }
}
