// conary-core/src/repository/rpm_dependency.rs

//! Formal parser for RPM boolean dependency expressions.

use super::dependency_model::{
    RepositoryRequirementClause, RepositoryRequirementExpression as Expression,
};

/// Parse one RPM dependency name or parenthesized boolean expression.
pub fn parse_rpm_dependency(input: &str) -> Result<Expression, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("RPM dependency expression is empty".to_string());
    }
    let unwrapped = strip_outer_parentheses(input)?;
    if unwrapped == input && !top_level_operators(input)?.is_empty() {
        return Err(format!(
            "RPM boolean dependency must be enclosed in parentheses: {input}"
        ));
    }
    let expression = parse_expression(input)?;
    validate_operator_nesting(&expression)?;
    Ok(expression)
}

fn parse_expression(input: &str) -> Result<Expression, String> {
    let input = strip_outer_parentheses(input)?;
    let operators = top_level_operators(input)?;

    if let Some(index) = unique_operator(&operators, "if")? {
        reject_operators(&operators, &["or", "unless"], "if")?;
        let else_index = unique_operator(&operators, "else")?;
        let condition_end = else_index.unwrap_or(input.len());
        return Ok(Expression::If {
            requirement: Box::new(parse_expression(input[..index].trim())?),
            condition: Box::new(parse_expression(
                input[index + "if".len()..condition_end].trim(),
            )?),
            otherwise: else_index
                .map(|index| parse_expression(input[index + "else".len()..].trim()).map(Box::new))
                .transpose()?,
        });
    }

    if let Some(index) = unique_operator(&operators, "unless")? {
        reject_operators(&operators, &["and", "if"], "unless")?;
        let else_index = unique_operator(&operators, "else")?;
        let condition_end = else_index.unwrap_or(input.len());
        return Ok(Expression::Unless {
            requirement: Box::new(parse_expression(input[..index].trim())?),
            condition: Box::new(parse_expression(
                input[index + "unless".len()..condition_end].trim(),
            )?),
            otherwise: else_index
                .map(|index| parse_expression(input[index + "else".len()..].trim()).map(Box::new))
                .transpose()?,
        });
    }

    if operators.iter().any(|(_, operator)| *operator == "else") {
        return Err(format!(
            "RPM dependency has 'else' without 'if' or 'unless': {input}"
        ));
    }

    if operators.iter().any(|(_, operator)| *operator == "or") {
        reject_operators(&operators, &["and", "with", "without"], "or")?;
        return parse_chain(input, &operators, "or", Expression::Or);
    }
    if operators.iter().any(|(_, operator)| *operator == "and") {
        reject_operators(&operators, &["with", "without"], "and")?;
        return parse_chain(input, &operators, "and", Expression::And);
    }
    if let Some(index) = unique_operator(&operators, "with")? {
        reject_operators(&operators, &["without"], "with")?;
        return Ok(Expression::With {
            left: Box::new(parse_expression(input[..index].trim())?),
            right: Box::new(parse_expression(input[index + "with".len()..].trim())?),
        });
    }
    if let Some(index) = unique_operator(&operators, "without")? {
        return Ok(Expression::Without {
            left: Box::new(parse_expression(input[..index].trim())?),
            right: Box::new(parse_expression(input[index + "without".len()..].trim())?),
        });
    }

    parse_atom(input)
}

fn strip_outer_parentheses(input: &str) -> Result<&str, String> {
    if !input.starts_with('(') {
        return Ok(input);
    }
    let mut depth = 0_u32;
    for (index, character) in input.char_indices() {
        match character {
            '(' => depth += 1,
            ')' => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| format!("unbalanced RPM dependency parentheses: {input}"))?;
                if depth == 0 && index + character.len_utf8() != input.len() {
                    return Ok(input);
                }
            }
            _ => {}
        }
    }
    if depth != 0 || !input.ends_with(')') {
        return Err(format!("unbalanced RPM dependency parentheses: {input}"));
    }
    parse_balanced_inner(&input[1..input.len() - 1])
}

fn parse_balanced_inner(input: &str) -> Result<&str, String> {
    if input.trim().is_empty() {
        Err("RPM dependency expression has empty parentheses".to_string())
    } else {
        Ok(input.trim())
    }
}

fn top_level_operators(input: &str) -> Result<Vec<(usize, &str)>, String> {
    const OPERATORS: [&str; 7] = ["and", "or", "if", "unless", "else", "with", "without"];
    let mut depth = 0_u32;
    let mut operators = Vec::new();
    let mut word_start = None;
    for (index, character) in input
        .char_indices()
        .chain(std::iter::once((input.len(), ' ')))
    {
        match character {
            '(' => {
                depth += 1;
                word_start = None;
            }
            ')' => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| format!("unbalanced RPM dependency parentheses: {input}"))?;
                word_start = None;
            }
            character if depth == 0 && character.is_whitespace() => {
                if let Some(start) = word_start.take() {
                    let word = &input[start..index];
                    if let Some(operator) = OPERATORS.iter().find(|operator| **operator == word) {
                        operators.push((start, *operator));
                    }
                }
            }
            _ if depth == 0 => {
                word_start.get_or_insert(index);
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err(format!("unbalanced RPM dependency parentheses: {input}"));
    }
    Ok(operators)
}

fn unique_operator(operators: &[(usize, &str)], expected: &str) -> Result<Option<usize>, String> {
    let matches = operators
        .iter()
        .filter(|(_, operator)| *operator == expected)
        .map(|(index, _)| *index)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Ok(None),
        [index] => Ok(Some(*index)),
        _ => Err(format!(
            "RPM dependency repeats binary operator '{expected}'"
        )),
    }
}

