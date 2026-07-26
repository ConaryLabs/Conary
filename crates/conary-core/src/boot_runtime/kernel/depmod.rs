// crates/conary-core/src/boot_runtime/kernel/depmod.rs

use crate::boot_runtime::argv::{OptionSpec, only_operand_count, require_nonempty, scan_gnu};
use crate::boot_runtime::{BootRuntimeParseError, InvocationDisposition, UpstreamContract};

pub const DEPMOD_CONTRACT: UpstreamContract = UpstreamContract {
    project: "kmod",
    version: "34",
    revision: "84fa569b2eb3230284168d59863330bafb07bc52",
    source_url: "https://github.com/kmod-project/kmod",
};

const OPTIONS: &[OptionSpec] = &[
    OptionSpec::flag("all", Some('a'), "all"),
    OptionSpec::flag("quick", Some('A'), "quick"),
    OptionSpec::value("basedir", Some('b'), "basedir"),
    OptionSpec::value("moduledir", Some('m'), "moduledir"),
    OptionSpec::value("outdir", Some('o'), "outdir"),
    OptionSpec::value("config", Some('C'), "config"),
    OptionSpec::value("symvers", Some('E'), "symvers"),
    OptionSpec::value("filesyms", Some('F'), "filesyms"),
    OptionSpec::flag("errsyms", Some('e'), "errsyms"),
    OptionSpec::flag("verbose", Some('v'), "verbose"),
    OptionSpec::flag("show", Some('n'), "show"),
    OptionSpec::flag("show", None, "dry-run"),
    OptionSpec::value("symbol-prefix", Some('P'), "symbol-prefix"),
    OptionSpec::flag("warn", Some('w'), "warn"),
    OptionSpec::flag("version", Some('V'), "version"),
    OptionSpec::flag("help", Some('h'), "help"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DepmodOption {
    All,
    Quick,
    BaseDirectory(String),
    ModuleDirectory(String),
    OutputDirectory(String),
    Config(String),
    SymbolVersions(String),
    FileSymbols(String),
    ErrorSymbols,
    Verbose,
    Show,
    SymbolPrefix(u8),
    WarnDuplicates,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DepmodAction {
    Help,
    Version,
    Generate {
        kernel_version: Option<String>,
        modules: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepmodInvocation {
    pub contract: UpstreamContract,
    pub action: DepmodAction,
    pub options: Vec<DepmodOption>,
}

impl DepmodInvocation {
    pub fn disposition(&self) -> InvocationDisposition {
        match self.action {
            DepmodAction::Help | DepmodAction::Version => InvocationDisposition::Information,
            DepmodAction::Generate { .. } => InvocationDisposition::Mutation,
        }
    }
}

pub(super) fn parse_depmod(argv: &[&str]) -> Result<DepmodInvocation, BootRuntimeParseError> {
    const COMMAND: &str = "depmod";
    let scanned = scan_gnu(COMMAND, argv, OPTIONS)?;
    let mut options = Vec::new();
    let mut information = None;

    for option in scanned.options {
        let value = || option.values[0].clone();
        match option.id {
            "all" => options.push(DepmodOption::All),
            "quick" => options.push(DepmodOption::Quick),
            "basedir" => options.push(DepmodOption::BaseDirectory(require_nonempty(
                COMMAND,
                &option.spelling,
                value(),
                "a directory path",
            )?)),
            "moduledir" => options.push(DepmodOption::ModuleDirectory(require_nonempty(
                COMMAND,
                &option.spelling,
                value(),
                "a module directory",
            )?)),
            "outdir" => options.push(DepmodOption::OutputDirectory(require_nonempty(
                COMMAND,
                &option.spelling,
                value(),
                "a directory path",
            )?)),
            "config" => options.push(DepmodOption::Config(require_nonempty(
                COMMAND,
                &option.spelling,
                value(),
                "a configuration path",
            )?)),
            "symvers" => options.push(DepmodOption::SymbolVersions(require_nonempty(
                COMMAND,
                &option.spelling,
                value(),
                "a Module.symvers path",
            )?)),
            "filesyms" => options.push(DepmodOption::FileSymbols(require_nonempty(
                COMMAND,
                &option.spelling,
                value(),
                "a System.map path",
            )?)),
            "errsyms" => options.push(DepmodOption::ErrorSymbols),
            "verbose" => options.push(DepmodOption::Verbose),
            "show" => options.push(DepmodOption::Show),
            "symbol-prefix" => {
                let prefix = value();
                if prefix.len() != 1 {
                    return Err(BootRuntimeParseError::InvalidOptionValue {
                        command: COMMAND,
                        option: option.spelling,
                        value: prefix,
                        expected: "exactly one byte",
                    });
                }
                options.push(DepmodOption::SymbolPrefix(prefix.as_bytes()[0]));
            }
            "warn" => options.push(DepmodOption::WarnDuplicates),
            "version" => information = Some(DepmodAction::Version),
            "help" => information = Some(DepmodAction::Help),
            _ => unreachable!("closed depmod option table"),
        }
    }

    let action = if let Some(action) = information {
        action
    } else {
        let (kernel_version, modules) = if scanned.operands.is_empty() {
            (None, Vec::new())
        } else {
            only_operand_count(
                COMMAND,
                "generate",
                &scanned.operands,
                1,
                usize::MAX,
                "a kernel version",
            )?;
            let version = scanned.operands[0].clone();
            if !is_kernel_version(&version) {
                return Err(BootRuntimeParseError::InvalidOptionValue {
                    command: COMMAND,
                    option: "kernel-version".to_string(),
                    value: version,
                    expected: "a version beginning with two unsigned decimal components",
                });
            }
            let modules = scanned.operands[1..].to_vec();
            if let Some(module) = modules.iter().find(|module| !module.starts_with('/')) {
                return Err(BootRuntimeParseError::InvalidOptionValue {
                    command: COMMAND,
                    option: "module".to_string(),
                    value: module.clone(),
                    expected: "an absolute module path",
                });
            }
            (Some(scanned.operands[0].clone()), modules)
        };
        DepmodAction::Generate {
            kernel_version,
            modules,
        }
    };

    Ok(DepmodInvocation {
        contract: DEPMOD_CONTRACT,
        action,
        options,
    })
}

fn is_kernel_version(value: &str) -> bool {
    let bytes = value.as_bytes();
    let first = bytes
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if first == 0 || bytes.get(first) != Some(&b'.') {
        return false;
    }
    bytes[first + 1..]
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count()
        > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_every_declared_option_spelling() {
        let argv = [
            "-a",
            "-A",
            "-b/root",
            "--moduledir=lib/modules",
            "-o",
            "/output",
            "-C/etc/depmod.d",
            "--symvers",
            "/Module.symvers",
            "-F/System.map",
            "-evn",
            "--dry-run",
            "-P_",
            "-w",
            "6.12.1-test",
            "/lib/modules/test.ko",
        ];
        let parsed = parse_depmod(&argv).unwrap();
        assert!(matches!(
            parsed.action,
            DepmodAction::Generate {
                kernel_version: Some(ref version),
                ref modules,
            } if version == "6.12.1-test" && modules == &["/lib/modules/test.ko"]
        ));
        assert_eq!(parsed.options.len(), 14);
    }

    #[test]
    fn accepts_help_and_version_branches() {
        assert!(matches!(
            parse_depmod(&["--help"]).unwrap().action,
            DepmodAction::Help
        ));
        assert!(matches!(
            parse_depmod(&["-V"]).unwrap().action,
            DepmodAction::Version
        ));
    }

    #[test]
    fn rejects_unknown_malformed_and_relative_module_forms() {
        assert!(parse_depmod(&["--future"]).is_err());
        assert!(parse_depmod(&["-Pab"]).is_err());
        assert!(parse_depmod(&["not-a-version"]).is_err());
        assert!(parse_depmod(&["6.12", "relative.ko"]).is_err());
        assert!(parse_depmod(&["--basedir"]).is_err());
    }

    #[test]
    fn option_terminator_preserves_dash_prefixed_operands() {
        assert!(parse_depmod(&["--", "6.12", "-module.ko"]).is_err());
    }
}
