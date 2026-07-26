// conary-core/src/activation/systemd/grammar.rs

//! Single lexical authority shared by typed systemctl parsing and proxy generation.

use super::{SystemdActivationAction, SystemdUnitFileNowAction};

#[derive(Debug, Clone, Copy)]
pub(super) enum ParsedVerb {
    Runtime(SystemdActivationAction),
    UnitFile(SystemdUnitFileNowAction),
}

#[derive(Debug, Clone, Copy)]
struct SystemctlVerbGrammar {
    spelling: &'static str,
    parsed: ParsedVerb,
}

const SYSTEMCTL_VERB_GRAMMAR: &[SystemctlVerbGrammar] = &[
    SystemctlVerbGrammar {
        spelling: "start",
        parsed: ParsedVerb::Runtime(SystemdActivationAction::Start),
    },
    SystemctlVerbGrammar {
        spelling: "stop",
        parsed: ParsedVerb::Runtime(SystemdActivationAction::Stop),
    },
    SystemctlVerbGrammar {
        spelling: "condstop",
        parsed: ParsedVerb::Runtime(SystemdActivationAction::Stop),
    },
    SystemctlVerbGrammar {
        spelling: "reload",
        parsed: ParsedVerb::Runtime(SystemdActivationAction::Reload),
    },
    SystemctlVerbGrammar {
        spelling: "restart",
        parsed: ParsedVerb::Runtime(SystemdActivationAction::Restart),
    },
    SystemctlVerbGrammar {
        spelling: "try-restart",
        parsed: ParsedVerb::Runtime(SystemdActivationAction::TryRestart),
    },
    SystemctlVerbGrammar {
        spelling: "condrestart",
        parsed: ParsedVerb::Runtime(SystemdActivationAction::TryRestart),
    },
    SystemctlVerbGrammar {
        spelling: "reload-or-restart",
        parsed: ParsedVerb::Runtime(SystemdActivationAction::ReloadOrRestart),
    },
    SystemctlVerbGrammar {
        spelling: "try-reload-or-restart",
        parsed: ParsedVerb::Runtime(SystemdActivationAction::ReloadOrTryRestart),
    },
    SystemctlVerbGrammar {
        spelling: "reload-or-try-restart",
        parsed: ParsedVerb::Runtime(SystemdActivationAction::ReloadOrTryRestart),
    },
    SystemctlVerbGrammar {
        spelling: "condreload",
        parsed: ParsedVerb::Runtime(SystemdActivationAction::ReloadOrTryRestart),
    },
    SystemctlVerbGrammar {
        spelling: "force-reload",
        parsed: ParsedVerb::Runtime(SystemdActivationAction::ReloadOrTryRestart),
    },
    SystemctlVerbGrammar {
        spelling: "enable",
        parsed: ParsedVerb::UnitFile(SystemdUnitFileNowAction::Enable),
    },
    SystemctlVerbGrammar {
        spelling: "disable",
        parsed: ParsedVerb::UnitFile(SystemdUnitFileNowAction::Disable),
    },
    SystemctlVerbGrammar {
        spelling: "reenable",
        parsed: ParsedVerb::UnitFile(SystemdUnitFileNowAction::Reenable),
    },
    SystemctlVerbGrammar {
        spelling: "mask",
        parsed: ParsedVerb::UnitFile(SystemdUnitFileNowAction::Mask),
    },
];

