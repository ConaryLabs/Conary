// conary-core/src/ccs/convert/debian_adapters.rs

use crate::ccs::convert::adapters::{AdapterInput, ScriptletEffectAdapter};
use crate::ccs::convert::command_evidence::CommandEvidenceSource;
use crate::ccs::convert::effects::{
    ScriptletClassification, ScriptletCommandEvidence, ScriptletEffectEvidence,
};
use crate::ccs::legacy_scriptlets::{
    CommandArgumentProvenance, CommandExecutionContext, EffectConfidence, EffectReplacement,
    EffectSource,
};
use std::collections::BTreeMap;

const COMPLETE_REASON: &str = "helper-complete-deb-systemd-helper-unit-state";
const REVIEW_REASON: &str = "helper-review-deb-systemd-helper-state";
const MAINTSCRIPT_COMPLETE_REASON: &str = "helper-complete-dpkg-maintscript-transition";
const MAINTSCRIPT_REVIEW_REASON: &str = "helper-review-dpkg-maintscript-transition";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DebSystemdHelperAction {
    Enable,
    Disable,
    Purge,
    Mask,
    Unmask,
    IsEnabled,
    WasEnabled,
    DebianInstalled,
    UpdateState,
    Reenable,
}

impl DebSystemdHelperAction {
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "enable" => Self::Enable,
            "disable" => Self::Disable,
            "purge" => Self::Purge,
            "mask" => Self::Mask,
            "unmask" => Self::Unmask,
            "is-enabled" => Self::IsEnabled,
            "was-enabled" => Self::WasEnabled,
            "debian-installed" => Self::DebianInstalled,
            "update-state" => Self::UpdateState,
            "reenable" => Self::Reenable,
            _ => return None,
        })
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Enable => "enable",
            Self::Disable => "disable",
            Self::Purge => "purge",
            Self::Mask => "mask",
            Self::Unmask => "unmask",
            Self::IsEnabled => "is-enabled",
            Self::WasEnabled => "was-enabled",
            Self::DebianInstalled => "debian-installed",
            Self::UpdateState => "update-state",
            Self::Reenable => "reenable",
        }
    }

    fn state_model(self) -> &'static str {
        match self {
            Self::Enable => "first-enable-state-file",
            Self::Disable => "enablement-state-update",
            Self::Purge => "state-file-purge",
            Self::Mask => "mask-state-save",
            Self::Unmask => "mask-state-restore",
            Self::IsEnabled => "enablement-query",
            Self::WasEnabled => "previous-enablement-query",
            Self::DebianInstalled => "state-file-presence-query",
            Self::UpdateState => "state-file-reconcile",
            Self::Reenable => "reenable-from-recorded-state",
        }
    }

    fn can_be_complete(self) -> bool {
        matches!(self, Self::Enable | Self::Disable)
    }
}

#[derive(Debug, Clone)]
struct DebSystemdHelperInvocation {
    action: DebSystemdHelperAction,
    units: Vec<String>,
}

impl DebSystemdHelperInvocation {
    fn parse(input: AdapterInput<'_>) -> Option<Self> {
        if input.invocation.command != "deb-systemd-helper" {
            return None;
        }

        let action = input
            .invocation
            .argv
            .first()
            .and_then(|arg| DebSystemdHelperAction::parse(arg))?;
        let units = input
            .invocation
            .argv
            .iter()
            .skip(1)
            .cloned()
            .collect::<Vec<_>>();
        if units.is_empty() || units.iter().any(|unit| unit.starts_with('-')) {
            return None;
        }

        Some(Self { action, units })
    }

    fn all_units_are_packaged(&self, input: AdapterInput<'_>) -> bool {
        self.units
            .iter()
            .all(|unit| input.payload.systemd_units.contains(unit))
    }
}

pub(super) struct DebSystemdHelperAdapter;
pub(super) struct DpkgMaintscriptHelperAdapter;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DpkgMaintscriptAction {
    RemoveConffile,
    MoveConffile,
    SymlinkToDirectory,
    DirectoryToSymlink,
}

impl DpkgMaintscriptAction {
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "rm_conffile" => Self::RemoveConffile,
            "mv_conffile" => Self::MoveConffile,
            "symlink_to_dir" => Self::SymlinkToDirectory,
            "dir_to_symlink" => Self::DirectoryToSymlink,
            _ => return None,
        })
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::RemoveConffile => "rm-conffile",
            Self::MoveConffile => "move-conffile",
            Self::SymlinkToDirectory => "symlink-to-directory",
            Self::DirectoryToSymlink => "directory-to-symlink",
        }
    }
}

#[derive(Debug, Clone)]
struct DpkgMaintscriptInvocation {
    action: DpkgMaintscriptAction,
    paths: Vec<String>,
    prior_version: Option<String>,
    package: Option<String>,
}

