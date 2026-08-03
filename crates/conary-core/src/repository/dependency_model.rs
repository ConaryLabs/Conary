// conary-core/src/repository/dependency_model.rs

//! Native repository dependency model.
//!
//! Normalized representation of dependencies and capabilities as they appear in
//! RPM, Debian, and Arch repository metadata.  The types here capture the full
//! native semantics (alternatives, conditional markers, separate provide
//! versions) without collapsing them into a lowest-common-denominator string.
//!
//! Design principles:
//! - Preserve native ecosystem semantics first, normalize into one Conary model
//!   second.
//! - Do not silently downgrade conditional or rich dependencies to unconditional
//!   package requirements.
//! - Treat alternatives (`A | B`) as first-class groups, not flattened strings.

use super::versioning::VersionScheme;
use serde::{Deserialize, Serialize};

/// Source package format that established a signed or persisted capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourcePackageFormat {
    Rpm,
    Debian,
    Alpm,
    Ccs,
}

impl SourcePackageFormat {
    #[must_use]
    pub const fn version_scheme(self) -> VersionScheme {
        match self {
            Self::Rpm => VersionScheme::Rpm,
            Self::Debian => VersionScheme::Debian,
            Self::Alpm => VersionScheme::Arch,
            Self::Ccs => VersionScheme::Conary,
        }
    }
}

/// Why a capability exists. Exact package identity is deliberately distinct
/// from every declared or derived capability.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "kebab-case")]
pub enum CapabilityProvenance {
    ExactIdentity,
    AuthorDeclared,
    SourceDeclared {
        format: SourcePackageFormat,
        record_index: u32,
    },
    SourceDerivedFile {
        format: SourcePackageFormat,
        source_path: String,
    },
}

// ---------------------------------------------------------------------------
// Exact source profile
// ---------------------------------------------------------------------------

/// Which native ecosystem a dependency originates from.
///
/// This is intentionally parallel to [`VersionScheme`] but is used to tag
/// dependency entries rather than version strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum RepositoryDependencyFlavor {
    /// Conary-native package metadata and version ordering.
    Conary,
    /// RPM-based (Fedora, RHEL, openSUSE, etc.)
    Rpm,
    /// Debian-based (Debian, Ubuntu, etc.)
    Deb,
    /// Arch Linux / ALPM-based
    Arch,
}

impl RepositoryDependencyFlavor {
    /// Return the corresponding version comparison scheme.
    #[must_use]
    pub fn version_scheme(self) -> VersionScheme {
        match self {
            Self::Conary => VersionScheme::Conary,
            Self::Rpm => VersionScheme::Rpm,
            Self::Deb => VersionScheme::Debian,
            Self::Arch => VersionScheme::Arch,
        }
    }
}

// ---------------------------------------------------------------------------
// Capability kinds
// ---------------------------------------------------------------------------

/// What kind of thing a repository package provides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RepositoryCapabilityKind {
    /// The package name itself (always implicitly provided).
    PackageName,
    /// A virtual capability (e.g. `mail-transport-agent`, `java-runtime`).
    Virtual,
    /// A shared-library soname (e.g. `libc.so.6()(64bit)`).
    Soname,
    /// A filesystem path (e.g. `/usr/bin/python3`).
    File,
    /// A distinct path capability.
    Path,
    /// An executable name capability.
    Binary,
    /// A pkg-config module capability.
    PkgConfig,
    /// Anything that does not fit the above categories.
    Generic,
}

// ---------------------------------------------------------------------------
// Requirement kinds
// ---------------------------------------------------------------------------

