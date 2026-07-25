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

// ---------------------------------------------------------------------------
// Source distro flavor
// ---------------------------------------------------------------------------

/// Which native ecosystem a dependency originates from.
///
/// This is intentionally parallel to [`VersionScheme`] but is used to tag
/// dependency entries rather than version strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
    /// Must be configured before the depending package (Debian Pre-Depends).
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

    /// The original native text for diagnostics (e.g. `"kernel-core-uname-r = 6.19.6-200.fc44.x86_64"`).
    pub native_text: Option<String>,
}

impl RepositoryProvide {
    /// Create a provide for the package name itself (always present).
    #[must_use]
    pub fn package_name(name: String, version: Option<String>) -> Self {
        Self {
            name,
            kind: RepositoryCapabilityKind::PackageName,
            version,
            native_text: None,
        }
    }

    /// Create a virtual provide.
    #[must_use]
    pub fn virtual_cap(name: String, version: Option<String>) -> Self {
        Self {
            name,
            kind: RepositoryCapabilityKind::Virtual,
            version,
            native_text: None,
        }
    }

    /// Create a soname provide.
    #[must_use]
    pub fn soname(name: String, version: Option<String>) -> Self {
        Self {
            name,
            kind: RepositoryCapabilityKind::Soname,
            version,
            native_text: None,
        }
    }

    /// Create a file provide.
    #[must_use]
    pub fn file(path: String) -> Self {
        Self {
            name: path,
            kind: RepositoryCapabilityKind::File,
            version: None,
            native_text: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Requirement clauses and groups
// ---------------------------------------------------------------------------

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
}