impl DpkgMaintscriptInvocation {
    fn parse(input: AdapterInput<'_>) -> Option<Self> {
        let invocation = input.invocation;
        if invocation.command != "dpkg-maintscript-helper"
            || invocation.command_provenance != CommandArgumentProvenance::Literal
            || invocation.execution_context != CommandExecutionContext::Unconditional
            || invocation.argv.len() != invocation.argument_provenance.len()
        {
            return None;
        }
        let separator = invocation.argv.iter().position(|arg| arg == "--")?;
        if separator + 2 != invocation.argv.len()
            || invocation.argument_provenance[separator] != CommandArgumentProvenance::Literal
            || invocation.argument_provenance[separator + 1] != CommandArgumentProvenance::Expansion
            || !is_forwarded_maintscript_argv(&invocation.argv[separator + 1])
            || invocation.argument_provenance[..separator]
                .iter()
                .any(|provenance| *provenance != CommandArgumentProvenance::Literal)
        {
            return None;
        }

        let args = &invocation.argv[..separator];
        let action = DpkgMaintscriptAction::parse(args.first()?.as_str())?;
        let required_paths = match action {
            DpkgMaintscriptAction::RemoveConffile => 1,
            DpkgMaintscriptAction::MoveConffile
            | DpkgMaintscriptAction::SymlinkToDirectory
            | DpkgMaintscriptAction::DirectoryToSymlink => 2,
        };
        if args.len() < required_paths + 1 || args.len() > required_paths + 3 {
            return None;
        }
        let paths = args[1..=required_paths].to_vec();
        let path_contract_valid = match action {
            DpkgMaintscriptAction::RemoveConffile => is_absolute_normalized_path(&paths[0]),
            DpkgMaintscriptAction::MoveConffile => {
                paths.iter().all(|path| is_absolute_normalized_path(path))
            }
            DpkgMaintscriptAction::SymlinkToDirectory
            | DpkgMaintscriptAction::DirectoryToSymlink => {
                is_absolute_normalized_path(&paths[0]) && is_valid_symlink_target(&paths[1])
            }
        };
        if !path_contract_valid {
            return None;
        }
        let prior_version = args.get(required_paths + 1).cloned();
        let package = args.get(required_paths + 2).cloned();
        if package.as_deref().is_some_and(|package| {
            !input.payload.package_name.as_deref().is_some_and(|name| {
                package == name
                    || package
                        .strip_prefix(name)
                        .and_then(|rest| rest.strip_prefix(':'))
                        .is_some_and(|architecture| {
                            !architecture.is_empty() && !architecture.contains(':')
                        })
            })
        }) {
            return None;
        }

        Some(Self {
            action,
            paths,
            prior_version,
            package,
        })
    }

    fn native_replacement_model(&self, input: AdapterInput<'_>) -> Option<&'static str> {
        match self.action {
            DpkgMaintscriptAction::RemoveConffile
                if !input.payload.payload_paths.contains(&self.paths[0]) =>
            {
                Some("generation-etc-orphan-preservation")
            }
            DpkgMaintscriptAction::MoveConffile
            | DpkgMaintscriptAction::RemoveConffile
            | DpkgMaintscriptAction::SymlinkToDirectory
            | DpkgMaintscriptAction::DirectoryToSymlink => None,
        }
    }
}

impl ScriptletEffectAdapter for DpkgMaintscriptHelperAdapter {
    fn id(&self) -> &'static str {
        "dpkg-maintscript-helper/v1"
    }

    fn digest(&self) -> String {
        crate::hash::sha256_prefixed(
            b"dpkg-maintscript-helper/v1:official-four-actions:typed-argv:payload-validated",
        )
    }

    fn command_names(&self) -> &'static [&'static str] {
        &["dpkg-maintscript-helper"]
    }

    fn matches(&self, input: AdapterInput<'_>) -> bool {
        input.invocation.command == "dpkg-maintscript-helper"
    }

    fn classify(&self, input: AdapterInput<'_>) -> ScriptletClassification {
        let Some(helper) = DpkgMaintscriptInvocation::parse(input) else {
            return ScriptletClassification::Review {
                reason_code: "review-class-dpkg-maintscript-helper-grammar".to_string(),
                class_id: Some("dpkg-maintscript-helper-grammar".to_string()),
                command: Some(ScriptletCommandEvidence::from_invocation(input.invocation)),
            };
        };
        let native_model = helper.native_replacement_model(input);
        let replacement = if native_model.is_some() {
            EffectReplacement::Complete
        } else {
            EffectReplacement::Partial
        };
        let reason_code = if native_model.is_some() {
            MAINTSCRIPT_COMPLETE_REASON
        } else {
            MAINTSCRIPT_REVIEW_REASON
        };
        let mut extra = BTreeMap::from([
            (
                "action".to_string(),
                toml::Value::String(helper.action.as_str().to_string()),
            ),
            (
                "paths".to_string(),
                toml::Value::Array(
                    helper
                        .paths
                        .iter()
                        .cloned()
                        .map(toml::Value::String)
                        .collect(),
                ),
            ),
            (
                "official_argv_contract".to_string(),
                toml::Value::Boolean(true),
            ),
            (
                "maintscript_argv_forwarded".to_string(),
                toml::Value::Boolean(true),
            ),
        ]);
        if let Some(version) = &helper.prior_version {
            extra.insert(
                "prior_version".to_string(),
                toml::Value::String(version.clone()),
            );
        }
        if let Some(package) = &helper.package {
            extra.insert("package".to_string(), toml::Value::String(package.clone()));
        }
        if let Some(model) = native_model {
            extra.insert(
                "native_replacement_model".to_string(),
                toml::Value::String(model.to_string()),
            );
        } else {
            extra.insert(
                "missing_native_model".to_string(),
                toml::Value::String(
                    match helper.action {
                        DpkgMaintscriptAction::MoveConffile => "config-path-identity-migration",
                        _ => "payload-final-state-proof",
                    }
                    .to_string(),
                ),
            );
        }

        ScriptletClassification::Known {
            reason_code: reason_code.to_string(),
            effects: vec![ScriptletEffectEvidence {
                kind: "debian-maintscript-transition".to_string(),
                source: effect_source(input.invocation.source),
                confidence: EffectConfidence::Declared,
                replacement,
                adapter_id: Some(self.id().to_string()),
                adapter_digest: Some(self.digest()),
                command: Some(input.invocation.command.clone()),
                args: input.invocation.argv.clone(),
                path: helper.paths.first().cloned(),
                reason_code: Some(reason_code.to_string()),
                extra,
            }],
        }
    }

    fn is_authoritative(&self, input: AdapterInput<'_>) -> bool {
        DpkgMaintscriptInvocation::parse(input).is_some()
    }
}

