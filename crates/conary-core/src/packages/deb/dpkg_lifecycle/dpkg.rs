// crates/conary-core/src/packages/deb/dpkg_lifecycle/dpkg.rs

#[path = "dpkg/action.rs"]
mod action;
#[path = "dpkg/options.rs"]
mod options;
#[path = "dpkg/render.rs"]
mod render;

use super::common::{
    DPKG, DPKG_QUERY, DpkgLifecycleGrammarError, push_long_value, render_with_operands,
    require_arity,
};
use super::query::DpkgQueryAction;
use action::DpkgNamedAction;
use options::parse_option;
use render::disposition_argv;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DpkgForceMode {
    Force,
    Refuse,
}

/// Every force/refuse value documented by dpkg 1.23.7.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DpkgForceThing {
    All,
    Downgrade,
    ConfigureAny,
    Hold,
    RemoveReinstRequired,
    RemoveProtected,
    RemoveEssential,
    Depends,
    DependsVersion,
    Breaks,
    Conflicts,
    ConfMissing,
    ConfNew,
    ConfOld,
    ConfDefault,
    ConfAsk,
    Overwrite,
    OverwriteDirectory,
    OverwriteDiverted,
    StatoverrideAdd,
    StatoverrideRemove,
    SecurityMac,
    UnsafeIo,
    ScriptChrootless,
    Architecture,
    BadVersion,
    BadPath,
    NotRoot,
    BadVerify,
}

impl DpkgForceThing {
    pub const ALL: [Self; 29] = [
        Self::All,
        Self::Downgrade,
        Self::ConfigureAny,
        Self::Hold,
        Self::RemoveReinstRequired,
        Self::RemoveProtected,
        Self::RemoveEssential,
        Self::Depends,
        Self::DependsVersion,
        Self::Breaks,
        Self::Conflicts,
        Self::ConfMissing,
        Self::ConfNew,
        Self::ConfOld,
        Self::ConfDefault,
        Self::ConfAsk,
        Self::Overwrite,
        Self::OverwriteDirectory,
        Self::OverwriteDiverted,
        Self::StatoverrideAdd,
        Self::StatoverrideRemove,
        Self::SecurityMac,
        Self::UnsafeIo,
        Self::ScriptChrootless,
        Self::Architecture,
        Self::BadVersion,
        Self::BadPath,
        Self::NotRoot,
        Self::BadVerify,
    ];

    pub const fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::All => b"all",
            Self::Downgrade => b"downgrade",
            Self::ConfigureAny => b"configure-any",
            Self::Hold => b"hold",
            Self::RemoveReinstRequired => b"remove-reinstreq",
            Self::RemoveProtected => b"remove-protected",
            Self::RemoveEssential => b"remove-essential",
            Self::Depends => b"depends",
            Self::DependsVersion => b"depends-version",
            Self::Breaks => b"breaks",
            Self::Conflicts => b"conflicts",
            Self::ConfMissing => b"confmiss",
            Self::ConfNew => b"confnew",
            Self::ConfOld => b"confold",
            Self::ConfDefault => b"confdef",
            Self::ConfAsk => b"confask",
            Self::Overwrite => b"overwrite",
            Self::OverwriteDirectory => b"overwrite-dir",
            Self::OverwriteDiverted => b"overwrite-diverted",
            Self::StatoverrideAdd => b"statoverride-add",
            Self::StatoverrideRemove => b"statoverride-remove",
            Self::SecurityMac => b"security-mac",
            Self::UnsafeIo => b"unsafe-io",
            Self::ScriptChrootless => b"script-chrootless",
            Self::Architecture => b"architecture",
            Self::BadVersion => b"bad-version",
            Self::BadPath => b"bad-path",
            Self::NotRoot => b"not-root",
            Self::BadVerify => b"bad-verify",
        }
    }

    fn parse(value: &[u8]) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|thing| thing.as_bytes() == value)
    }
}

