// conary-core/src/repository/rpm_dependency.rs

//! Source-derived parser for RPM boolean dependency expressions.
//!
//! The grammar and parser-state checks mirror RPM's `rpmrichParseInternal()`
//! and `rpmrichParseForTag()` at upstream commit
//! `a8f0192aee1c08bd1454ed2ac6ebaf506004b55c`. Keep conformance tests tied to
//! that source boundary; package names and diagnostic text are never grammar.

use super::dependency_model::{
    RepositoryRequirementClause, RepositoryRequirementExpression as Expression,
    RepositoryRequirementKind,
};

const CHECK_ACTIVE: u8 = 1 << 0;
const CHECK_NO_WITH: u8 = 1 << 1;
const CHECK_NO_AND: u8 = 1 << 2;
const CHECK_NO_OR: u8 = 1 << 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RichOperator {
    And,
    Or,
    If,
    Unless,
    Else,
    With,
    Without,
}

impl RichOperator {
    fn parse(token: &str) -> Option<Self> {
        match token {
            "and" => Some(Self::And),
            "or" => Some(Self::Or),
            "if" => Some(Self::If),
            "unless" => Some(Self::Unless),
            "else" => Some(Self::Else),
            "with" => Some(Self::With),
            "without" => Some(Self::Without),
            _ => None,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::And => "and",
            Self::Or => "or",
            Self::If => "if",
            Self::Unless => "unless",
            Self::Else => "else",
            Self::With => "with",
            Self::Without => "without",
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct ParseChecks(u8);

impl ParseChecks {
    const fn active() -> Self {
        Self(CHECK_ACTIVE)
    }

    const fn only_activation(self) -> Self {
        Self(self.0 & CHECK_ACTIVE)
    }

    fn contains(self, flag: u8) -> bool {
        self.0 & flag != 0
    }

    fn insert(&mut self, flag: u8) {
        self.0 |= flag;
    }

    fn remove(&mut self, flag: u8) {
        self.0 &= !flag;
    }

    fn merge(&mut self, other: Self) {
        self.0 |= other.0;
    }
}

/// Parse one RPM dependency according to the source tag represented by `kind`.
///
/// RPM validates `if`/`unless` context differently for conflict-like tags.
/// Carrying the typed relation kind into this boundary keeps that decision out
/// of package names, repository names, and caller-specific heuristics.
pub fn parse_rpm_dependency(
    kind: RepositoryRequirementKind,
    input: &str,
) -> Result<Expression, String> {
    parse_rpm_dependency_with_source_mode(kind, input, false)
}

/// Decode one dependency expression from RPM-owned source bytes.
///
/// RPM's `parseEVR()` treats an empty serialized epoch before `:` as epoch
/// zero. Only source ingestion uses this entry point; canonical and persisted
/// requirements continue through [`parse_rpm_dependency`] and reject that
/// spelling.
pub(crate) fn parse_source_rpm_dependency(
    kind: RepositoryRequirementKind,
    input: &str,
) -> Result<Expression, String> {
    parse_rpm_dependency_with_source_mode(kind, input, true)
}

fn parse_rpm_dependency_with_source_mode(
    kind: RepositoryRequirementKind,
    input: &str,
    canonicalize_source_evr: bool,
) -> Result<Expression, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("RPM dependency expression is empty".to_string());
    }

    let mut parser = Parser::new(input, canonicalize_source_evr);
    let expression = if parser.peek() == Some('(') {
        let mut checks = ParseChecks::active();
        let expression = parser.parse_rich(&mut checks)?;
        validate_tag_context(kind, checks)?;
        expression
    } else {
        parser.parse_simple()?
    };

    parser.skip_separators();
    if !parser.is_finished() {
        return Err(format!(
            "junk after RPM dependency expression: {}",
            parser.remaining()
        ));
    }
    Ok(expression)
}

struct Parser<'a> {
    input: &'a str,
    cursor: usize,
    canonicalize_source_evr: bool,
}

impl<'a> Parser<'a> {
    const fn new(input: &'a str, canonicalize_source_evr: bool) -> Self {
        Self {
            input,
            cursor: 0,
            canonicalize_source_evr,
        }
    }