/// The relationship expressed by a dependency entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RepositoryRequirementKind {
    /// Hard runtime dependency (RPM Requires, Debian Depends).
    Depends,
    /// Source-defined strong installation ordering (Debian Pre-Depends or an
    /// RPM install prerequirement).
    PreDepends,
    /// Optional / recommended (RPM Suggests, Debian Recommends, Arch optdepends).
    Optional,
    /// Build-time only dependency (RPM BuildRequires, Debian Build-Depends).
    Build,
    /// Mutual exclusion (RPM Conflicts, Debian Conflicts).
    Conflict,
    /// Partial breakage declaration (Debian Breaks).
    Breaks,
    /// Ownership replacement (Debian/Arch Replaces).
    Replace,
    /// Package obsolescence with replacement (RPM Obsoletes).
    Obsolete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageRelationRemovalMode {
    /// Remove a package solely to satisfy a co-installation constraint.
    Constraint,
    /// Transfer payload ownership from a replaced/obsolete package.
    OwnershipTransfer,
}

impl RepositoryRequirementKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Depends => "depends",
            Self::PreDepends => "pre_depends",
            Self::Optional => "optional",
            Self::Build => "build",
            Self::Conflict => "conflict",
            Self::Breaks => "breaks",
            Self::Replace => "replace",
            Self::Obsolete => "obsolete",
        }
    }

    #[must_use]
    pub fn from_str_exact(value: &str) -> Option<Self> {
        match value {
            "depends" => Some(Self::Depends),
            "pre_depends" => Some(Self::PreDepends),
            "optional" => Some(Self::Optional),
            "build" => Some(Self::Build),
            "conflict" => Some(Self::Conflict),
            "breaks" => Some(Self::Breaks),
            "replace" => Some(Self::Replace),
            "obsolete" => Some(Self::Obsolete),
            _ => None,
        }
    }

    #[must_use]
    pub const fn is_negative_relation(self) -> bool {
        matches!(
            self,
            Self::Conflict | Self::Breaks | Self::Replace | Self::Obsolete
        )
    }

    #[must_use]
    pub const fn removes_matching_packages(self) -> bool {
        matches!(self, Self::Replace | Self::Obsolete)
    }

    #[must_use]
    pub const fn removal_mode(self) -> Option<PackageRelationRemovalMode> {
        match self {
            Self::Conflict => Some(PackageRelationRemovalMode::Constraint),
            Self::Replace | Self::Obsolete => Some(PackageRelationRemovalMode::OwnershipTransfer),
            Self::Depends | Self::PreDepends | Self::Optional | Self::Build | Self::Breaks => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Conditional / rich dependency behavior
// ---------------------------------------------------------------------------

/// Diagnostic shape of a parsed dependency expression.
///
/// Resolution uses [`RepositoryRequirementExpression`] directly. This label is
/// never an execution or admission decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ConditionalRequirementBehavior {
    /// Unconditional hard requirement (the common case).
    Hard,
    /// Conditional on a boolean predicate (RPM rich deps `(foo if bar)`).
    Conditional,
}

// ---------------------------------------------------------------------------
// Provides
// ---------------------------------------------------------------------------

/// Architecture carried by one source-native provided capability.
///
/// Debian `Provides` atoms may omit a qualifier, advertise the wildcard
/// `:any`, or name one exact architecture. This is distinct from a dependency
/// qualifier: a literal `:native` in `Provides` is an exact architecture token,
/// not the dependency-side native-architecture selector.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "architecture", rename_all = "snake_case")]
pub enum ProvideArchitectureQualifier {
    /// Inherit the providing package's architecture.
    #[default]
    Implicit,
    /// Debian's explicit wildcard `:any` provide.
    Any,
    /// One exact source architecture token.
    Exact(String),
}

/// Source-native relation attached to a versioned provided capability.
///
/// RPM permits all five ordered relations on `Provides`; Debian and Arch
/// currently emit only [`Self::Equal`]. An unversioned existence provide is
/// represented by no relation and no version, never by an `Any` range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProvideVersionRelation {
    LessThan,
    LessOrEqual,
    Equal,
    GreaterOrEqual,
    GreaterThan,
}

/// Capability projected for dependency resolution, persistence, or signing.
/// Native parsers retain their source-specific records and construct this DTO
/// only at a named consumer boundary.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvidedCapability {
    pub kind: RepositoryCapabilityKind,
    pub name: String,
    pub version: Option<String>,
    pub version_relation: Option<ProvideVersionRelation>,
    pub version_scheme: VersionScheme,
    pub architecture_qualifier: ProvideArchitectureQualifier,
    pub provenance: CapabilityProvenance,
}