/// One source-ordered dpkg option. Order is retained because hooks, filters,
/// root/admin directory overrides, and force toggles are order-sensitive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DpkgOption {
    AbortAfter(u32),
    AutoDeconfigure,
    Debug(u32),
    Force {
        mode: DpkgForceMode,
        things: Vec<DpkgForceThing>,
    },
    IgnoreDepends(Vec<Vec<u8>>),
    NoAct,
    Recursive,
    RefuseDowngrade,
    AdministrativeDirectory(Vec<u8>),
    InstallationDirectory(Vec<u8>),
    Root(Vec<u8>),
    SelectedOnly,
    SkipSameVersion,
    PreInvoke(Vec<u8>),
    PostInvoke(Vec<u8>),
    PathExclude(Vec<u8>),
    PathInclude(Vec<u8>),
    VerifyFormat(Vec<u8>),
    StatusFd(u32),
    StatusLogger(Vec<u8>),
    Log(Vec<u8>),
    Robot,
    NoPager,
    NoDebsig,
    TriggerProcessing(bool),
    Pending,
}

impl DpkgOption {
    fn append_argv(&self, argv: &mut Vec<Vec<u8>>) {
        match self {
            Self::AbortAfter(value) => {
                push_long_value(argv, b"--abort-after", value.to_string().as_bytes());
            }
            Self::AutoDeconfigure => argv.push(b"--auto-deconfigure".to_vec()),
            Self::Debug(value) => {
                push_long_value(argv, b"--debug", format!("{value:o}").as_bytes());
            }
            Self::Force { mode, things } => {
                let prefix = match mode {
                    DpkgForceMode::Force => b"--force-".as_slice(),
                    DpkgForceMode::Refuse => b"--refuse-".as_slice(),
                };
                let mut value = prefix.to_vec();
                for (index, thing) in things.iter().enumerate() {
                    if index > 0 {
                        value.push(b',');
                    }
                    value.extend_from_slice(thing.as_bytes());
                }
                argv.push(value);
            }
            Self::IgnoreDepends(packages) => {
                let mut value = Vec::new();
                for (index, package) in packages.iter().enumerate() {
                    if index > 0 {
                        value.push(b',');
                    }
                    value.extend_from_slice(package);
                }
                push_long_value(argv, b"--ignore-depends", &value);
            }
            Self::NoAct => argv.push(b"--no-act".to_vec()),
            Self::Recursive => argv.push(b"--recursive".to_vec()),
            Self::RefuseDowngrade => argv.push(b"-G".to_vec()),
            Self::AdministrativeDirectory(value) => {
                push_long_value(argv, b"--admindir", value);
            }
            Self::InstallationDirectory(value) => {
                push_long_value(argv, b"--instdir", value);
            }
            Self::Root(value) => push_long_value(argv, b"--root", value),
            Self::SelectedOnly => argv.push(b"--selected-only".to_vec()),
            Self::SkipSameVersion => argv.push(b"--skip-same-version".to_vec()),
            Self::PreInvoke(value) => push_long_value(argv, b"--pre-invoke", value),
            Self::PostInvoke(value) => push_long_value(argv, b"--post-invoke", value),
            Self::PathExclude(value) => push_long_value(argv, b"--path-exclude", value),
            Self::PathInclude(value) => push_long_value(argv, b"--path-include", value),
            Self::VerifyFormat(value) => push_long_value(argv, b"--verify-format", value),
            Self::StatusFd(value) => {
                push_long_value(argv, b"--status-fd", value.to_string().as_bytes());
            }
            Self::StatusLogger(value) => push_long_value(argv, b"--status-logger", value),
            Self::Log(value) => push_long_value(argv, b"--log", value),
            Self::Robot => argv.push(b"--robot".to_vec()),
            Self::NoPager => argv.push(b"--no-pager".to_vec()),
            Self::NoDebsig => argv.push(b"--no-debsig".to_vec()),
            Self::TriggerProcessing(true) => argv.push(b"--triggers".to_vec()),
            Self::TriggerProcessing(false) => argv.push(b"--no-triggers".to_vec()),
            Self::Pending => argv.push(b"--pending".to_vec()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DpkgAssertFeature {
    Help,
    SupportPredepends,
    WorkingEpoch,
    LongFilenames,
    MultiConflictsReplaces,
    MultiArch,
    VersionedProvides,
    ProtectedField,
}

impl DpkgAssertFeature {
    pub const ALL: [Self; 8] = [
        Self::Help,
        Self::SupportPredepends,
        Self::WorkingEpoch,
        Self::LongFilenames,
        Self::MultiConflictsReplaces,
        Self::MultiArch,
        Self::VersionedProvides,
        Self::ProtectedField,
    ];

    pub const fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::Help => b"help",
            Self::SupportPredepends => b"support-predepends",
            Self::WorkingEpoch => b"working-epoch",
            Self::LongFilenames => b"long-filenames",
            Self::MultiConflictsReplaces => b"multi-conrep",
            Self::MultiArch => b"multi-arch",
            Self::VersionedProvides => b"versioned-provides",
            Self::ProtectedField => b"protected-field",
        }
    }

    fn parse(value: &[u8]) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|feature| feature.as_bytes() == value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DpkgValidationKind {
    PackageName,
    TriggerName,
    ArchitectureName,
    Version,
}

impl DpkgValidationKind {
    pub const ALL: [Self; 4] = [
        Self::PackageName,
        Self::TriggerName,
        Self::ArchitectureName,
        Self::Version,
    ];

    pub const fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::PackageName => b"pkgname",
            Self::TriggerName => b"trigname",
            Self::ArchitectureName => b"archname",
            Self::Version => b"version",
        }
    }

    fn parse(value: &[u8]) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_bytes() == value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionRelation {
    Less,
    LessOrEqual,
    Equal,
    NotEqual,
    GreaterOrEqual,
    Greater,
    LessNoEarlier,
    LessOrEqualNoEarlier,
    GreaterOrEqualNoEarlier,
    GreaterNoEarlier,
    ControlLessObsolete,
    ControlStrictLess,
    ControlLessOrEqual,
    ControlEqual,
    ControlGreaterOrEqual,
    ControlStrictGreater,
    ControlGreaterObsolete,
}

impl VersionRelation {
    pub const ALL: [Self; 17] = [
        Self::Less,
        Self::LessOrEqual,
        Self::Equal,
        Self::NotEqual,
        Self::GreaterOrEqual,
        Self::Greater,
        Self::LessNoEarlier,
        Self::LessOrEqualNoEarlier,
        Self::GreaterOrEqualNoEarlier,
        Self::GreaterNoEarlier,
        Self::ControlLessObsolete,
        Self::ControlStrictLess,
        Self::ControlLessOrEqual,
        Self::ControlEqual,
        Self::ControlGreaterOrEqual,
        Self::ControlStrictGreater,
        Self::ControlGreaterObsolete,
    ];

    pub const fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::Less => b"lt",
            Self::LessOrEqual => b"le",
            Self::Equal => b"eq",
            Self::NotEqual => b"ne",
            Self::GreaterOrEqual => b"ge",
            Self::Greater => b"gt",
            Self::LessNoEarlier => b"lt-nl",
            Self::LessOrEqualNoEarlier => b"le-nl",
            Self::GreaterOrEqualNoEarlier => b"ge-nl",
            Self::GreaterNoEarlier => b"gt-nl",
            Self::ControlLessObsolete => b"<",
            Self::ControlStrictLess => b"<<",
            Self::ControlLessOrEqual => b"<=",
            Self::ControlEqual => b"=",
            Self::ControlGreaterOrEqual => b">=",
            Self::ControlStrictGreater => b">>",
            Self::ControlGreaterObsolete => b">",
        }
    }