    fn is_finished(&self) -> bool {
        self.cursor == self.input.len()
    }

    fn remaining(&self) -> &'a str {
        &self.input[self.cursor..]
    }

    fn peek(&self) -> Option<char> {
        self.remaining().chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let character = self.peek()?;
        self.cursor += character.len_utf8();
        Some(character)
    }

    fn skip_separators(&mut self) {
        while self
            .peek()
            .is_some_and(|ch| ch.is_whitespace() || ch == ',')
        {
            self.bump();
        }
    }

    fn parse_rich(&mut self, parent_checks: &mut ParseChecks) -> Result<Expression, String> {
        let expression_start = self.cursor;
        if self.bump() != Some('(') {
            return Err("RPM rich dependency does not start with '('".to_string());
        }

        let mut checks = *parent_checks;
        let mut operands = Vec::new();
        let mut operators = Vec::new();
        let mut chain_operator = None;
        let mut previous_operator = None;

        loop {
            self.skip_separators();
            if self.peek() == Some(')') {
                return Err(if chain_operator.is_some() {
                    "missing argument to RPM rich dependency operator".to_string()
                } else {
                    "RPM rich dependency has empty parentheses".to_string()
                });
            }

            let operand = if self.peek() == Some('(') {
                let mut nested_checks = checks.only_activation();
                let operand = self.parse_rich(&mut nested_checks)?;
                if matches!(
                    previous_operator,
                    Some(RichOperator::If | RichOperator::Unless)
                ) {
                    nested_checks.remove(CHECK_NO_AND | CHECK_NO_OR);
                }
                checks.merge(nested_checks);
                operand
            } else {
                self.parse_simple()?
            };
            operands.push(operand);

            self.skip_separators();
            match self.peek() {
                None => {
                    return Err(format!(
                        "unterminated RPM rich dependency: {}",
                        &self.input[expression_start..]
                    ));
                }
                Some(')') => break,
                Some(_) => {}
            }

            let operator = self.parse_rich_operator()?;
            if operator == RichOperator::Else
                && matches!(
                    chain_operator,
                    Some(RichOperator::If | RichOperator::Unless)
                )
            {
                chain_operator = None;
            }
            if let Some(chained) = chain_operator {
                if operator != chained {
                    return Err(format!(
                        "cannot chain different RPM rich dependency operators '{}' and '{}'",
                        chained.as_str(),
                        operator.as_str()
                    ));
                }
                if !matches!(
                    operator,
                    RichOperator::And | RichOperator::Or | RichOperator::With
                ) {
                    return Err(format!(
                        "RPM rich dependency operator '{}' cannot be chained",
                        operator.as_str()
                    ));
                }
            }
            chain_operator = Some(operator);
            previous_operator = Some(operator);
            operators.push(operator);
        }

        let first_operator = operators.first().copied();
        let last_operator = operators.last().copied();
        if let Some(operator) = first_operator {
            validate_parse_checks(operator, checks)?;
        }

        if first_operator == Some(RichOperator::If) {
            checks.insert(CHECK_NO_OR);
        }
        if first_operator == Some(RichOperator::Unless) {
            checks.insert(CHECK_NO_AND);
        }
        if matches!(last_operator, Some(RichOperator::And | RichOperator::Or)) {
            checks.remove(CHECK_NO_AND | CHECK_NO_OR);
        }
        if !matches!(
            last_operator,
            None | Some(RichOperator::With | RichOperator::Without | RichOperator::Or)
        ) {
            checks.insert(CHECK_NO_WITH);
        }

        debug_assert_eq!(self.peek(), Some(')'));
        self.bump();
        parent_checks.merge(checks);
        build_expression(operands, operators)
    }

    fn parse_simple(&mut self) -> Result<Expression, String> {
        let name = self.take_dependency_token();
        if name.is_empty() {
            return Err("RPM dependency name is required".to_string());
        }

        self.skip_separators();
        let comparison_start = self.cursor;
        let comparison = self.take_dependency_token();
        let Some(comparison) = canonical_comparison(comparison) else {
            self.cursor = comparison_start;
            return Ok(Expression::Atom(RepositoryRequirementClause::name_only(
                name.to_string(),
            )));
        };

        self.skip_separators();
        let source_version = self.take_dependency_token();
        let version = if self.canonicalize_source_evr {
            canonicalize_source_rpm_evr(source_version)?
        } else {
            source_version
        };
        if version.is_empty() {
            return Err(format!(
                "RPM dependency version is required after '{comparison}'"
            ));
        }
        Ok(Expression::Atom(RepositoryRequirementClause::versioned(
            name.to_string(),
            format!("{comparison} {version}"),
        )))
    }

    /// Match RPM's `SKIPNONWHITEX`: capability names may contain balanced
    /// parentheses, while a top-level right parenthesis ends the operand.
    fn take_dependency_token(&mut self) -> &'a str {
        let start = self.cursor;
        let mut nested_parentheses = 0_u32;
        while let Some(character) = self.peek() {
            if character.is_whitespace() || character == ',' {
                break;
            }
            match character {
                '(' => {
                    nested_parentheses += 1;
                    self.bump();
                }
                ')' if nested_parentheses == 0 => break,
                ')' => {
                    nested_parentheses -= 1;
                    self.bump();
                }
                _ => {
                    self.bump();
                }
            }
        }
        &self.input[start..self.cursor]
    }

    fn parse_rich_operator(&mut self) -> Result<RichOperator, String> {
        let start = self.cursor;
        while self
            .peek()
            .is_some_and(|character| !character.is_whitespace() && character != ')')
        {
            self.bump();
        }
        let token = &self.input[start..self.cursor];
        RichOperator::parse(token)
            .ok_or_else(|| format!("unknown RPM rich dependency operator '{token}'"))
    }
}