impl ProvidedCapability {
    pub fn validate(&self) -> crate::error::Result<()> {
        if self.name.trim().is_empty() {
            return Err(crate::error::Error::ParseError(
                "provided capability name is empty".to_string(),
            ));
        }
        if self.version.is_some() != self.version_relation.is_some() {
            return Err(crate::error::Error::ParseError(format!(
                "provided capability '{}' must carry both a version relation and boundary, or neither",
                self.name
            )));
        }
        if let Some(version) = self.version.as_deref() {
            super::versioning::validate_repo_version(self.version_scheme, version).map_err(
                |error| {
                    crate::error::Error::ParseError(format!(
                        "provided capability '{}' has invalid {} provider version: {error}",
                        self.name,
                        self.version_scheme.as_str()
                    ))
                },
            )?;
        }
        if self.version_scheme != VersionScheme::Rpm
            && self
                .version_relation
                .is_some_and(|relation| relation != ProvideVersionRelation::Equal)
        {
            return Err(crate::error::Error::ParseError(format!(
                "non-RPM provided capability '{}' may only carry an exact version relation",
                self.name
            )));
        }
        if matches!(
            &self.architecture_qualifier,
            ProvideArchitectureQualifier::Exact(architecture) if architecture.trim().is_empty()
        ) {
            return Err(crate::error::Error::ParseError(format!(
                "provided capability '{}' has an empty exact architecture qualifier",
                self.name
            )));
        }
        match &self.provenance {
            CapabilityProvenance::ExactIdentity
                if self.kind != RepositoryCapabilityKind::PackageName
                    || self.version_relation != Some(ProvideVersionRelation::Equal)
                    || self.architecture_qualifier != ProvideArchitectureQualifier::Implicit =>
            {
                return Err(crate::error::Error::ParseError(
                    "exact package identity must be an exact, implicit-architecture package-name provider"
                        .to_string(),
                ));
            }
            CapabilityProvenance::SourceDeclared { format, .. }
            | CapabilityProvenance::SourceDerivedFile { format, .. }
                if format.version_scheme() != self.version_scheme =>
            {
                return Err(crate::error::Error::ParseError(format!(
                    "provided capability '{}' source format contradicts its version scheme",
                    self.name
                )));
            }
            CapabilityProvenance::SourceDerivedFile { source_path, .. }
                if self.kind != RepositoryCapabilityKind::File || !source_path.starts_with('/') =>
            {
                return Err(crate::error::Error::ParseError(
                    "source-derived file capability requires a file kind and absolute source path"
                        .to_string(),
                ));
            }
            _ => {}
        }
        Ok(())
    }
}

impl ProvideVersionRelation {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LessThan => "lt",
            Self::LessOrEqual => "le",
            Self::Equal => "eq",
            Self::GreaterOrEqual => "ge",
            Self::GreaterThan => "gt",
        }
    }

    pub fn parse_exact(value: &str) -> Result<Self, String> {
        match value {
            "lt" => Ok(Self::LessThan),
            "le" => Ok(Self::LessOrEqual),
            "eq" => Ok(Self::Equal),
            "ge" => Ok(Self::GreaterOrEqual),
            "gt" => Ok(Self::GreaterThan),
            _ => Err(format!(
                "unsupported persisted provider version relation '{value}'"
            )),
        }
    }
}

/// A single capability that a repository package advertises.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RepositoryProvide {
    /// The capability name (e.g. package name, virtual, soname).
    pub name: String,

    /// What kind of capability this is.
    pub kind: RepositoryCapabilityKind,

    /// Optional version of the *provide itself* (e.g. `= 6.19.6`).
    ///
    /// This is separate from the package version -- a package at version 2.0
    /// may provide `libfoo.so.1` with provide-version `1.3`.
    pub version: Option<String>,

    /// Ordered relation associated with `version`.
    ///
    /// This is paired with `version`: both are absent for an existence provide,
    /// and both are present for a versioned range.
    pub version_relation: Option<ProvideVersionRelation>,

    /// Exact source-native architecture of this provided capability.
    pub architecture_qualifier: ProvideArchitectureQualifier,

    /// The original native text for diagnostics (e.g. `"kernel-core-uname-r = 6.19.6-200.fc44.x86_64"`).
    pub native_text: Option<String>,

    /// Consumer-visible origin of this provider row.
    pub provenance: CapabilityProvenance,
}