    pub const fn is_obsolete(self) -> bool {
        matches!(
            self,
            Self::ControlLessObsolete | Self::ControlGreaterObsolete
        )
    }

    fn parse(value: &[u8]) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|relation| relation.as_bytes() == value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DpkgReadOnlyAction {
    Verify(Vec<Vec<u8>>),
    Query(DpkgQueryAction),
    GetSelections(Vec<Vec<u8>>),
    Audit(Vec<Vec<u8>>),
    YetToUnpack,
    PredependencyPackage,
    PrintArchitecture,
    PrintForeignArchitectures,
    Assert(DpkgAssertFeature),
    Validate {
        kind: DpkgValidationKind,
        value: Vec<u8>,
    },
    CompareVersions {
        left: Vec<u8>,
        relation: VersionRelation,
        right: Vec<u8>,
    },
    Help,
    ForceHelp,
    DebugHelp,
    Version,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DpkgMutationClass {
    PackageTransaction,
    TriggerProcessing,
    PackageDatabase,
    ArchitectureConfiguration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DpkgMutationAction {
    Install(Vec<Vec<u8>>),
    Unpack(Vec<Vec<u8>>),
    RecordAvailable(Vec<Vec<u8>>),
    Configure(Vec<Vec<u8>>),
    Remove(Vec<Vec<u8>>),
    Purge(Vec<Vec<u8>>),
    ProcessTriggers(Vec<Vec<u8>>),
    SetSelections,
    ClearSelections,
    UpdateAvailable(Option<Vec<u8>>),
    MergeAvailable(Option<Vec<u8>>),
    ClearAvailable,
    ForgetOldUnavailable,
    AddArchitecture(Vec<u8>),
    RemoveArchitecture(Vec<u8>),
}

impl DpkgMutationAction {
    pub const fn class(&self) -> DpkgMutationClass {
        match self {
            Self::Install(_)
            | Self::Unpack(_)
            | Self::Configure(_)
            | Self::Remove(_)
            | Self::Purge(_) => DpkgMutationClass::PackageTransaction,
            Self::ProcessTriggers(_) => DpkgMutationClass::TriggerProcessing,
            Self::RecordAvailable(_)
            | Self::SetSelections
            | Self::ClearSelections
            | Self::UpdateAvailable(_)
            | Self::MergeAvailable(_)
            | Self::ClearAvailable
            | Self::ForgetOldUnavailable => DpkgMutationClass::PackageDatabase,
            Self::AddArchitecture(_) | Self::RemoveArchitecture(_) => {
                DpkgMutationClass::ArchitectureConfiguration
            }
        }
    }

    pub const fn reads_stdin(&self) -> bool {
        matches!(self, Self::SetSelections)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DpkgArchiveBackendAction {
    Build(Vec<Vec<u8>>),
    Contents(Vec<u8>),
    Control(Vec<Vec<u8>>),
    Extract {
        archive: Vec<u8>,
        directory: Vec<u8>,
    },
    VerboseExtract {
        archive: Vec<u8>,
        directory: Vec<u8>,
    },
    Field(Vec<Vec<u8>>),
    ControlTar(Vec<u8>),
    FilesystemTar(Vec<u8>),
    Info(Vec<Vec<u8>>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DpkgDisposition {
    ReadOnly(DpkgReadOnlyAction),
    RecursiveMutation(DpkgMutationAction),
    ArchiveBackendDelegation(DpkgArchiveBackendAction),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DpkgInvocation {
    pub options: Vec<DpkgOption>,
    pub disposition: DpkgDisposition,
}

impl DpkgInvocation {
    pub fn parse(argv: &[Vec<u8>]) -> Result<Self, DpkgLifecycleGrammarError> {
        let mut options = Vec::new();
        let mut action: Option<Action> = None;
        let mut action_spelling: Option<Vec<u8>> = None;
        let mut index = 0;
        while let Some(argument) = argv.get(index) {
            if argument == b"--" {
                index += 1;
                break;
            }
            if argument.first() != Some(&b'-') || argument == b"-" {
                break;
            }
            if let Some(parsed) = parse_action(argument)? {
                set_action(&mut action, &mut action_spelling, parsed, argument)?;
            } else if argument == b"--force-help" {
                set_action(
                    &mut action,
                    &mut action_spelling,
                    Action::ForceHelp,
                    argument,
                )?;
            } else if argument == b"-Dh" || argument == b"--debug=help" {
                set_action(
                    &mut action,
                    &mut action_spelling,
                    Action::DebugHelp,
                    argument,
                )?;
            } else if let Some(option) = parse_option(argv, &mut index, argument)? {
                options.push(option);
            } else {
                return Err(DpkgLifecycleGrammarError::UnknownOption {
                    tool: DPKG,
                    option: argument.clone(),
                });
            }
            index += 1;
        }
        let action = action.ok_or(DpkgLifecycleGrammarError::MissingAction { tool: DPKG })?;
        let disposition = action.finish(&options, argv[index..].to_vec())?;
        Ok(Self {
            options,
            disposition,
        })
    }

    pub fn argv(&self) -> Vec<Vec<u8>> {
        let mut options = Vec::new();
        for option in &self.options {
            option.append_argv(&mut options);
        }
        let (action, operands) = disposition_argv(&self.disposition);
        render_with_operands(options, &action, &operands)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Action {
    Named(DpkgNamedAction),
    Assert(DpkgAssertFeature),
    Validate(DpkgValidationKind),
    ForceHelp,
    DebugHelp,
}

impl Action {
    fn finish(
        self,
        options: &[DpkgOption],
        operands: Vec<Vec<u8>>,
    ) -> Result<DpkgDisposition, DpkgLifecycleGrammarError> {
        match self {
            Self::Assert(feature) => {
                require_arity(DPKG, "assert", &operands, 0, Some(0), "argument")?;
                Ok(DpkgDisposition::ReadOnly(DpkgReadOnlyAction::Assert(
                    feature,
                )))
            }
            Self::Validate(kind) => {
                require_arity(DPKG, "validate", &operands, 1, Some(1), "value")?;
                Ok(DpkgDisposition::ReadOnly(DpkgReadOnlyAction::Validate {
                    kind,
                    value: operands[0].clone(),
                }))
            }
            Self::ForceHelp => {
                require_arity(DPKG, "force-help", &operands, 0, Some(0), "argument")?;
                Ok(DpkgDisposition::ReadOnly(DpkgReadOnlyAction::ForceHelp))
            }
            Self::DebugHelp => {
                require_arity(DPKG, "debug-help", &operands, 0, Some(0), "argument")?;
                Ok(DpkgDisposition::ReadOnly(DpkgReadOnlyAction::DebugHelp))
            }
            Self::Named(name) => finish_named(name, options, operands),
        }
    }
}

fn parse_action(argument: &[u8]) -> Result<Option<Action>, DpkgLifecycleGrammarError> {
    if let Some(feature) = argument.strip_prefix(b"--assert-") {
        return DpkgAssertFeature::parse(feature)
            .map(Action::Assert)
            .map(Some)
            .ok_or_else(|| DpkgLifecycleGrammarError::UnknownAction {
                tool: DPKG,
                action: argument.to_vec(),
            });
    }
    if let Some(kind) = argument.strip_prefix(b"--validate-") {
        return DpkgValidationKind::parse(kind)
            .map(Action::Validate)
            .map(Some)
            .ok_or_else(|| DpkgLifecycleGrammarError::UnknownAction {
                tool: DPKG,
                action: argument.to_vec(),
            });
    }
    Ok(DpkgNamedAction::parse(argument).map(Action::Named))
}

fn set_action(
    action: &mut Option<Action>,
    spelling: &mut Option<Vec<u8>>,
    parsed: Action,
    argument: &[u8],
) -> Result<(), DpkgLifecycleGrammarError> {
    if let Some(first) = spelling {
        return Err(DpkgLifecycleGrammarError::ConflictingActions {
            tool: DPKG,
            first: first.clone(),
            second: argument.to_vec(),
        });
    }
    *action = Some(parsed);
    *spelling = Some(argument.to_vec());
    Ok(())
}

fn finish_named(
    action: DpkgNamedAction,
    options: &[DpkgOption],
    operands: Vec<Vec<u8>>,
) -> Result<DpkgDisposition, DpkgLifecycleGrammarError> {
    use DpkgDisposition::{ArchiveBackendDelegation, ReadOnly, RecursiveMutation};
    use DpkgNamedAction::*;

    let pending = options
        .iter()
        .any(|option| matches!(option, DpkgOption::Pending));
    let arity = |minimum, maximum, missing| {
        require_arity(DPKG, action.as_str(), &operands, minimum, maximum, missing)
    };
    let query_arity = |minimum, missing| {
        require_arity(
            DPKG_QUERY,
            action.as_str(),
            &operands,
            minimum,
            None,
            missing,
        )
    };
    let package_arity = || {
        if pending {
            arity(0, Some(0), "argument")
        } else {
            arity(1, None, "package")
        }
    };
    Ok(match action {
        Install => {
            arity(1, None, "package archive")?;
            RecursiveMutation(DpkgMutationAction::Install(operands))
        }
        Unpack => {
            arity(1, None, "package archive")?;
            RecursiveMutation(DpkgMutationAction::Unpack(operands))
        }
        RecordAvailable => {
            arity(1, None, "package archive")?;
            RecursiveMutation(DpkgMutationAction::RecordAvailable(operands))
        }
        Configure => {
            package_arity()?;
            RecursiveMutation(DpkgMutationAction::Configure(operands))
        }
        Remove => {
            package_arity()?;
            RecursiveMutation(DpkgMutationAction::Remove(operands))
        }
        Purge => {
            package_arity()?;
            RecursiveMutation(DpkgMutationAction::Purge(operands))
        }
        ProcessTriggers => {
            package_arity()?;
            RecursiveMutation(DpkgMutationAction::ProcessTriggers(operands))
        }
        Verify => {
            arity(0, None, "package")?;
            ReadOnly(DpkgReadOnlyAction::Verify(operands))
        }
        ListFiles => {
            query_arity(1, "package name")?;
            ReadOnly(DpkgReadOnlyAction::Query(DpkgQueryAction::ListFiles {
                packages: operands,
            }))
        }
        Status => {
            query_arity(0, "package name")?;
            ReadOnly(DpkgReadOnlyAction::Query(DpkgQueryAction::Status {
                packages: operands,
            }))
        }
        GetSelections => {
            arity(0, None, "pattern")?;
            ReadOnly(DpkgReadOnlyAction::GetSelections(operands))
        }
        SetSelections => {
            arity(0, Some(0), "argument")?;
            RecursiveMutation(DpkgMutationAction::SetSelections)
        }
        ClearSelections => {
            arity(0, Some(0), "argument")?;
            RecursiveMutation(DpkgMutationAction::ClearSelections)
        }
        PrintAvailable => {
            query_arity(0, "package name")?;
            ReadOnly(DpkgReadOnlyAction::Query(DpkgQueryAction::PrintAvailable {
                packages: operands,
            }))
        }
        UpdateAvailable => {
            arity(0, Some(1), "Packages file")?;
            RecursiveMutation(DpkgMutationAction::UpdateAvailable(
                operands.first().cloned(),
            ))
        }
        MergeAvailable => {
            arity(0, Some(1), "Packages file")?;
            RecursiveMutation(DpkgMutationAction::MergeAvailable(
                operands.first().cloned(),
            ))
        }
        ClearAvailable => {
            arity(0, Some(0), "argument")?;
            RecursiveMutation(DpkgMutationAction::ClearAvailable)
        }
        ForgetOldUnavailable => {
            arity(0, Some(0), "argument")?;
            RecursiveMutation(DpkgMutationAction::ForgetOldUnavailable)
        }
        Audit => {
            arity(0, None, "package")?;
            ReadOnly(DpkgReadOnlyAction::Audit(operands))
        }
        YetToUnpack => {
            arity(0, Some(0), "argument")?;
            ReadOnly(DpkgReadOnlyAction::YetToUnpack)
        }
        List => {
            query_arity(0, "package pattern")?;
            ReadOnly(DpkgReadOnlyAction::Query(DpkgQueryAction::List {
                patterns: operands,
            }))
        }
        Search => {
            query_arity(1, "filename pattern")?;
            ReadOnly(DpkgReadOnlyAction::Query(DpkgQueryAction::Search {
                patterns: operands,
            }))
        }
        AddArchitecture => {
            arity(1, Some(1), "architecture")?;
            RecursiveMutation(DpkgMutationAction::AddArchitecture(operands[0].clone()))
        }
        RemoveArchitecture => {
            arity(1, Some(1), "architecture")?;
            RecursiveMutation(DpkgMutationAction::RemoveArchitecture(operands[0].clone()))
        }
        PrintArchitecture => {
            arity(0, Some(0), "argument")?;
            ReadOnly(DpkgReadOnlyAction::PrintArchitecture)
        }
        PrintForeignArchitectures => {
            arity(0, Some(0), "argument")?;
            ReadOnly(DpkgReadOnlyAction::PrintForeignArchitectures)
        }
        PredependencyPackage => {
            arity(0, Some(0), "argument")?;
            ReadOnly(DpkgReadOnlyAction::PredependencyPackage)
        }
        CompareVersions => {
            arity(3, Some(3), "two versions and relation")?;
            let relation = VersionRelation::parse(&operands[1]).ok_or_else(|| {
                DpkgLifecycleGrammarError::InvalidOperand {
                    tool: DPKG,
                    action: action.as_str(),
                    operand: "relation",
                    value: operands[1].clone(),
                    reason: "is not a dpkg 1.23.7 version relation",
                }
            })?;
            ReadOnly(DpkgReadOnlyAction::CompareVersions {
                left: operands[0].clone(),
                relation,
                right: operands[2].clone(),
            })
        }
        Help => {
            arity(0, Some(0), "argument")?;
            ReadOnly(DpkgReadOnlyAction::Help)
        }
        Version => {
            arity(0, Some(0), "argument")?;
            ReadOnly(DpkgReadOnlyAction::Version)
        }
        Build => {
            arity(1, Some(2), "directory")?;
            ArchiveBackendDelegation(DpkgArchiveBackendAction::Build(operands))
        }
        Contents => {
            arity(1, Some(1), "archive")?;
            ArchiveBackendDelegation(DpkgArchiveBackendAction::Contents(operands[0].clone()))
        }
        Control => {
            arity(1, Some(2), "archive")?;
            ArchiveBackendDelegation(DpkgArchiveBackendAction::Control(operands))
        }
        Info => {
            arity(1, None, "archive")?;
            ArchiveBackendDelegation(DpkgArchiveBackendAction::Info(operands))
        }
        Field => {
            arity(1, None, "archive")?;
            ArchiveBackendDelegation(DpkgArchiveBackendAction::Field(operands))
        }
        Extract => {
            arity(2, Some(2), "archive and directory")?;
            ArchiveBackendDelegation(DpkgArchiveBackendAction::Extract {
                archive: operands[0].clone(),
                directory: operands[1].clone(),
            })
        }
        VerboseExtract => {
            arity(2, Some(2), "archive and directory")?;
            ArchiveBackendDelegation(DpkgArchiveBackendAction::VerboseExtract {
                archive: operands[0].clone(),
                directory: operands[1].clone(),
            })
        }
        ControlTar => {
            arity(1, Some(1), "archive")?;
            ArchiveBackendDelegation(DpkgArchiveBackendAction::ControlTar(operands[0].clone()))
        }
        FilesystemTar => {
            arity(1, Some(1), "archive")?;
            ArchiveBackendDelegation(DpkgArchiveBackendAction::FilesystemTar(operands[0].clone()))
        }
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DpkgValidationStatus {
    Valid = 0,
    InvalidButLax = 1,
    Invalid = 2,
}

impl DpkgValidationStatus {
    pub const fn exit_code(self) -> u8 {
        self as u8
    }

    pub fn from_exit_code(status: u8) -> Result<Self, DpkgLifecycleGrammarError> {
        match status {
            0 => Ok(Self::Valid),
            1 => Ok(Self::InvalidButLax),
            2 => Ok(Self::Invalid),
            status => Err(DpkgLifecycleGrammarError::UnsupportedExitStatus { tool: DPKG, status }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DpkgAssertionStatus {
    Supported = 0,
    KnownUnsupported = 1,
    Unknown = 2,
}

impl DpkgAssertionStatus {
    pub const fn exit_code(self) -> u8 {
        self as u8
    }

    pub fn from_exit_code(status: u8) -> Result<Self, DpkgLifecycleGrammarError> {
        match status {
            0 => Ok(Self::Supported),
            1 => Ok(Self::KnownUnsupported),
            2 => Ok(Self::Unknown),
            status => Err(DpkgLifecycleGrammarError::UnsupportedExitStatus { tool: DPKG, status }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DpkgComparisonStatus {
    Satisfied = 0,
    Unsatisfied = 1,
    Error = 2,
}

impl DpkgComparisonStatus {
    pub const fn exit_code(self) -> u8 {
        self as u8
    }

    pub fn from_exit_code(status: u8) -> Result<Self, DpkgLifecycleGrammarError> {
        match status {
            0 => Ok(Self::Satisfied),
            1 => Ok(Self::Unsatisfied),
            2 => Ok(Self::Error),
            status => Err(DpkgLifecycleGrammarError::UnsupportedExitStatus { tool: DPKG, status }),
        }
    }
}