impl ParsedVerb {
    pub(super) fn parse(value: &str) -> Option<Self> {
        SYSTEMCTL_VERB_GRAMMAR
            .iter()
            .find_map(|grammar| (grammar.spelling == value).then_some(grammar.parsed))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SystemctlOptionGrammar {
    System,
    All,
    NoBlock,
    NoAskPassword,
    Quiet,
    NoWarn,
    Wait,
    ShowTransaction,
    DryRun,
    Now,
    Fail,
    Irreversible,
    IgnoreDependencies,
    AcceptedNoEffect,
    JobModeValue,
    UnsupportedTarget,
    UnsupportedTargetValue,
}

impl SystemctlOptionGrammar {
    const fn consumes_following_value(self) -> bool {
        matches!(self, Self::JobModeValue | Self::UnsupportedTargetValue)
    }
}

#[derive(Debug, Clone, Copy)]
struct SystemctlExactOptionGrammar {
    spelling: &'static str,
    parsed: SystemctlOptionGrammar,
}

const SYSTEMCTL_EXACT_OPTION_GRAMMAR: &[SystemctlExactOptionGrammar] = &[
    SystemctlExactOptionGrammar {
        spelling: "--system",
        parsed: SystemctlOptionGrammar::System,
    },
    SystemctlExactOptionGrammar {
        spelling: "--all",
        parsed: SystemctlOptionGrammar::All,
    },
    SystemctlExactOptionGrammar {
        spelling: "-a",
        parsed: SystemctlOptionGrammar::All,
    },
    SystemctlExactOptionGrammar {
        spelling: "--no-block",
        parsed: SystemctlOptionGrammar::NoBlock,
    },
    SystemctlExactOptionGrammar {
        spelling: "--no-ask-password",
        parsed: SystemctlOptionGrammar::NoAskPassword,
    },
    SystemctlExactOptionGrammar {
        spelling: "--quiet",
        parsed: SystemctlOptionGrammar::Quiet,
    },
    SystemctlExactOptionGrammar {
        spelling: "-q",
        parsed: SystemctlOptionGrammar::Quiet,
    },
    SystemctlExactOptionGrammar {
        spelling: "--no-warn",
        parsed: SystemctlOptionGrammar::NoWarn,
    },
    SystemctlExactOptionGrammar {
        spelling: "--wait",
        parsed: SystemctlOptionGrammar::Wait,
    },
    SystemctlExactOptionGrammar {
        spelling: "--show-transaction",
        parsed: SystemctlOptionGrammar::ShowTransaction,
    },
    SystemctlExactOptionGrammar {
        spelling: "-T",
        parsed: SystemctlOptionGrammar::ShowTransaction,
    },
    SystemctlExactOptionGrammar {
        spelling: "--dry-run",
        parsed: SystemctlOptionGrammar::DryRun,
    },
    SystemctlExactOptionGrammar {
        spelling: "--now",
        parsed: SystemctlOptionGrammar::Now,
    },
    SystemctlExactOptionGrammar {
        spelling: "--fail",
        parsed: SystemctlOptionGrammar::Fail,
    },
    SystemctlExactOptionGrammar {
        spelling: "--irreversible",
        parsed: SystemctlOptionGrammar::Irreversible,
    },
    SystemctlExactOptionGrammar {
        spelling: "--ignore-dependencies",
        parsed: SystemctlOptionGrammar::IgnoreDependencies,
    },
    SystemctlExactOptionGrammar {
        spelling: "--no-pager",
        parsed: SystemctlOptionGrammar::AcceptedNoEffect,
    },
    SystemctlExactOptionGrammar {
        spelling: "--no-legend",
        parsed: SystemctlOptionGrammar::AcceptedNoEffect,
    },
    SystemctlExactOptionGrammar {
        spelling: "--plain",
        parsed: SystemctlOptionGrammar::AcceptedNoEffect,
    },
    SystemctlExactOptionGrammar {
        spelling: "--full",
        parsed: SystemctlOptionGrammar::AcceptedNoEffect,
    },
    SystemctlExactOptionGrammar {
        spelling: "--no-reload",
        parsed: SystemctlOptionGrammar::AcceptedNoEffect,
    },
    SystemctlExactOptionGrammar {
        spelling: "--runtime",
        parsed: SystemctlOptionGrammar::AcceptedNoEffect,
    },
    SystemctlExactOptionGrammar {
        spelling: "--force",
        parsed: SystemctlOptionGrammar::AcceptedNoEffect,
    },
    SystemctlExactOptionGrammar {
        spelling: "-f",
        parsed: SystemctlOptionGrammar::AcceptedNoEffect,
    },
    SystemctlExactOptionGrammar {
        spelling: "--job-mode",
        parsed: SystemctlOptionGrammar::JobModeValue,
    },
    SystemctlExactOptionGrammar {
        spelling: "--user",
        parsed: SystemctlOptionGrammar::UnsupportedTarget,
    },
    SystemctlExactOptionGrammar {
        spelling: "--global",
        parsed: SystemctlOptionGrammar::UnsupportedTarget,
    },
    SystemctlExactOptionGrammar {
        spelling: "--host",
        parsed: SystemctlOptionGrammar::UnsupportedTargetValue,
    },
    SystemctlExactOptionGrammar {
        spelling: "-H",
        parsed: SystemctlOptionGrammar::UnsupportedTargetValue,
    },
    SystemctlExactOptionGrammar {
        spelling: "--machine",
        parsed: SystemctlOptionGrammar::UnsupportedTargetValue,
    },
    SystemctlExactOptionGrammar {
        spelling: "-M",
        parsed: SystemctlOptionGrammar::UnsupportedTargetValue,
    },
    SystemctlExactOptionGrammar {
        spelling: "--capsule",
        parsed: SystemctlOptionGrammar::UnsupportedTargetValue,
    },
    SystemctlExactOptionGrammar {
        spelling: "-C",
        parsed: SystemctlOptionGrammar::UnsupportedTargetValue,
    },
    SystemctlExactOptionGrammar {
        spelling: "--root",
        parsed: SystemctlOptionGrammar::UnsupportedTargetValue,
    },
    SystemctlExactOptionGrammar {
        spelling: "--image",
        parsed: SystemctlOptionGrammar::UnsupportedTargetValue,
    },
];

#[derive(Debug, Clone, Copy)]
struct SystemctlPrefixedOptionGrammar {
    prefix: &'static str,
    parsed: SystemctlOptionGrammar,
}

const SYSTEMCTL_PREFIXED_OPTION_GRAMMAR: &[SystemctlPrefixedOptionGrammar] = &[
    SystemctlPrefixedOptionGrammar {
        prefix: "--job-mode=",
        parsed: SystemctlOptionGrammar::JobModeValue,
    },
    SystemctlPrefixedOptionGrammar {
        prefix: "--host=",
        parsed: SystemctlOptionGrammar::UnsupportedTarget,
    },
    SystemctlPrefixedOptionGrammar {
        prefix: "--machine=",
        parsed: SystemctlOptionGrammar::UnsupportedTarget,
    },
    SystemctlPrefixedOptionGrammar {
        prefix: "--capsule=",
        parsed: SystemctlOptionGrammar::UnsupportedTarget,
    },
    SystemctlPrefixedOptionGrammar {
        prefix: "--root=",
        parsed: SystemctlOptionGrammar::UnsupportedTarget,
    },
    SystemctlPrefixedOptionGrammar {
        prefix: "--image=",
        parsed: SystemctlOptionGrammar::UnsupportedTarget,
    },
];

pub(super) fn exact_option(token: &str) -> Option<SystemctlOptionGrammar> {
    SYSTEMCTL_EXACT_OPTION_GRAMMAR
        .iter()
        .find_map(|grammar| (grammar.spelling == token).then_some(grammar.parsed))
}

pub(super) fn prefixed_option(token: &str) -> Option<(SystemctlOptionGrammar, &str)> {
    SYSTEMCTL_PREFIXED_OPTION_GRAMMAR
        .iter()
        .find_map(|grammar| {
            token
                .strip_prefix(grammar.prefix)
                .map(|value| (grammar.parsed, value))
        })
}

/// Generate the shell-side conservative scan from the parser's exact grammar.
///
/// The proxy cannot invoke Rust inside the selected root. It therefore uses
/// this generated scan only to decide whether a command must be prevented from
/// reaching the live manager. The parent still parses the captured argv
/// through the typed parser before commit.
pub(crate) fn systemctl_proxy_shell_scan() -> String {
    let runtime_verbs = SYSTEMCTL_VERB_GRAMMAR
        .iter()
        .filter_map(|grammar| {
            matches!(grammar.parsed, ParsedVerb::Runtime(_)).then_some(grammar.spelling)
        })
        .collect::<Vec<_>>()
        .join("|");
    let value_options = SYSTEMCTL_EXACT_OPTION_GRAMMAR
        .iter()
        .filter_map(|grammar| {
            grammar
                .parsed
                .consumes_following_value()
                .then_some(grammar.spelling)
        })
        .collect::<Vec<_>>()
        .join("|");
    let flag_options = SYSTEMCTL_EXACT_OPTION_GRAMMAR
        .iter()
        .filter_map(|grammar| {
            (!grammar.parsed.consumes_following_value()).then_some(grammar.spelling)
        })
        .chain(
            SYSTEMCTL_PREFIXED_OPTION_GRAMMAR
                .iter()
                .map(|grammar| grammar.prefix.trim_end_matches('=')),
        )
        .map(|spelling| {
            if SYSTEMCTL_PREFIXED_OPTION_GRAMMAR
                .iter()
                .any(|grammar| grammar.prefix.trim_end_matches('=') == spelling)
            {
                format!("{spelling}=*")
            } else {
                spelling.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("|");

    format!(
        "conary_positional=false\n\
         conary_option_value=false\n\
         for conary_arg do\n\
             if [ \"$conary_option_value\" = true ]; then\n\
                 conary_option_value=false\n\
                 continue\n\
             fi\n\
             if [ \"$conary_positional\" = true ]; then\n\
                 case \"$conary_arg\" in\n\
                     {runtime_verbs})\n\
                         exit 0\n\
                         ;;\n\
                     *)\n\
                         break\n\
                         ;;\n\
                 esac\n\
             fi\n\
             case \"$conary_arg\" in\n\
                 --)\n\
                     conary_positional=true\n\
                     ;;\n\
                 {value_options})\n\
                     conary_option_value=true\n\
                     ;;\n\
                 {flag_options})\n\
                     ;;\n\
                 {runtime_verbs})\n\
                     exit 0\n\
                     ;;\n\
                 -*)\n\
                     exit 0\n\
                     ;;\n\
                 *)\n\
                     break\n\
                     ;;\n\
             esac\n\
         done\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_scan_contains_every_typed_runtime_verb_and_option_shape() {
        let scan = systemctl_proxy_shell_scan();
        for grammar in SYSTEMCTL_VERB_GRAMMAR {
            if matches!(grammar.parsed, ParsedVerb::Runtime(_)) {
                assert!(scan.contains(grammar.spelling), "{}", grammar.spelling);
            }
        }
        for grammar in SYSTEMCTL_EXACT_OPTION_GRAMMAR {
            assert!(scan.contains(grammar.spelling), "{}", grammar.spelling);
        }
        for grammar in SYSTEMCTL_PREFIXED_OPTION_GRAMMAR {
            assert!(
                scan.contains(&format!("{}*", grammar.prefix)),
                "{}",
                grammar.prefix
            );
        }
    }
}