fn is_forwarded_maintscript_argv(value: &str) -> bool {
    value.trim() == "\"$@\""
}

fn is_absolute_normalized_path(value: &str) -> bool {
    let path = std::path::Path::new(value);
    path.is_absolute()
        && !path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
        && !value.contains('\0')
}

fn is_valid_symlink_target(value: &str) -> bool {
    !value.is_empty() && !value.contains('\0')
}

impl ScriptletEffectAdapter for DebSystemdHelperAdapter {
    fn id(&self) -> &'static str {
        "deb-systemd-helper/v1"
    }

    fn digest(&self) -> String {
        crate::hash::sha256_prefixed(b"deb-systemd-helper/v1:documented-actions:state-semantics")
    }

    fn command_names(&self) -> &'static [&'static str] {
        &["deb-systemd-helper"]
    }

    fn matches(&self, input: AdapterInput<'_>) -> bool {
        DebSystemdHelperInvocation::parse(input).is_some()
    }

    fn classify(&self, input: AdapterInput<'_>) -> ScriptletClassification {
        let helper = DebSystemdHelperInvocation::parse(input)
            .expect("matches() must ensure deb-systemd-helper invocation");
        let payload_backed = helper.all_units_are_packaged(input);
        let complete = helper.action.can_be_complete() && payload_backed;
        let replacement = if complete {
            EffectReplacement::Complete
        } else {
            EffectReplacement::Partial
        };
        let reason_code = if complete {
            COMPLETE_REASON
        } else {
            REVIEW_REASON
        };

        let mut extra = BTreeMap::from([
            (
                "debian_helper_action".to_string(),
                toml::Value::String(helper.action.as_str().to_string()),
            ),
            (
                "units".to_string(),
                toml::Value::Array(
                    helper
                        .units
                        .iter()
                        .cloned()
                        .map(toml::Value::String)
                        .collect(),
                ),
            ),
            (
                "state_model".to_string(),
                toml::Value::String(helper.action.state_model().to_string()),
            ),
            (
                "payload_backed".to_string(),
                toml::Value::Boolean(payload_backed),
            ),
            ("documented_action".to_string(), toml::Value::Boolean(true)),
            ("maintscript_only".to_string(), toml::Value::Boolean(true)),
            ("dpkg_root_aware".to_string(), toml::Value::Boolean(true)),
        ]);
        if !complete {
            extra.insert("review_required".to_string(), toml::Value::Boolean(true));
        }

        ScriptletClassification::Known {
            reason_code: reason_code.to_string(),
            effects: vec![ScriptletEffectEvidence {
                kind: "debian-systemd-helper-state".to_string(),
                source: effect_source(input.invocation.source),
                confidence: EffectConfidence::Inferred,
                replacement,
                adapter_id: Some(self.id().to_string()),
                adapter_digest: Some(self.digest()),
                command: Some(input.invocation.command.clone()),
                args: input.invocation.argv.clone(),
                path: helper.units.first().cloned(),
                reason_code: Some(reason_code.to_string()),
                extra,
            }],
        }
    }
}

fn effect_source(source: CommandEvidenceSource) -> EffectSource {
    match source {
        CommandEvidenceSource::ShellAst => EffectSource::ShellAst,
        CommandEvidenceSource::NativeShellAst => EffectSource::NativeShellAst,
        CommandEvidenceSource::PackageMetadata => EffectSource::PackageMetadata,
        CommandEvidenceSource::HelperGrammar => EffectSource::HelperGrammar,
        CommandEvidenceSource::Unresolved => EffectSource::Unknown("unresolved".to_string()),
    }
}