impl RepositoryProvide {
    /// Create a provide for the package name itself (always present).
    #[must_use]
    pub fn package_name(name: String, version: Option<String>) -> Self {
        let version_relation = version.as_ref().map(|_| ProvideVersionRelation::Equal);
        Self {
            name,
            kind: RepositoryCapabilityKind::PackageName,
            version,
            version_relation,
            architecture_qualifier: ProvideArchitectureQualifier::Implicit,
            native_text: None,
            provenance: CapabilityProvenance::ExactIdentity,
        }
    }

    /// Create a virtual provide.
    #[must_use]
    pub fn virtual_cap(name: String, version: Option<String>) -> Self {
        let version_relation = version.as_ref().map(|_| ProvideVersionRelation::Equal);
        Self {
            name,
            kind: RepositoryCapabilityKind::Virtual,
            version,
            version_relation,
            architecture_qualifier: ProvideArchitectureQualifier::Implicit,
            native_text: None,
            provenance: CapabilityProvenance::AuthorDeclared,
        }
    }

    /// Create a soname provide.
    #[must_use]
    pub fn soname(name: String, version: Option<String>) -> Self {
        let version_relation = version.as_ref().map(|_| ProvideVersionRelation::Equal);
        Self {
            name,
            kind: RepositoryCapabilityKind::Soname,
            version,
            version_relation,
            architecture_qualifier: ProvideArchitectureQualifier::Implicit,
            native_text: None,
            provenance: CapabilityProvenance::AuthorDeclared,
        }
    }

    /// Create a file provide.
    #[must_use]
    pub fn file(path: String) -> Self {
        Self {
            name: path,
            kind: RepositoryCapabilityKind::File,
            version: None,
            version_relation: None,
            architecture_qualifier: ProvideArchitectureQualifier::Implicit,
            native_text: None,
            provenance: CapabilityProvenance::AuthorDeclared,
        }
    }
}

// ---------------------------------------------------------------------------
// Requirement clauses and groups
// ---------------------------------------------------------------------------

/// Architecture qualifier attached to one source-native dependency atom.
///
/// Debian dependency names may be unqualified, use the special `:any` or
/// `:native` qualifiers, or name one exact Debian architecture. The parsed
/// qualifier remains separate from the package name so later layers never
/// have to recover mutation authority from string suffixes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "architecture", rename_all = "snake_case")]
pub enum RequirementArchitectureQualifier {
    /// No source-native architecture qualifier was written.
    #[default]
    Unqualified,
    /// Debian `:any`.
    Any,
    /// Debian `:native`.
    Native,
    /// One exact Debian architecture qualifier such as `:amd64`.
    Exact(String),
}

/// Debian package `Multi-Arch` behavior.
///
/// Absence of the control field is exactly [`Self::No`] under Debian Policy.
/// This type is package metadata and must not be inferred from dependency
/// spellings, repository names, or target distro names.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DebianMultiArch {
    /// The field is absent or explicitly `no`.
    #[default]
    No,
    /// Same-name packages of multiple architectures may be co-installed.
    Same,
    /// Cross-architecture use requires an explicit `:any` dependency.
    Allowed,
    /// Unqualified dependencies may be satisfied across architectures.
    Foreign,
}

impl DebianMultiArch {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::No => "no",
            Self::Same => "same",
            Self::Allowed => "allowed",
            Self::Foreign => "foreign",
        }
    }

    pub fn parse_exact(value: &str) -> Result<Self, String> {
        match value {
            "no" => Ok(Self::No),
            "same" => Ok(Self::Same),
            "allowed" => Ok(Self::Allowed),
            "foreign" => Ok(Self::Foreign),
            _ => Err(format!("invalid Debian Multi-Arch value: {value:?}")),
        }
    }
}