/// Canonicalize RPM's source-defined empty serialized epoch to omitted epoch
/// zero without weakening the repository-wide EVR grammar.
pub(crate) fn canonicalize_source_rpm_evr(version: &str) -> Result<&str, String> {
    let Some(version) = version.strip_prefix(':') else {
        return Ok(version);
    };
    if version.contains(':') {
        return Err("multiple epoch separators are not allowed".to_string());
    }
    Ok(version)
}

fn canonical_comparison(token: &str) -> Option<&'static str> {
    match token {
        "<=" | "=<" => Some("<="),
        "<" => Some("<"),
        "==" | "=" => Some("="),
        ">=" | "=>" => Some(">="),
        ">" => Some(">"),
        _ => None,
    }
}

fn validate_parse_checks(operator: RichOperator, checks: ParseChecks) -> Result<(), String> {
    if matches!(operator, RichOperator::With | RichOperator::Without)
        && checks.contains(CHECK_NO_WITH)
    {
        return Err("illegal RPM rich dependency operator inside 'with/without'".to_string());
    }
    if !checks.contains(CHECK_ACTIVE) {
        return Ok(());
    }
    if matches!(operator, RichOperator::And | RichOperator::If) && checks.contains(CHECK_NO_AND) {
        return Err("illegal RPM 'unless' context; use 'or' instead".to_string());
    }
    if matches!(operator, RichOperator::Or | RichOperator::Unless) && checks.contains(CHECK_NO_OR) {
        return Err("illegal RPM 'if' context; use 'and' instead".to_string());
    }
    Ok(())
}

fn validate_tag_context(
    kind: RepositoryRequirementKind,
    checks: ParseChecks,
) -> Result<(), String> {
    let tag_operator = match kind {
        RepositoryRequirementKind::Conflict
        | RepositoryRequirementKind::Supplements
        | RepositoryRequirementKind::Enhances => RichOperator::Or,
        RepositoryRequirementKind::Depends
        | RepositoryRequirementKind::PreDepends
        | RepositoryRequirementKind::Optional
        | RepositoryRequirementKind::Recommends
        | RepositoryRequirementKind::Suggests
        | RepositoryRequirementKind::Build
        | RepositoryRequirementKind::Breaks
        | RepositoryRequirementKind::Replace
        | RepositoryRequirementKind::Obsolete => RichOperator::And,
    };
    validate_parse_checks(tag_operator, checks)
}

