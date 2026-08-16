// conary-core/src/repository/package_relation.rs

//! Exact native package-relation parsing and validation.
//!
//! Conflict and replacement declarations are transaction authority. They are
//! parsed with their source grammar and retained as typed requirement
//! expressions; callers must never recover these semantics from substrings.

use super::dependency_model::{
    ConditionalRequirementBehavior, ProvideArchitectureQualifier, RepositoryCapabilityKind,
    RepositoryProvide, RepositoryRequirementClause, RepositoryRequirementExpression,
    RepositoryRequirementGroup, RepositoryRequirementKind, RequirementArchitectureQualifier,
};
use super::versioning::{RepoVersionConstraint, repo_version_satisfies};
use super::versioning::{VersionScheme, parse_repo_constraint};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedArchProvide {
    pub name: String,
    pub kind: RepositoryCapabilityKind,
    pub version: Option<String>,
}

pub struct PackageRelationProvide<'a> {
    pub name: &'a str,
    pub version: Option<&'a str>,
    pub version_scheme: VersionScheme,
}

pub struct PackageRelationCandidate<'a> {
    pub name: &'a str,
    pub version: &'a str,
    pub version_scheme: VersionScheme,
    pub provides: &'a [PackageRelationProvide<'a>],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedPackageRelationProvide {
    pub name: String,
    pub version: Option<String>,
    pub version_scheme: VersionScheme,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedPackageRelationCandidate {
    pub name: String,
    pub version: String,
    pub version_scheme: VersionScheme,
    pub provides: Vec<OwnedPackageRelationProvide>,
}

impl OwnedPackageRelationCandidate {
    fn evaluate(
        &self,
        expression: &RepositoryRequirementExpression,
        relation_scheme: VersionScheme,
    ) -> Result<bool, String> {
        let provides = self
            .provides
            .iter()
            .map(|provide| PackageRelationProvide {
                name: &provide.name,
                version: provide.version.as_deref(),
                version_scheme: provide.version_scheme,
            })
            .collect::<Vec<_>>();
        evaluate_expression(
            expression,
            relation_scheme,
            &PackageRelationCandidate {
                name: &self.name,
                version: &self.version,
                version_scheme: self.version_scheme,
                provides: &provides,
            },
        )
    }
}

/// Parse one source-native conflict, break, replacement, or obsolete entry.
pub fn parse_native_relation(
    kind: RepositoryRequirementKind,
    scheme: VersionScheme,
    native_text: &str,
) -> Result<RepositoryRequirementGroup, String> {
    if !kind.is_negative_relation() {
        return Err(format!(
            "{kind:?} is not a conflict or replacement relation"
        ));
    }
    let native_text = native_text.trim();
    if native_text.is_empty() {
        return Err("package relation is empty".to_string());
    }

    let expression = match scheme {
        VersionScheme::Rpm => super::rpm_dependency::parse_rpm_dependency(kind, native_text)?,
        VersionScheme::Debian => parse_debian_relation(native_text)?,
        VersionScheme::Arch => RepositoryRequirementExpression::Atom(parse_arch_atom(native_text)?),
        VersionScheme::Conary | VersionScheme::Eopkg => {
            RepositoryRequirementExpression::Atom(parse_inline_atom(native_text, scheme)?)
        }
    };
    validate_expression_shape(kind, scheme, &expression)?;
    validate_expression_constraints(&expression, scheme)?;

    let alternatives = expression.atoms().into_iter().cloned().collect::<Vec<_>>();
    let first = alternatives
        .first()
        .cloned()
        .ok_or_else(|| format!("package relation produced no atoms: {native_text}"))?;
    let behavior = if expression.is_conditional() {
        ConditionalRequirementBehavior::Conditional
    } else {
        ConditionalRequirementBehavior::Hard
    };
    let mut group = RepositoryRequirementGroup::simple(kind, first)
        .with_behavior(behavior)
        .with_expression(expression)
        .with_native_text(native_text.to_string());
    group.alternatives = alternatives;
    Ok(group)
}

/// Validate a relation already decoded from CCS or persisted repository state.
pub fn validate_native_relation(
    relation: &RepositoryRequirementGroup,
    scheme: VersionScheme,
) -> Result<(), String> {
    if !relation.kind.is_negative_relation() {
        return Err(format!(
            "{:?} is not valid in package relation authority",
            relation.kind
        ));
    }
    if relation.alternatives.is_empty() {
        return Err("package relation has no indexed atoms".to_string());
    }
    let expression_atoms = relation.expression.atoms();
    if expression_atoms.is_empty() {
        return Err("package relation expression has no atoms".to_string());
    }
    if expression_atoms.len() != relation.alternatives.len()
        || expression_atoms
            .iter()
            .zip(&relation.alternatives)
            .any(|(expression, indexed)| *expression != indexed)
    {
        return Err(
            "package relation clause index disagrees with its authoritative expression".to_string(),
        );
    }
    let expected_behavior = if relation.expression.is_conditional() {
        ConditionalRequirementBehavior::Conditional
    } else {
        ConditionalRequirementBehavior::Hard
    };
    if relation.behavior != expected_behavior {
        return Err(
            "package relation behavior disagrees with its authoritative expression".to_string(),
        );
    }
    validate_expression_shape(relation.kind, scheme, &relation.expression)?;
    validate_expression_constraints(&relation.expression, scheme)
}

/// Evaluate a typed relation against one package candidate. Unversioned
/// relations may cross package ecosystems; versioned relations only compare
/// versions carrying the relation's exact source scheme.
pub fn relation_matches_candidate(
    relation: &RepositoryRequirementGroup,
    relation_scheme: VersionScheme,
    candidate: &PackageRelationCandidate<'_>,
) -> Result<bool, String> {
    validate_native_relation(relation, relation_scheme)?;
    evaluate_expression(&relation.expression, relation_scheme, candidate)
}

/// Evaluate a native relation expression over the complete candidate set.
///
/// RPM `and`/`or`/conditionals combine matches across packages, while
/// `with`/`without` deliberately evaluate against one provider at a time.
pub fn expression_matches_candidate_set(
    expression: &RepositoryRequirementExpression,
    relation_scheme: VersionScheme,
    candidates: &[OwnedPackageRelationCandidate],
) -> Result<bool, String> {
    match expression {
        RepositoryRequirementExpression::Atom(_) => {
            for candidate in candidates {
                if candidate.evaluate(expression, relation_scheme)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        RepositoryRequirementExpression::And(operands) => {
            for operand in operands {
                if !expression_matches_candidate_set(operand, relation_scheme, candidates)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        RepositoryRequirementExpression::Or(operands) => {
            for operand in operands {
                if expression_matches_candidate_set(operand, relation_scheme, candidates)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        RepositoryRequirementExpression::If {
            requirement,
            condition,
            otherwise,
        } => {
            if expression_matches_candidate_set(condition, relation_scheme, candidates)? {
                expression_matches_candidate_set(requirement, relation_scheme, candidates)
            } else if let Some(otherwise) = otherwise {
                expression_matches_candidate_set(otherwise, relation_scheme, candidates)
            } else {
                Ok(true)
            }
        }
        RepositoryRequirementExpression::Unless {
            requirement,
            condition,
            otherwise,
        } => {
            if !expression_matches_candidate_set(condition, relation_scheme, candidates)? {
                expression_matches_candidate_set(requirement, relation_scheme, candidates)
            } else if let Some(otherwise) = otherwise {
                expression_matches_candidate_set(otherwise, relation_scheme, candidates)
            } else {
                Ok(true)
            }
        }
        RepositoryRequirementExpression::With { .. }
        | RepositoryRequirementExpression::Without { .. } => {
            for candidate in candidates {
                if candidate.evaluate(expression, relation_scheme)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
    }
}

/// Return the exact minimum-cardinality removable candidate set that makes a
/// conflict expression false. `fixed` candidates represent co-selected
/// packages and can never be silently removed.
pub fn minimum_conflict_removal_indices(
    relation: &RepositoryRequirementGroup,
    relation_scheme: VersionScheme,
    removable: &[OwnedPackageRelationCandidate],
    fixed: &[OwnedPackageRelationCandidate],
) -> Result<Vec<usize>, String> {
    validate_native_relation(relation, relation_scheme)?;
    let mut all = removable.to_vec();
    all.extend_from_slice(fixed);
    if !expression_matches_candidate_set(&relation.expression, relation_scheme, &all)? {
        return Ok(Vec::new());
    }

    let mut eligible = Vec::new();
    for (index, candidate) in removable.iter().enumerate() {
        let mut relevant = false;
        for clause in relation.expression.atoms() {
            if candidate.evaluate(
                &RepositoryRequirementExpression::Atom((*clause).clone()),
                relation_scheme,
            )? {
                relevant = true;
                break;
            }
        }
        if relevant {
            eligible.push(index);
        }
    }

    for target_size in 1..=eligible.len() {
        let mut selected = Vec::with_capacity(target_size);
        if let Some(indices) = find_false_subset(
            relation,
            relation_scheme,
            removable,
            fixed,
            &eligible,
            target_size,
            0,
            &mut selected,
        )? {
            return Ok(indices);
        }
    }
    Err(format!(
        "{} relation '{}' cannot be satisfied by removing installed candidates because a co-selected package keeps it true",
        relation.kind.as_str(),
        relation
            .native_text
            .as_deref()
            .unwrap_or("<typed relation>")
    ))
}

/// Return the exact minimum-cardinality incoming candidate set that changes a
/// relation from false against `baseline` to true.
///
/// This identifies every package that jointly causes an installed rich
/// Conflict or Breaks expression. It never attributes a multi-package
/// expression to an arbitrary first candidate.
pub fn minimum_relation_addition_indices(
    relation: &RepositoryRequirementGroup,
    relation_scheme: VersionScheme,
    baseline: &[OwnedPackageRelationCandidate],
    additions: &[OwnedPackageRelationCandidate],
) -> Result<Vec<usize>, String> {
    validate_native_relation(relation, relation_scheme)?;
    if expression_matches_candidate_set(&relation.expression, relation_scheme, baseline)? {
        return Ok(Vec::new());
    }

    let mut all = baseline.to_vec();
    all.extend_from_slice(additions);
    if !expression_matches_candidate_set(&relation.expression, relation_scheme, &all)? {
        return Err(format!(
            "{} relation '{}' does not become true with the selected incoming packages",
            relation.kind.as_str(),
            relation
                .native_text
                .as_deref()
                .unwrap_or("<typed relation>")
        ));
    }

    let eligible = (0..additions.len()).collect::<Vec<_>>();
    for target_size in 1..=eligible.len() {
        let mut selected = Vec::with_capacity(target_size);
        if let Some(indices) = find_true_subset(
            relation,
            relation_scheme,
            baseline,
            additions,
            &eligible,
            target_size,
            0,
            &mut selected,
        )? {
            return Ok(indices);
        }
    }
    Err(format!(
        "{} relation '{}' became true but no exact incoming cause set was found",
        relation.kind.as_str(),
        relation
            .native_text
            .as_deref()
            .unwrap_or("<typed relation>")
    ))
}

#[allow(clippy::too_many_arguments)]
fn find_false_subset(
    relation: &RepositoryRequirementGroup,
    relation_scheme: VersionScheme,
    removable: &[OwnedPackageRelationCandidate],
    fixed: &[OwnedPackageRelationCandidate],
    eligible: &[usize],
    target_size: usize,
    start: usize,
    selected: &mut Vec<usize>,
) -> Result<Option<Vec<usize>>, String> {
    if selected.len() == target_size {
        let selected = selected
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>();
        let mut remaining = removable
            .iter()
            .enumerate()
            .filter(|(index, _)| !selected.contains(index))
            .map(|(_, candidate)| candidate.clone())
            .collect::<Vec<_>>();
        remaining.extend_from_slice(fixed);
        return expression_matches_candidate_set(&relation.expression, relation_scheme, &remaining)
            .map(|matches| {
                (!matches).then(|| {
                    let mut result = selected.into_iter().collect::<Vec<_>>();
                    result.sort_unstable();
                    result
                })
            });
    }
    let needed = target_size - selected.len();
    if eligible.len().saturating_sub(start) < needed {
        return Ok(None);
    }
    for offset in start..eligible.len() {
        selected.push(eligible[offset]);
        if let Some(result) = find_false_subset(
            relation,
            relation_scheme,
            removable,
            fixed,
            eligible,
            target_size,
            offset + 1,
            selected,
        )? {
            return Ok(Some(result));
        }
        selected.pop();
    }
    Ok(None)
}

#[allow(clippy::too_many_arguments)]
fn find_true_subset(
    relation: &RepositoryRequirementGroup,
    relation_scheme: VersionScheme,
    baseline: &[OwnedPackageRelationCandidate],
    additions: &[OwnedPackageRelationCandidate],
    eligible: &[usize],
    target_size: usize,
    start: usize,
    selected: &mut Vec<usize>,
) -> Result<Option<Vec<usize>>, String> {
    if selected.len() == target_size {
        let mut candidates = baseline.to_vec();
        candidates.extend(selected.iter().map(|index| additions[*index].clone()));
        return expression_matches_candidate_set(
            &relation.expression,
            relation_scheme,
            &candidates,
        )
        .map(|matches| matches.then(|| selected.clone()));
    }
    let needed = target_size - selected.len();
    if eligible.len().saturating_sub(start) < needed {
        return Ok(None);
    }
    for offset in start..eligible.len() {
        selected.push(eligible[offset]);
        if let Some(result) = find_true_subset(
            relation,
            relation_scheme,
            baseline,
            additions,
            eligible,
            target_size,
            offset + 1,
            selected,
        )? {
            return Ok(Some(result));
        }
        selected.pop();
    }
    Ok(None)
}

fn parse_debian_relation(input: &str) -> Result<RepositoryRequirementExpression, String> {
    if input.bytes().any(|byte| byte == b'|') {
        return Err(format!(
            "Debian negative relationship fields do not permit alternatives: {input}"
        ));
    }
    Ok(RepositoryRequirementExpression::Atom(parse_debian_atom(
        input,
    )?))
}

pub(crate) fn parse_debian_atom(input: &str) -> Result<RepositoryRequirementClause, String> {
    let input = input.trim();
    let name_end = input
        .char_indices()
        .find_map(|(index, character)| {
            (character.is_whitespace() || matches!(character, '(' | '[' | '<')).then_some(index)
        })
        .unwrap_or(input.len());
    let native_name = &input[..name_end];
    let (name, architecture_qualifier) = parse_debian_name(native_name, input)?;
    let suffix = input[name_end..].trim_start();
    if suffix.starts_with('[') || suffix.starts_with('<') {
        return Err(format!(
            "Debian architecture/profile restrictions are not valid in binary package relationships: {input}"
        ));
    }
    if suffix.is_empty() {
        let mut clause = RepositoryRequirementClause::name_only(name.to_string());
        clause.architecture_qualifier = architecture_qualifier;
        clause.native_text = Some(input.to_string());
        return Ok(clause);
    }
    if !suffix.starts_with('(') || !suffix.ends_with(')') {
        return Err(format!("malformed Debian relation atom: {input}"));
    }
    let constraint = suffix[1..suffix.len() - 1].trim();
    if constraint.bytes().any(|byte| matches!(byte, b'(' | b')')) {
        return Err(format!("malformed Debian relation atom: {input}"));
    }
    let mut parts = constraint.split_whitespace();
    let operator = parts
        .next()
        .ok_or_else(|| format!("missing Debian relation constraint: {input}"))?;
    let version = parts
        .next()
        .ok_or_else(|| format!("missing Debian relation version: {input}"))?;
    if !matches!(operator, "<<" | "<=" | "=" | ">=" | ">>")
        || version.is_empty()
        || parts.next().is_some()
    {
        return Err(format!("invalid Debian relation constraint: {input}"));
    }
    let mut clause =
        RepositoryRequirementClause::versioned(name.to_string(), format!("{operator} {version}"));
    clause.architecture_qualifier = architecture_qualifier;
    clause.native_text = Some(input.to_string());
    Ok(clause)
}

/// Parse one Debian `Provides` atom with dpkg's exact version and architecture
/// grammar. Only an exact provided version is meaningful to dpkg. A relation
/// naming the package itself is package-name authority rather than a virtual
/// alias.
pub(crate) fn parse_debian_provide(
    input: &str,
    package_name: &str,
) -> Result<RepositoryProvide, String> {
    let clause = parse_debian_atom(input)?;
    let version = match clause.version_constraint.as_deref() {
        None => None,
        Some(raw) => match parse_repo_constraint(VersionScheme::Debian, raw)
            .map_err(|error| error.to_string())?
        {
            RepoVersionConstraint::Exact(version) => Some(version),
            _ => {
                return Err(format!(
                    "Debian Provides permits only an exact '=' version: {input}"
                ));
            }
        },
    };
    let architecture_qualifier = match clause.architecture_qualifier {
        RequirementArchitectureQualifier::Unqualified => ProvideArchitectureQualifier::Implicit,
        RequirementArchitectureQualifier::Any => ProvideArchitectureQualifier::Any,
        RequirementArchitectureQualifier::Native => {
            ProvideArchitectureQualifier::Exact("native".to_string())
        }
        RequirementArchitectureQualifier::Exact(architecture) => {
            ProvideArchitectureQualifier::Exact(architecture)
        }
    };
    let kind = if clause.name == package_name {
        RepositoryCapabilityKind::PackageName
    } else {
        RepositoryCapabilityKind::Virtual
    };
    Ok(RepositoryProvide {
        name: clause.name,
        kind,
        version_relation: version
            .as_ref()
            .map(|_| crate::repository::dependency_model::ProvideVersionRelation::Equal),
        version,
        architecture_qualifier,
        native_text: Some(input.trim().to_string()),
        provenance: crate::repository::dependency_model::CapabilityProvenance::SourceDeclared {
            format: crate::repository::dependency_model::SourcePackageFormat::Debian,
            record_index: 0,
        },
    })
}

pub(crate) fn parse_arch_atom(input: &str) -> Result<RepositoryRequirementClause, String> {
    if input.contains(char::is_whitespace) {
        return Err(format!(
            "Arch relation must use the exact inline grammar: {input}"
        ));
    }
    let clause = parse_inline_atom(input, VersionScheme::Arch)?;
    validate_arch_name(&clause.name, input)?;
    Ok(clause)
}

pub(crate) fn parse_arch_runtime_atom(input: &str) -> Result<RepositoryRequirementClause, String> {
    match input
        .parse::<alpm_types::RelationOrSoname>()
        .map_err(|error| error.to_string())?
    {
        alpm_types::RelationOrSoname::Relation(relation) => {
            Ok(arch_package_relation_clause(relation))
        }
        alpm_types::RelationOrSoname::SonameV1(soname) => {
            Ok(arch_soname_clause(soname.to_string(), input))
        }
        alpm_types::RelationOrSoname::SonameV2(soname) => {
            Ok(arch_soname_clause(soname.to_string(), input))
        }
    }
}

pub(crate) fn parse_arch_provide(
    input: &str,
    package_name: &str,
) -> Result<ParsedArchProvide, String> {
    match input
        .parse::<alpm_types::RelationOrSoname>()
        .map_err(|error| error.to_string())?
    {
        alpm_types::RelationOrSoname::Relation(relation) => {
            let version = match relation.version_requirement {
                None => None,
                Some(requirement)
                    if requirement.comparison == alpm_types::VersionComparison::Equal =>
                {
                    Some(requirement.version.to_string())
                }
                Some(_) => {
                    return Err(format!(
                        "Arch %PROVIDES% entry '{input}' must be unversioned or use the exact '=' relation"
                    ));
                }
            };
            let name = relation.name.to_string();
            let kind = if name == package_name {
                RepositoryCapabilityKind::PackageName
            } else {
                RepositoryCapabilityKind::Virtual
            };
            Ok(ParsedArchProvide {
                name,
                kind,
                version,
            })
        }
        alpm_types::RelationOrSoname::SonameV1(soname) => Ok(ParsedArchProvide {
            name: soname.to_string(),
            kind: RepositoryCapabilityKind::Soname,
            version: None,
        }),
        alpm_types::RelationOrSoname::SonameV2(soname) => Ok(ParsedArchProvide {
            name: soname.to_string(),
            kind: RepositoryCapabilityKind::Soname,
            version: None,
        }),
    }
}

fn arch_package_relation_clause(
    relation: alpm_types::PackageRelation,
) -> RepositoryRequirementClause {
    let name = relation.name.to_string();
    match relation.version_requirement {
        Some(requirement) => RepositoryRequirementClause::versioned(
            name,
            format!("{} {}", requirement.comparison, requirement.version),
        ),
        None => RepositoryRequirementClause::name_only(name),
    }
}

fn arch_soname_clause(identity: String, native_text: &str) -> RepositoryRequirementClause {
    let mut clause = RepositoryRequirementClause::name_only(identity);
    clause.capability_kind = Some(RepositoryCapabilityKind::Soname);
    clause.native_text = Some(native_text.to_string());
    clause
}

pub(super) fn parse_inline_atom(
    input: &str,
    scheme: VersionScheme,
) -> Result<RepositoryRequirementClause, String> {
    let operator = [">=", "<=", "!=", "=", ">", "<"]
        .into_iter()
        .find_map(|operator| input.find(operator).map(|index| (index, operator)));
    let Some((index, operator)) = operator else {
        validate_name(input, input)?;
        return Ok(RepositoryRequirementClause::name_only(input.to_string()));
    };
    if scheme == VersionScheme::Arch && operator == "!=" {
        return Err(format!("ALPM relation does not support '!=': {input}"));
    }
    let name = input[..index].trim();
    let version = input[index + operator.len()..].trim();
    validate_name(name, input)?;
    if version.is_empty() {
        return Err(format!("package relation has an empty version: {input}"));
    }
    Ok(RepositoryRequirementClause::versioned(
        name.to_string(),
        format!("{operator} {version}"),
    ))
}

fn validate_name(name: &str, input: &str) -> Result<(), String> {
    if name.is_empty() || name.chars().any(char::is_whitespace) {
        return Err(format!("invalid package relation name: {input}"));
    }
    Ok(())
}

fn parse_debian_name<'a>(
    name: &'a str,
    input: &str,
) -> Result<(&'a str, RequirementArchitectureQualifier), String> {
    let (package_name, architecture) = name
        .split_once(':')
        .map_or((name, None), |(package, arch)| (package, Some(arch)));
    let mut characters = package_name.chars();
    let valid_start = characters
        .next()
        .is_some_and(|character| character.is_ascii_lowercase() || character.is_ascii_digit());
    if package_name.len() < 2
        || !valid_start
        || !characters.all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '+' | '-' | '.')
        })
    {
        return Err(format!("invalid Debian package relation name: {input}"));
    }
    let architecture_qualifier = match architecture {
        None => RequirementArchitectureQualifier::Unqualified,
        Some("any") => RequirementArchitectureQualifier::Any,
        Some("native") => RequirementArchitectureQualifier::Native,
        Some(architecture)
            if !architecture.is_empty()
                && architecture.chars().all(|character| {
                    character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
                }) =>
        {
            RequirementArchitectureQualifier::Exact(architecture.to_string())
        }
        Some(_) => {
            return Err(format!(
                "invalid Debian dependency architecture qualifier: {input}"
            ));
        }
    };
    Ok((package_name, architecture_qualifier))
}

fn validate_arch_name(name: &str, input: &str) -> Result<(), String> {
    let mut characters = name.chars();
    if !characters.next().is_some_and(|character| {
        character.is_ascii_alphanumeric() || matches!(character, '@' | '_' | '+')
    }) || !characters.all(|character| {
        character.is_ascii_alphanumeric() || matches!(character, '@' | '.' | '_' | '+' | '-')
    }) {
        return Err(format!("invalid ALPM package relation name: {input}"));
    }
    Ok(())
}

fn validate_expression_shape(
    kind: RepositoryRequirementKind,
    scheme: VersionScheme,
    expression: &RepositoryRequirementExpression,
) -> Result<(), String> {
    if (scheme != VersionScheme::Rpm || kind.removes_matching_packages())
        && !matches!(expression, RepositoryRequirementExpression::Atom(_))
    {
        return Err(format!(
            "{} {} relations require one exact package relation atom",
            scheme.as_str(),
            kind.as_str()
        ));
    }
    Ok(())
}

fn validate_expression_constraints(
    expression: &RepositoryRequirementExpression,
    scheme: VersionScheme,
) -> Result<(), String> {
    for clause in expression.atoms() {
        validate_name(&clause.name, &clause.name)?;
        if let Some(raw) = clause.version_constraint.as_deref() {
            parse_repo_constraint(scheme, raw).map_err(|error| {
                format!(
                    "relation '{}' has invalid {} constraint '{}': {error}",
                    clause.name,
                    scheme.as_str(),
                    raw
                )
            })?;
        }
    }
    Ok(())
}

pub(crate) fn evaluate_expression(
    expression: &RepositoryRequirementExpression,
    relation_scheme: VersionScheme,
    candidate: &PackageRelationCandidate<'_>,
) -> Result<bool, String> {
    match expression {
        RepositoryRequirementExpression::Atom(clause) => {
            clause_matches_candidate(clause, relation_scheme, candidate)
        }
        RepositoryRequirementExpression::And(operands) => {
            for operand in operands {
                if !evaluate_expression(operand, relation_scheme, candidate)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        RepositoryRequirementExpression::Or(operands) => {
            for operand in operands {
                if evaluate_expression(operand, relation_scheme, candidate)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        RepositoryRequirementExpression::If {
            requirement,
            condition,
            otherwise,
        } => {
            if evaluate_expression(condition, relation_scheme, candidate)? {
                evaluate_expression(requirement, relation_scheme, candidate)
            } else if let Some(otherwise) = otherwise {
                evaluate_expression(otherwise, relation_scheme, candidate)
            } else {
                Ok(true)
            }
        }
        RepositoryRequirementExpression::Unless {
            requirement,
            condition,
            otherwise,
        } => {
            if !evaluate_expression(condition, relation_scheme, candidate)? {
                evaluate_expression(requirement, relation_scheme, candidate)
            } else if let Some(otherwise) = otherwise {
                evaluate_expression(otherwise, relation_scheme, candidate)
            } else {
                Ok(true)
            }
        }
        RepositoryRequirementExpression::With { left, right } => {
            Ok(evaluate_expression(left, relation_scheme, candidate)?
                && evaluate_expression(right, relation_scheme, candidate)?)
        }
        RepositoryRequirementExpression::Without { left, right } => {
            Ok(evaluate_expression(left, relation_scheme, candidate)?
                && !evaluate_expression(right, relation_scheme, candidate)?)
        }
    }
}

fn clause_matches_candidate(
    clause: &RepositoryRequirementClause,
    relation_scheme: VersionScheme,
    candidate: &PackageRelationCandidate<'_>,
) -> Result<bool, String> {
    let constraint = match clause.version_constraint.as_deref() {
        Some(raw) => parse_repo_constraint(relation_scheme, raw).map_err(|error| {
            format!(
                "relation '{}' has invalid {} constraint '{}': {error}",
                clause.name,
                relation_scheme.as_str(),
                raw
            )
        })?,
        None => RepoVersionConstraint::Any,
    };
    if candidate.name == clause.name
        && version_matches(
            &constraint,
            relation_scheme,
            candidate.version,
            candidate.version_scheme,
        )?
    {
        return Ok(true);
    }
    for provide in candidate
        .provides
        .iter()
        .filter(|provide| provide.name == clause.name)
    {
        if matches!(constraint, RepoVersionConstraint::Any) {
            return Ok(true);
        }
        if let Some(version) = provide.version
            && version_matches(
                &constraint,
                relation_scheme,
                version,
                provide.version_scheme,
            )?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn version_matches(
    constraint: &RepoVersionConstraint,
    relation_scheme: VersionScheme,
    version: &str,
    version_scheme: VersionScheme,
) -> Result<bool, String> {
    if matches!(constraint, RepoVersionConstraint::Any) {
        return Ok(true);
    }
    if relation_scheme != version_scheme {
        return Ok(false);
    }
    repo_version_satisfies(relation_scheme, version, constraint).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_each_native_relation_grammar_without_fallback() {
        let rpm = parse_native_relation(
            RepositoryRequirementKind::Conflict,
            VersionScheme::Rpm,
            "openssl-libs < 3.2",
        )
        .unwrap();
        assert_eq!(
            rpm.alternatives[0].version_constraint.as_deref(),
            Some("< 3.2")
        );

        let deb = parse_native_relation(
            RepositoryRequirementKind::Breaks,
            VersionScheme::Debian,
            "libssl3 (<< 3.0.2)",
        )
        .unwrap();
        assert_eq!(deb.alternatives.len(), 1);

        let arch = parse_native_relation(
            RepositoryRequirementKind::Replace,
            VersionScheme::Arch,
            "libfoo<=2.0-1",
        )
        .unwrap();
        assert_eq!(
            arch.alternatives[0].version_constraint.as_deref(),
            Some("<= 2.0-1")
        );
    }

    #[test]
    fn malformed_constraints_do_not_become_unversioned_relations() {
        assert!(
            parse_native_relation(
                RepositoryRequirementKind::Conflict,
                VersionScheme::Debian,
                "libssl3 (>=)",
            )
            .is_err()
        );
        assert!(
            parse_native_relation(
                RepositoryRequirementKind::Conflict,
                VersionScheme::Debian,
                "libssl3 | libssl1.1",
            )
            .is_err()
        );
        assert!(
            parse_native_relation(
                RepositoryRequirementKind::Replace,
                VersionScheme::Arch,
                "libfoo>=",
            )
            .is_err()
        );
    }

    #[test]
    fn debian_same_name_provide_is_package_name_authority() {
        let same_name =
            parse_debian_provide("fixture (= 1.0)", "fixture").expect("same-name provide");
        assert_eq!(same_name.kind, RepositoryCapabilityKind::PackageName);
        assert_eq!(same_name.version.as_deref(), Some("1.0"));

        let alias = parse_debian_provide("fixture-abi (= 1.0)", "fixture").expect("alias");
        assert_eq!(alias.kind, RepositoryCapabilityKind::Virtual);
    }
}