/// A single alternative inside a requirement group.
///
/// For simple dependencies this is the only clause.  For Debian-style
/// `A | B | C` alternatives there will be multiple clauses in one
/// [`RepositoryRequirementGroup`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RepositoryRequirementClause {
    /// The capability or package name being required.
    pub name: String,

    /// Whether this targets a specific capability kind, or is ambiguous.
    ///
    /// `None` means "match by the native ecosystem's default lookup order"
    /// (package name first, then virtual, then soname/file).
    pub capability_kind: Option<RepositoryCapabilityKind>,

    /// Version constraint in the native format (e.g. `">= 2.34"`).
    ///
    /// The constraint string is intended to be parsed with
    /// [`super::versioning::parse_repo_constraint`] using the appropriate
    /// [`VersionScheme`].
    pub version_constraint: Option<String>,

    /// Exact source-native architecture qualifier.
    #[serde(default)]
    pub architecture_qualifier: RequirementArchitectureQualifier,

    /// The original native text for this single clause.
    pub native_text: Option<String>,
}

/// Exact boolean dependency expression from a native package manager.
///
/// RPM rich dependencies use all six binary operators below. Keeping the
/// parsed expression makes the source grammar authoritative through sync and
/// resolution; no later layer needs to rediscover semantics from text.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "operator", content = "operands", rename_all = "snake_case")]
pub enum RepositoryRequirementExpression {
    Atom(RepositoryRequirementClause),
    And(Vec<RepositoryRequirementExpression>),
    Or(Vec<RepositoryRequirementExpression>),
    If {
        requirement: Box<RepositoryRequirementExpression>,
        condition: Box<RepositoryRequirementExpression>,
        otherwise: Option<Box<RepositoryRequirementExpression>>,
    },
    Unless {
        requirement: Box<RepositoryRequirementExpression>,
        condition: Box<RepositoryRequirementExpression>,
        otherwise: Option<Box<RepositoryRequirementExpression>>,
    },
    With {
        left: Box<RepositoryRequirementExpression>,
        right: Box<RepositoryRequirementExpression>,
    },
    Without {
        left: Box<RepositoryRequirementExpression>,
        right: Box<RepositoryRequirementExpression>,
    },
}

impl RepositoryRequirementExpression {
    /// Collect the atomic clauses for candidate preloading and diagnostics.
    pub fn atoms(&self) -> Vec<&RepositoryRequirementClause> {
        let mut atoms = Vec::new();
        self.collect_atoms(&mut atoms);
        atoms
    }

    fn collect_atoms<'a>(&'a self, atoms: &mut Vec<&'a RepositoryRequirementClause>) {
        match self {
            Self::Atom(clause) => atoms.push(clause),
            Self::And(operands) | Self::Or(operands) => {
                for operand in operands {
                    operand.collect_atoms(atoms);
                }
            }
            Self::If {
                requirement,
                condition,
                otherwise,
            }
            | Self::Unless {
                requirement,
                condition,
                otherwise,
            } => {
                requirement.collect_atoms(atoms);
                condition.collect_atoms(atoms);
                if let Some(otherwise) = otherwise {
                    otherwise.collect_atoms(atoms);
                }
            }
            Self::With { left, right } | Self::Without { left, right } => {
                left.collect_atoms(atoms);
                right.collect_atoms(atoms);
            }
        }
    }

    #[must_use]
    pub fn is_conditional(&self) -> bool {
        match self {
            Self::If { .. } | Self::Unless { .. } => true,
            Self::And(operands) | Self::Or(operands) => operands.iter().any(Self::is_conditional),
            Self::With { left, right } | Self::Without { left, right } => {
                left.is_conditional() || right.is_conditional()
            }
            Self::Atom(_) => false,
        }
    }
}

impl RepositoryRequirementClause {
    /// Simple clause with just a name.
    #[must_use]
    pub fn name_only(name: String) -> Self {
        Self {
            name,
            capability_kind: None,
            version_constraint: None,
            architecture_qualifier: RequirementArchitectureQualifier::Unqualified,
            native_text: None,
        }
    }