fn reject_operators(
    operators: &[(usize, &str)],
    rejected: &[&str],
    context: &str,
) -> Result<(), String> {
    if let Some((_, operator)) = operators
        .iter()
        .find(|(_, operator)| rejected.contains(operator))
    {
        return Err(format!(
            "RPM dependency mixes '{context}' and '{operator}' without nested parentheses"
        ));
    }
    Ok(())
}

fn parse_chain(
    input: &str,
    operators: &[(usize, &str)],
    expected: &str,
    constructor: fn(Vec<Expression>) -> Expression,
) -> Result<Expression, String> {
    let positions = operators
        .iter()
        .filter(|(_, operator)| *operator == expected)
        .map(|(index, _)| *index)
        .collect::<Vec<_>>();
    let mut operands = Vec::with_capacity(positions.len() + 1);
    let mut start = 0;
    for index in positions {
        operands.push(parse_expression(input[start..index].trim())?);
        start = index + expected.len();
    }
    operands.push(parse_expression(input[start..].trim())?);
    Ok(constructor(operands))
}

fn parse_atom(input: &str) -> Result<Expression, String> {
    let parts = input.split_whitespace().collect::<Vec<_>>();
    let clause = match parts.as_slice() {
        [name] if !name.is_empty() => RepositoryRequirementClause::name_only((*name).to_string()),
        [name, operator, version] if matches!(*operator, "<" | "<=" | "=" | ">=" | ">" | "!=") => {
            RepositoryRequirementClause::versioned(
                (*name).to_string(),
                format!("{operator} {version}"),
            )
        }
        _ => return Err(format!("invalid RPM dependency atom: {input}")),
    };
    Ok(Expression::Atom(clause))
}

fn validate_operator_nesting(expression: &Expression) -> Result<(), String> {
    match expression {
        Expression::And(operands) => {
            if operands.iter().any(contains_unless) {
                return Err(
                    "RPM dependency cannot nest 'unless' inside an 'and' expression".to_string(),
                );
            }
            for operand in operands {
                validate_operator_nesting(operand)?;
            }
        }
        Expression::Or(operands) => {
            if operands.iter().any(contains_if) {
                return Err("RPM dependency cannot nest 'if' inside an 'or' expression".to_string());
            }
            for operand in operands {
                validate_operator_nesting(operand)?;
            }
        }
        Expression::If {
            requirement,
            condition,
            otherwise,
        }
        | Expression::Unless {
            requirement,
            condition,
            otherwise,
        } => {
            validate_operator_nesting(requirement)?;
            validate_operator_nesting(condition)?;
            if let Some(otherwise) = otherwise {
                validate_operator_nesting(otherwise)?;
            }
        }
        Expression::With { left, right } | Expression::Without { left, right } => {
            validate_operator_nesting(left)?;
            validate_operator_nesting(right)?;
        }
        Expression::Atom(_) => {}
    }
    Ok(())
}

fn contains_if(expression: &Expression) -> bool {
    match expression {
        Expression::If { .. } => true,
        Expression::And(operands) | Expression::Or(operands) => operands.iter().any(contains_if),
        Expression::Unless {
            requirement,
            condition,
            otherwise,
        } => {
            contains_if(requirement)
                || contains_if(condition)
                || otherwise.as_deref().is_some_and(contains_if)
        }
        Expression::With { left, right } | Expression::Without { left, right } => {
            contains_if(left) || contains_if(right)
        }
        Expression::Atom(_) => false,
    }
}

fn contains_unless(expression: &Expression) -> bool {
    match expression {
        Expression::Unless { .. } => true,
        Expression::And(operands) | Expression::Or(operands) => {
            operands.iter().any(contains_unless)
        }
        Expression::If {
            requirement,
            condition,
            otherwise,
        } => {
            contains_unless(requirement)
                || contains_unless(condition)
                || otherwise.as_deref().is_some_and(contains_unless)
        }
        Expression::With { left, right } | Expression::Without { left, right } => {
            contains_unless(left) || contains_unless(right)
        }
        Expression::Atom(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_rpm_boolean_operator_and_nesting() {
        for expression in [
            "(a and b and c)",
            "(a or (b and c))",
            "(a if b)",
            "(a if b else c)",
            "(a unless b)",
            "(a unless b else c)",
            "(a with b)",
            "(a without b)",
            "((a with b) or (c without d))",
            "((a >= 2 and (b or c)) if d)",
        ] {
            parse_rpm_dependency(expression)
                .unwrap_or_else(|error| panic!("{expression}: {error}"));
        }
    }

    #[test]
    fn rejects_malformed_or_ambiguous_expressions() {
        for expression in [
            "",
            "()",
            "(a if b if c)",
            "(a if b or c)",
            "(a unless b and c)",
            "(a else b)",
            "(a or)",
            "(a >=)",
            "(a",
            "a or b",
            "((a if b) or c)",
            "((a unless b) and c)",
        ] {
            assert!(
                parse_rpm_dependency(expression).is_err(),
                "{expression} should fail"
            );
        }
    }

    #[test]
    fn accepts_parenthesized_capability_names_as_single_atoms() {
        let expression = parse_rpm_dependency("libc.so.6(GLIBC_2.34)(64bit)").unwrap();
        let Expression::Atom(clause) = expression else {
            panic!("parenthesized capability name must remain one atom");
        };
        assert_eq!(clause.name, "libc.so.6(GLIBC_2.34)(64bit)");
    }
}