fn build_expression(
    mut operands: Vec<Expression>,
    operators: Vec<RichOperator>,
) -> Result<Expression, String> {
    if operators.is_empty() {
        return operands
            .pop()
            .ok_or_else(|| "RPM dependency expression has no operand".to_string());
    }
    if operands.len() != operators.len() + 1 {
        return Err("RPM dependency expression has mismatched operands and operators".to_string());
    }

    match operators.as_slice() {
        [RichOperator::And, ..]
            if operators
                .iter()
                .all(|operator| *operator == RichOperator::And) =>
        {
            Ok(Expression::And(operands))
        }
        [RichOperator::Or, ..]
            if operators
                .iter()
                .all(|operator| *operator == RichOperator::Or) =>
        {
            Ok(Expression::Or(operands))
        }
        [RichOperator::With, ..]
            if operators
                .iter()
                .all(|operator| *operator == RichOperator::With) =>
        {
            let mut operands = operands.into_iter().rev();
            let mut expression = operands
                .next()
                .expect("operator count guarantees a final operand");
            for left in operands {
                expression = Expression::With {
                    left: Box::new(left),
                    right: Box::new(expression),
                };
            }
            Ok(expression)
        }
        [RichOperator::Without] => Ok(Expression::Without {
            left: Box::new(operands.remove(0)),
            right: Box::new(operands.remove(0)),
        }),
        [RichOperator::If] | [RichOperator::If, RichOperator::Else] => {
            let requirement = Box::new(operands.remove(0));
            let condition = Box::new(operands.remove(0));
            let otherwise = operands.pop().map(Box::new);
            Ok(Expression::If {
                requirement,
                condition,
                otherwise,
            })
        }
        [RichOperator::Unless] | [RichOperator::Unless, RichOperator::Else] => {
            let requirement = Box::new(operands.remove(0));
            let condition = Box::new(operands.remove(0));
            let otherwise = operands.pop().map(Box::new);
            Ok(Expression::Unless {
                requirement,
                condition,
                otherwise,
            })
        }
        [RichOperator::Else] => {
            Err("RPM dependency has 'else' without 'if' or 'unless'".to_string())
        }
        _ => Err("invalid RPM rich dependency operator sequence".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REQUIRES: RepositoryRequirementKind = RepositoryRequirementKind::Depends;

    fn parse(input: &str) -> Result<Expression, String> {
        parse_rpm_dependency(REQUIRES, input)
    }

    #[test]
    fn parses_every_rpm_boolean_operator_and_nesting() {
        for expression in [
            "(a and b and c)",
            "(a or (b and c))",
            "(a if b)",
            "(a if b else c)",
            "(a with b)",
            "(a without b)",
            "((a with b) or (c without d))",
            "((a >= 2 and (b or c)) if d)",
        ] {
            parse(expression).unwrap_or_else(|error| panic!("{expression}: {error}"));
        }
        for expression in ["(a unless b)", "(a unless b else c)"] {
            parse_rpm_dependency(RepositoryRequirementKind::Conflict, expression)
                .unwrap_or_else(|error| panic!("{expression}: {error}"));
        }
    }

    #[test]
    fn chained_with_is_right_associative_like_rpm() {
        let expression = parse("(a with b with c)").unwrap();
        assert_eq!(
            expression,
            Expression::With {
                left: Box::new(atom("a")),
                right: Box::new(Expression::With {
                    left: Box::new(atom("b")),
                    right: Box::new(atom("c")),
                }),
            }
        );
    }

    #[test]
    fn parses_fedora_44_chained_with_requirements() {
        let corpus = include_str!("../../tests/fixtures/rpm/fedora-44-rich-dependencies.txt");
        let expressions = corpus
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .collect::<Vec<_>>();
        assert_eq!(expressions.len(), 9, "fixture count drifted");
        for expression in expressions {
            parse_source_rpm_dependency(REQUIRES, expression)
                .unwrap_or_else(|error| panic!("{expression}: {error}"));
        }
    }

    #[test]
    fn canonicalizes_every_upstream_rpm_comparison_spelling() {
        for (operator, expected) in [
            ("<=", "<="),
            ("=<", "<="),
            ("<", "<"),
            ("==", "="),
            ("=", "="),
            (">=", ">="),
            ("=>", ">="),
            (">", ">"),
        ] {
            let Expression::Atom(clause) = parse(&format!("pkg {operator} 1")).unwrap() else {
                panic!("comparison must produce an atom");
            };
            assert_eq!(
                clause.version_constraint.as_deref(),
                Some(format!("{expected} 1").as_str())
            );
        }
        assert!(parse("pkg != 1").is_err());
    }

    #[test]
    fn enforces_upstream_chain_rules() {
        for expression in [
            "(a and b or c)",
            "(a without b without c)",
            "(a if b if c)",
            "(a if b unless c)",
        ] {
            assert!(parse(expression).is_err(), "{expression} should fail");
        }
        for expression in [
            "(a and b and c)",
            "(a or b or c)",
            "(a with b with c)",
            "(a if b else c)",
        ] {
            parse(expression).unwrap_or_else(|error| panic!("{expression}: {error}"));
        }
        parse_rpm_dependency(RepositoryRequirementKind::Conflict, "(a unless b else c)").unwrap();
    }

    #[test]
    fn enforces_upstream_tag_context_rules() {
        for kind in [
            RepositoryRequirementKind::Conflict,
            RepositoryRequirementKind::Supplements,
            RepositoryRequirementKind::Enhances,
        ] {
            parse_rpm_dependency(kind, "(a unless b)")
                .unwrap_or_else(|error| panic!("{kind:?}: {error}"));
            assert!(
                parse_rpm_dependency(kind, "(a if b)").is_err(),
                "{kind:?} must use RPM's reverse OR-context"
            );
        }
        for kind in [
            RepositoryRequirementKind::Depends,
            RepositoryRequirementKind::PreDepends,
            RepositoryRequirementKind::Optional,
            RepositoryRequirementKind::Recommends,
            RepositoryRequirementKind::Suggests,
            RepositoryRequirementKind::Build,
            RepositoryRequirementKind::Breaks,
            RepositoryRequirementKind::Replace,
            RepositoryRequirementKind::Obsolete,
        ] {
            parse_rpm_dependency(kind, "(a if b)")
                .unwrap_or_else(|error| panic!("{kind:?}: {error}"));
            assert!(
                parse_rpm_dependency(kind, "(a unless b)").is_err(),
                "{kind:?} must use RPM's default AND-context"
            );
        }
        assert!(parse("((a unless b) and c)").is_err());
        assert!(
            parse_rpm_dependency(RepositoryRequirementKind::Conflict, "((a if b) or c)").is_err()
        );

        parse("(a if b)").unwrap();
        parse_rpm_dependency(RepositoryRequirementKind::Conflict, "(a unless b)").unwrap();
        parse("((a if b) and c)").unwrap();
        parse_rpm_dependency(RepositoryRequirementKind::Conflict, "((a unless b) or c)").unwrap();
    }

    #[test]
    fn rejects_malformed_or_ambiguous_expressions() {
        for expression in [
            "",
            "()",
            "(a else b)",
            "(a or)",
            "(a >=)",
            "(a",
            "a or b",
            "(a and b) trailing",
            "((a and b) with c)",
        ] {
            assert!(parse(expression).is_err(), "{expression} should fail");
        }
    }

    #[test]
    fn accepts_parenthesized_capability_names_as_single_atoms() {
        let expression = parse("libc.so.6(GLIBC_2.34)(64bit)").unwrap();
        let Expression::Atom(clause) = expression else {
            panic!("parenthesized capability name must remain one atom");
        };
        assert_eq!(clause.name, "libc.so.6(GLIBC_2.34)(64bit)");
    }

    fn atom(name: &str) -> Expression {
        Expression::Atom(RepositoryRequirementClause::name_only(name.to_string()))
    }
}