    /// Clause with a version constraint.
    #[must_use]
    pub fn versioned(name: String, constraint: String) -> Self {
        Self {
            name,
            capability_kind: None,
            version_constraint: Some(constraint),
            architecture_qualifier: RequirementArchitectureQualifier::Unqualified,
            native_text: None,
        }
    }
}

/// A requirement group (possibly with alternatives).
///
/// A group with one clause is a simple dependency.  A group with multiple
/// clauses represents alternatives: the requirement is satisfied if *any*
/// clause is satisfied.
///
/// ```text
/// Debian: "libc6 (>= 2.34), default-mta | mail-transport-agent"
///   -> two groups:
///      1. [libc6 >= 2.34]            (single clause)
///      2. [default-mta, mail-transport-agent]  (two alternatives)
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RepositoryRequirementGroup {
    /// The relationship kind (Depends, Optional, Conflict, ...).
    pub kind: RepositoryRequirementKind,

    /// How this requirement was classified (hard, conditional, unsupported).
    pub behavior: ConditionalRequirementBehavior,

    /// Parsed native boolean expression. Simple dependencies are represented
    /// as a single [`RepositoryRequirementExpression::Atom`].
    pub expression: RepositoryRequirementExpression,

    /// One or more alternative clauses (OR semantics).
    ///
    /// Invariant: never empty.
    pub alternatives: Vec<RepositoryRequirementClause>,

    /// Optional description for optional/recommended dependencies.
    pub description: Option<String>,

    /// The full original native text of this group for diagnostics.
    pub native_text: Option<String>,
}

impl RepositoryRequirementGroup {
    /// Create a simple hard dependency on a single name.
    #[must_use]
    pub fn simple(kind: RepositoryRequirementKind, clause: RepositoryRequirementClause) -> Self {
        Self {
            kind,
            behavior: ConditionalRequirementBehavior::Hard,
            expression: RepositoryRequirementExpression::Atom(clause.clone()),
            alternatives: vec![clause],
            description: None,
            native_text: None,
        }
    }

    /// Create a group with alternatives.
    #[must_use]
    pub fn alternatives(
        kind: RepositoryRequirementKind,
        clauses: Vec<RepositoryRequirementClause>,
    ) -> Self {
        Self {
            kind,
            behavior: ConditionalRequirementBehavior::Hard,
            expression: RepositoryRequirementExpression::Or(
                clauses
                    .iter()
                    .cloned()
                    .map(RepositoryRequirementExpression::Atom)
                    .collect(),
            ),
            alternatives: clauses,
            description: None,
            native_text: None,
        }
    }

    /// Create an optional/recommended dependency.
    #[must_use]
    pub fn optional(clause: RepositoryRequirementClause, description: Option<String>) -> Self {
        Self {
            kind: RepositoryRequirementKind::Optional,
            behavior: ConditionalRequirementBehavior::Hard,
            expression: RepositoryRequirementExpression::Atom(clause.clone()),
            alternatives: vec![clause],
            description,
            native_text: None,
        }
    }

    /// Mark this group as conditional.
    #[must_use]
    pub fn with_behavior(mut self, behavior: ConditionalRequirementBehavior) -> Self {
        self.behavior = behavior;
        self
    }

    /// Replace the default simple/alternative expression with source-native
    /// parsed semantics.
    #[must_use]
    pub fn with_expression(mut self, expression: RepositoryRequirementExpression) -> Self {
        self.expression = expression;
        self
    }

    /// Attach the original native text.
    #[must_use]
    pub fn with_native_text(mut self, text: String) -> Self {
        self.native_text = Some(text);
        self
    }

