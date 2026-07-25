// crates/conary-core/src/boot_runtime/bootloader/grub.rs

use crate::boot_runtime::{BootRuntimeParseError, InvocationDisposition, UpstreamContract};

pub const GRUB_MKCONFIG_CONTRACT: UpstreamContract = UpstreamContract {
    project: "GNU GRUB",
    version: "2.15",
    revision: "b07cc37ca64fe99260884e7325abc333374e098b",
    source_url: "https://git.savannah.gnu.org/git/grub.git",
};

pub const UPDATE_GRUB_CONTRACT: UpstreamContract = UpstreamContract {
    project: "Debian GRUB",
    version: "2.14-3",
    revision: "e86eb68f6bd3a3c4f144b192caf0a7a0994642cf",
    source_url: "https://salsa.debian.org/grub-team/grub",
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrubOption {
    Output(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrubAction {
    Help,
    Version,
    Generate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrubInvocation {
    pub generator_contract: UpstreamContract,
    pub wrapper_contract: Option<UpstreamContract>,
    pub action: GrubAction,
    pub options: Vec<GrubOption>,
    pub effective_output: Option<String>,
    /// GNU GRUB explicitly ignores non-option arguments for compatibility.
    pub compatibility_operands: Vec<String>,
}

impl GrubInvocation {
    pub fn disposition(&self) -> InvocationDisposition {
        match self.action {
            GrubAction::Help | GrubAction::Version => InvocationDisposition::Information,
            GrubAction::Generate => InvocationDisposition::Mutation,
        }
    }
}

pub(super) fn parse_grub_mkconfig(
    command: &str,
    argv: &[&str],
) -> Result<GrubInvocation, BootRuntimeParseError> {
    let command = match command {
        "grub-mkconfig" => "grub-mkconfig",
        "grub2-mkconfig" => "grub2-mkconfig",
        _ => unreachable!("top-level command dispatch is closed"),
    };
    parse(command, argv, None, None)
}

pub(super) fn parse_update_grub(argv: &[&str]) -> Result<GrubInvocation, BootRuntimeParseError> {
    parse(
        "update-grub",
        argv,
        Some(UPDATE_GRUB_CONTRACT),
        Some("/boot/grub/grub.cfg".to_string()),
    )
}

fn parse(
    command: &'static str,
    argv: &[&str],
    wrapper_contract: Option<UpstreamContract>,
    mut effective_output: Option<String>,
) -> Result<GrubInvocation, BootRuntimeParseError> {
    let mut options = Vec::new();
    let mut compatibility_operands = Vec::new();
    let mut information = None;
    let mut index = 0;

    while index < argv.len() {
        let token = argv[index];
        match token {
            "-h" | "--help" => {
                information.get_or_insert(GrubAction::Help);
            }
            "-V" | "--version" => {
                information.get_or_insert(GrubAction::Version);
            }
            "-o" | "--output" => {
                let value = argv.get(index + 1).ok_or_else(|| {
                    BootRuntimeParseError::MissingOptionValue {
                        command,
                        option: token.to_string(),
                        values: 1,
                    }
                })?;
                effective_output = Some((*value).to_string());
                options.push(GrubOption::Output((*value).to_string()));
                index += 1;
            }
            _ if token.starts_with("--output=") => {
                let value = token.strip_prefix("--output=").unwrap_or_default();
                effective_output = Some(value.to_string());
                options.push(GrubOption::Output(value.to_string()));
            }
            _ if token.starts_with('-') => {
                return Err(BootRuntimeParseError::UnknownOption {
                    command,
                    option: token.to_string(),
                });
            }
            _ => compatibility_operands.push(token.to_string()),
        }
        index += 1;
    }

    Ok(GrubInvocation {
        generator_contract: GRUB_MKCONFIG_CONTRACT,
        wrapper_contract,
        action: information.unwrap_or(GrubAction::Generate),
        options,
        effective_output,
        compatibility_operands,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_all_grub_mkconfig_options_and_compatibility_operands() {
        let parsed =
            parse_grub_mkconfig("grub-mkconfig", &["-o", "/one", "--output=/two", "ignored"])
                .unwrap();
        assert_eq!(parsed.action, GrubAction::Generate);
        assert_eq!(parsed.options.len(), 2);
        assert_eq!(parsed.effective_output.as_deref(), Some("/two"));
        assert_eq!(parsed.compatibility_operands, ["ignored"]);

        assert_eq!(
            parse_grub_mkconfig("grub2-mkconfig", &[])
                .unwrap()
                .effective_output,
            None
        );
    }

    #[test]
    fn update_grub_models_the_debian_wrapper_default_and_override() {
        let parsed = parse_update_grub(&[]).unwrap();
        assert_eq!(
            parsed.effective_output.as_deref(),
            Some("/boot/grub/grub.cfg")
        );
        assert_eq!(parsed.wrapper_contract, Some(UPDATE_GRUB_CONTRACT));

        let parsed = parse_update_grub(&["--output=/custom"]).unwrap();
        assert_eq!(parsed.effective_output.as_deref(), Some("/custom"));
    }

    #[test]
    fn covers_help_and_version_branches() {
        assert_eq!(
            parse_grub_mkconfig("grub-mkconfig", &["--help"])
                .unwrap()
                .action,
            GrubAction::Help
        );
        assert_eq!(
            parse_update_grub(&["-V"]).unwrap().action,
            GrubAction::Version
        );
    }

    #[test]
    fn rejects_undeclared_clusters_attached_short_values_and_terminator() {
        assert!(parse_update_grub(&["-o/custom"]).is_err());
        assert!(parse_update_grub(&["-hV"]).is_err());
        assert!(parse_update_grub(&["--"]).is_err());
        assert!(parse_update_grub(&["--future"]).is_err());
        assert!(parse_update_grub(&["-o"]).is_err());
    }
}