    /// Whether this is a simple (single-clause, no alternatives) group.
    #[must_use]
    pub fn is_simple(&self) -> bool {
        self.alternatives.len() == 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dependency_flavor_maps_to_version_scheme() {
        assert_eq!(
            RepositoryDependencyFlavor::Rpm.version_scheme(),
            VersionScheme::Rpm
        );
        assert_eq!(
            RepositoryDependencyFlavor::Deb.version_scheme(),
            VersionScheme::Debian
        );
        assert_eq!(
            RepositoryDependencyFlavor::Arch.version_scheme(),
            VersionScheme::Arch
        );
    }

    #[test]
    fn simple_requirement_group_is_simple() {
        let group = RepositoryRequirementGroup::simple(
            RepositoryRequirementKind::Depends,
            RepositoryRequirementClause::name_only("glibc".to_string()),
        );
        assert!(group.is_simple());
        assert_eq!(group.behavior, ConditionalRequirementBehavior::Hard);
    }

    #[test]
    fn alternative_group_has_multiple_clauses() {
        let group = RepositoryRequirementGroup::alternatives(
            RepositoryRequirementKind::Depends,
            vec![
                RepositoryRequirementClause::name_only("default-mta".to_string()),
                RepositoryRequirementClause::name_only("mail-transport-agent".to_string()),
            ],
        );
        assert!(!group.is_simple());
        assert_eq!(group.alternatives.len(), 2);
    }

    #[test]
    fn optional_group_carries_description() {
        let group = RepositoryRequirementGroup::optional(
            RepositoryRequirementClause::name_only("fzf".to_string()),
            Some("fuzzy finder for interactive use".to_string()),
        );
        assert_eq!(group.kind, RepositoryRequirementKind::Optional);
        assert!(group.description.is_some());
    }

    #[test]
    fn conditional_behavior_round_trips() {
        let group = RepositoryRequirementGroup::simple(
            RepositoryRequirementKind::Depends,
            RepositoryRequirementClause::versioned("systemd".to_string(), ">= 255".to_string()),
        )
        .with_behavior(ConditionalRequirementBehavior::Conditional)
        .with_native_text("(systemd >= 255 if systemd-resolved)".to_string());

        assert_eq!(group.behavior, ConditionalRequirementBehavior::Conditional);
        assert_eq!(
            group.native_text.as_deref(),
            Some("(systemd >= 255 if systemd-resolved)")
        );
    }

    #[test]
    fn provide_constructors_set_correct_kind() {
        let pkg = RepositoryProvide::package_name("bash".to_string(), Some("5.2-1".to_string()));
        assert_eq!(pkg.kind, RepositoryCapabilityKind::PackageName);

        let virt = RepositoryProvide::virtual_cap("java-runtime".to_string(), None);
        assert_eq!(virt.kind, RepositoryCapabilityKind::Virtual);

        let so = RepositoryProvide::soname("libc.so.6".to_string(), None);
        assert_eq!(so.kind, RepositoryCapabilityKind::Soname);

        let file = RepositoryProvide::file("/usr/bin/python3".to_string());
        assert_eq!(file.kind, RepositoryCapabilityKind::File);
        assert!(file.version.is_none());
    }

    #[test]
    fn versioned_clause_stores_constraint() {
        let clause =
            RepositoryRequirementClause::versioned("libc6".to_string(), ">= 2.34".to_string());
        assert_eq!(clause.version_constraint.as_deref(), Some(">= 2.34"));
        assert!(clause.capability_kind.is_none());
    }

    #[test]
    fn capability_validation_rejects_malformed_provenance_and_architecture() {
        let mut capability = ProvidedCapability {
            kind: RepositoryCapabilityKind::File,
            name: "/usr/bin/tool".to_string(),
            version: None,
            version_relation: None,
            version_scheme: VersionScheme::Arch,
            architecture_qualifier: ProvideArchitectureQualifier::Implicit,
            provenance: CapabilityProvenance::SourceDerivedFile {
                format: SourcePackageFormat::Alpm,
                source_path: "usr/bin/tool".to_string(),
            },
        };
        assert!(capability.validate().is_err());

        capability.provenance = CapabilityProvenance::AuthorDeclared;
        capability.architecture_qualifier = ProvideArchitectureQualifier::Exact(String::new());
        assert!(capability.validate().is_err());
    }
}
