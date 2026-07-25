// conary-core/src/scriptlet/rpm_runtime/macro_expand/expression.rs

//! Recursive-descent implementation of RPM's integer/string/version expression grammar.

use super::RpmMacroEngine;
use crate::repository::versioning::{VersionScheme, compare_repo_versions};
use anyhow::{Result, bail};
use mlua::{Lua, LuaOptions, MultiValue, StdLib};
use std::cmp::Ordering;

pub(super) fn evaluate(
    engine: &RpmMacroEngine,
    input: &str,
    depth: usize,
    expand_macros: bool,
) -> Result<String> {
    let source = if expand_macros {
        engine.expand_depth(input, depth + 1, false)?
    } else {
        input.to_string()
    };
    if expand_macros && source.trim().is_empty() {
        return Ok("0".to_string());
    }
    let tokens = tokenize(&source)?;
    let mut parser = Parser { tokens, offset: 0 };
    let expression = parser.ternary()?;
    if !matches!(parser.peek(), Token::End) {
        bail!("RpmMacroExpressionError: trailing input in '{source}'");
    }
    expression.validate_types()?;
    Ok(expression.evaluate()?.render())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueKind {
    Integer,
    String,
    Version,
}

#[derive(Debug, Clone, PartialEq)]
enum Value {
    Integer(i64),
    String(String),
    Version(String),
}

impl Value {
    fn truthy(&self) -> bool {
        match self {
            Self::Integer(value) => *value != 0,
            Self::String(value) => !value.is_empty(),
            Self::Version(_) => false,
        }
    }

    fn render(self) -> String {
        match self {
            Self::Integer(value) => value.to_string(),
            Self::String(value) | Self::Version(value) => value,
        }
    }

    fn same_kind(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (Self::Integer(_), Self::Integer(_))
                | (Self::String(_), Self::String(_))
                | (Self::Version(_), Self::Version(_))
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BinaryOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnaryOperator {
    Negate,
    Not,
}

#[derive(Debug, Clone)]
enum Expression {
    Literal(Value),
    Unary {
        operator: UnaryOperator,
        value: Box<Self>,
    },
    Binary {
        operator: BinaryOperator,
        left: Box<Self>,
        right: Box<Self>,
    },
    Ternary {
        condition: Box<Self>,
        then_value: Box<Self>,
        else_value: Box<Self>,
    },
    Function {
        name: String,
        arguments: Vec<Self>,
    },
}

impl Expression {
    fn validate_types(&self) -> Result<ValueKind> {
        match self {
            Self::Literal(Value::Integer(_)) => Ok(ValueKind::Integer),
            Self::Literal(Value::String(_)) => Ok(ValueKind::String),
            Self::Literal(Value::Version(_)) => Ok(ValueKind::Version),
            Self::Unary { operator, value } => {
                let kind = value.validate_types()?;
                match operator {
                    UnaryOperator::Not => Ok(ValueKind::Integer),
                    UnaryOperator::Negate if kind == ValueKind::Integer => Ok(ValueKind::Integer),
                    UnaryOperator::Negate => {
                        bail!("RpmMacroExpressionError: unary - requires an integer")
                    }
                }
            }
            Self::Binary {
                operator,
                left,
                right,
            } => {
                let left = left.validate_types()?;
                let right = right.validate_types()?;
                if left != right {
                    bail!("RpmMacroExpressionError: operand types must match");
                }
                match operator {
                    BinaryOperator::And | BinaryOperator::Or => Ok(left),
                    BinaryOperator::Equal
                    | BinaryOperator::NotEqual
                    | BinaryOperator::Less
                    | BinaryOperator::LessEqual
                    | BinaryOperator::Greater
                    | BinaryOperator::GreaterEqual => Ok(ValueKind::Integer),
                    BinaryOperator::Multiply | BinaryOperator::Divide
                        if left == ValueKind::Integer =>
                    {
                        Ok(ValueKind::Integer)
                    }
                    BinaryOperator::Multiply | BinaryOperator::Divide => {
                        bail!("RpmMacroExpressionError: * and / require integer operands")
                    }
                    BinaryOperator::Add if left != ValueKind::Version => Ok(left),
                    BinaryOperator::Subtract if left == ValueKind::Integer => {
                        Ok(ValueKind::Integer)
                    }
                    BinaryOperator::Add | BinaryOperator::Subtract => {
                        bail!("RpmMacroExpressionError: operator does not support these operands")
                    }
                }
            }
            Self::Ternary {
                condition,
                then_value,
                else_value,
            } => {
                condition.validate_types()?;
                let then_kind = then_value.validate_types()?;
                let else_kind = else_value.validate_types()?;
                if then_kind != else_kind {
                    bail!("RpmMacroExpressionError: ternary branch types must match");
                }
                Ok(then_kind)
            }
            Self::Function { arguments, .. } => {
                for argument in arguments {
                    if argument.validate_types()? == ValueKind::Version {
                        bail!(
                            "RpmMacroExpressionError: Lua functions do not accept version values"
                        );
                    }
                }
                Ok(ValueKind::String)
            }
        }
    }

    fn evaluate(&self) -> Result<Value> {
        match self {
            Self::Literal(value) => Ok(value.clone()),
            Self::Unary { operator, value } => {
                let value = value.evaluate()?;
                match operator {
                    UnaryOperator::Not => Ok(Value::Integer(i64::from(!value.truthy()))),
                    UnaryOperator::Negate => match value {
                        Value::Integer(value) => Ok(Value::Integer(-value)),
                        _ => bail!("RpmMacroExpressionError: unary - requires an integer"),
                    },
                }
            }
            Self::Binary {
                operator,
                left,
                right,
            } => {
                let left = left.evaluate()?;
                if *operator == BinaryOperator::And && !left.truthy() {
                    return Ok(left);
                }
                if *operator == BinaryOperator::Or && left.truthy() {
                    return Ok(left);
                }
                let right = right.evaluate()?;
                if !left.same_kind(&right) {
                    bail!("RpmMacroExpressionError: operand types must match");
                }
                evaluate_binary(*operator, left, right)
            }
            Self::Ternary {
                condition,
                then_value,
                else_value,
            } => {
                if condition.evaluate()?.truthy() {
                    then_value.evaluate()
                } else {
                    else_value.evaluate()
                }
            }
            Self::Function { name, arguments } => {
                let arguments = arguments
                    .iter()
                    .map(Self::evaluate)
                    .collect::<Result<Vec<_>>>()?;
                Ok(Value::String(lua_string_function(name, &arguments)?))
            }
        }
    }
}

fn lua_string_function(name: &str, arguments: &[Value]) -> Result<String> {
    let name = name
        .strip_prefix("lua:")
        .ok_or_else(|| anyhow::anyhow!("RpmMacroExpressionError: unsupported function '{name}'"))?;
    let lua = Lua::new_with(
        StdLib::STRING | StdLib::TABLE | StdLib::MATH | StdLib::UTF8,
        LuaOptions::default(),
    )
    .map_err(expression_lua_error)?;
    let function = lua
        .load(format!("return ({name})(...)"))
        .into_function()
        .map_err(expression_lua_error)?;
    let mut values = Vec::with_capacity(arguments.len());
    for argument in arguments {
        values.push(match argument {
            Value::Integer(value) => mlua::Value::Integer(*value),
            Value::String(value) => {
                mlua::Value::String(lua.create_string(value).map_err(expression_lua_error)?)
            }
            Value::Version(_) => unreachable!("version arguments rejected by validation"),
        });
    }
    let result = function
        .call::<mlua::Value>(MultiValue::from_vec(values))
        .map_err(expression_lua_error)?;
    match result {
        mlua::Value::Nil => Ok(String::new()),
        mlua::Value::Boolean(value) => Ok(if value { "1" } else { "" }.to_string()),
        value => {
            let tostring: mlua::Function = lua
                .globals()
                .get("tostring")
                .map_err(expression_lua_error)?;
            tostring.call(value).map_err(expression_lua_error)
        }
    }
}

fn expression_lua_error(error: mlua::Error) -> anyhow::Error {
    anyhow::anyhow!("RpmMacroExpressionLuaError: {error}")
}

fn evaluate_binary(operator: BinaryOperator, left: Value, right: Value) -> Result<Value> {
    match operator {
        BinaryOperator::And | BinaryOperator::Or => Ok(right),
        BinaryOperator::Multiply | BinaryOperator::Divide => match (left, right) {
            (Value::Integer(left), Value::Integer(right)) => {
                if operator == BinaryOperator::Divide && right == 0 {
                    bail!("RpmMacroExpressionError: division by zero");
                }
                Ok(Value::Integer(if operator == BinaryOperator::Multiply {
                    left * right
                } else {
                    left / right
                }))
            }
            (Value::Version(_), Value::Version(_)) => {
                bail!("RpmMacroExpressionError: * and / do not support versions")
            }
            _ => bail!("RpmMacroExpressionError: * and / do not support strings"),
        },
        BinaryOperator::Add | BinaryOperator::Subtract => match (left, right) {
            (Value::Integer(left), Value::Integer(right)) => {
                Ok(Value::Integer(if operator == BinaryOperator::Add {
                    left + right
                } else {
                    left - right
                }))
            }
            (Value::String(mut left), Value::String(right)) if operator == BinaryOperator::Add => {
                left.push_str(&right);
                Ok(Value::String(left))
            }
            (Value::Version(_), Value::Version(_)) => {
                bail!("RpmMacroExpressionError: + and - do not support versions")
            }
            _ => bail!("RpmMacroExpressionError: - does not support strings"),
        },
        _ => {
            let ordering = match (&left, &right) {
                (Value::Integer(left), Value::Integer(right)) => left.cmp(right),
                (Value::String(left), Value::String(right)) => left.cmp(right),
                (Value::Version(left), Value::Version(right)) => {
                    compare_repo_versions(VersionScheme::Rpm, left, right)?
                }
                _ => unreachable!("caller checked matching value kinds"),
            };
            let result = match operator {
                BinaryOperator::Equal => ordering == Ordering::Equal,
                BinaryOperator::NotEqual => ordering != Ordering::Equal,
                BinaryOperator::Less => ordering == Ordering::Less,
                BinaryOperator::LessEqual => ordering != Ordering::Greater,
                BinaryOperator::Greater => ordering == Ordering::Greater,
                BinaryOperator::GreaterEqual => ordering != Ordering::Less,
                _ => unreachable!("arithmetic and logical operators handled above"),
            };
            Ok(Value::Integer(i64::from(result)))
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Value(Value),
    Plus,
    Minus,
    Star,
    Slash,
    Open,
    Close,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Not,
    And,
    Or,
    Question,
    Colon,
    Comma,
    Function(String),
    End,
}

fn tokenize(input: &str) -> Result<Vec<Token>> {
    let bytes = input.as_bytes();
    let mut tokens = Vec::new();
    let mut offset = 0usize;
    while offset < bytes.len() {
        if bytes[offset].is_ascii_whitespace() {
            offset += 1;
            continue;
        }
        let (token, consumed) = match &bytes[offset..] {
            [b'=', b'=', ..] => (Token::Equal, 2),
            [b'!', b'=', ..] => (Token::NotEqual, 2),
            [b'<', b'=', ..] => (Token::LessEqual, 2),
            [b'>', b'=', ..] => (Token::GreaterEqual, 2),
            [b'&', b'&', ..] => (Token::And, 2),
            [b'|', b'|', ..] => (Token::Or, 2),
            [b'+', ..] => (Token::Plus, 1),
            [b'-', ..] => (Token::Minus, 1),
            [b'*', ..] => (Token::Star, 1),
            [b'/', ..] => (Token::Slash, 1),
            [b'(', ..] => (Token::Open, 1),
            [b')', ..] => (Token::Close, 1),
            [b'<', ..] => (Token::Less, 1),
            [b'>', ..] => (Token::Greater, 1),
            [b'!', ..] => (Token::Not, 1),
            [b'?', ..] => (Token::Question, 1),
            [b':', ..] => (Token::Colon, 1),
            [b',', ..] => (Token::Comma, 1),
            [b'"', ..] => {
                let end = input[offset + 1..]
                    .find('"')
                    .map(|relative| offset + 1 + relative)
                    .ok_or_else(|| {
                        anyhow::anyhow!("RpmMacroExpressionError: unterminated string")
                    })?;
                (
                    Token::Value(Value::String(input[offset + 1..end].to_string())),
                    end + 1 - offset,
                )
            }
            [b'v', b'"', ..] => {
                let end = input[offset + 2..]
                    .find('"')
                    .map(|relative| offset + 2 + relative)
                    .ok_or_else(|| {
                        anyhow::anyhow!("RpmMacroExpressionError: unterminated version")
                    })?;
                let version = &input[offset + 2..end];
                if version.is_empty() {
                    bail!("RpmMacroExpressionError: empty version");
                }
                (
                    Token::Value(Value::Version(version.to_string())),
                    end + 1 - offset,
                )
            }
            [digit, ..] if digit.is_ascii_digit() => {
                let end = bytes[offset..]
                    .iter()
                    .position(|byte| !byte.is_ascii_digit())
                    .map_or(bytes.len(), |relative| offset + relative);
                (
                    Token::Value(Value::Integer(input[offset..end].parse()?)),
                    end - offset,
                )
            }
            [first, ..] if first.is_ascii_alphabetic() || *first == b'_' => {
                let mut end = offset + 1;
                while bytes.get(end).is_some_and(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b':')
                }) {
                    end += 1;
                }
                if bytes.get(end) != Some(&b'(') {
                    bail!("RpmMacroExpressionError: bare word at byte {offset} in '{input}'");
                }
                (
                    Token::Function(input[offset..end].to_string()),
                    end + 1 - offset,
                )
            }
            _ => {
                bail!(
                    "RpmMacroExpressionError: bare or invalid token at byte {offset} in '{input}'"
                )
            }
        };
        tokens.push(token);
        offset += consumed;
    }
    tokens.push(Token::End);
    Ok(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    offset: usize,
}

impl Parser {
    fn peek(&self) -> &Token {
        &self.tokens[self.offset]
    }

    fn take(&mut self) -> Token {
        let token = self.tokens[self.offset].clone();
        self.offset += 1;
        token
    }

    fn ternary(&mut self) -> Result<Expression> {
        let condition = self.logical()?;
        if !matches!(self.peek(), Token::Question) {
            return Ok(condition);
        }
        self.take();
        let then_value = self.ternary()?;
        if !matches!(self.take(), Token::Colon) {
            bail!("RpmMacroExpressionError: ternary expression requires ':'");
        }
        let else_value = self.ternary()?;
        Ok(Expression::Ternary {
            condition: Box::new(condition),
            then_value: Box::new(then_value),
            else_value: Box::new(else_value),
        })
    }

    fn logical(&mut self) -> Result<Expression> {
        let mut expression = self.relational()?;
        loop {
            let operator = match self.peek() {
                Token::And => BinaryOperator::And,
                Token::Or => BinaryOperator::Or,
                _ => return Ok(expression),
            };
            self.take();
            expression = Expression::Binary {
                operator,
                left: Box::new(expression),
                right: Box::new(self.relational()?),
            };
        }
    }

    fn relational(&mut self) -> Result<Expression> {
        let mut expression = self.additive()?;
        loop {
            let operator = match self.peek() {
                Token::Equal => BinaryOperator::Equal,
                Token::NotEqual => BinaryOperator::NotEqual,
                Token::Less => BinaryOperator::Less,
                Token::LessEqual => BinaryOperator::LessEqual,
                Token::Greater => BinaryOperator::Greater,
                Token::GreaterEqual => BinaryOperator::GreaterEqual,
                _ => return Ok(expression),
            };
            self.take();
            expression = Expression::Binary {
                operator,
                left: Box::new(expression),
                right: Box::new(self.additive()?),
            };
        }
    }

    fn additive(&mut self) -> Result<Expression> {
        let mut expression = self.multiplicative()?;
        loop {
            let operator = match self.peek() {
                Token::Plus => BinaryOperator::Add,
                Token::Minus => BinaryOperator::Subtract,
                _ => return Ok(expression),
            };
            self.take();
            expression = Expression::Binary {
                operator,
                left: Box::new(expression),
                right: Box::new(self.multiplicative()?),
            };
        }
    }

    fn multiplicative(&mut self) -> Result<Expression> {
        let mut expression = self.primary()?;
        loop {
            let operator = match self.peek() {
                Token::Star => BinaryOperator::Multiply,
                Token::Slash => BinaryOperator::Divide,
                _ => return Ok(expression),
            };
            self.take();
            expression = Expression::Binary {
                operator,
                left: Box::new(expression),
                right: Box::new(self.primary()?),
            };
        }
    }

    fn primary(&mut self) -> Result<Expression> {
        match self.take() {
            Token::Value(value) => Ok(Expression::Literal(value)),
            Token::Minus => Ok(Expression::Unary {
                operator: UnaryOperator::Negate,
                value: Box::new(self.primary()?),
            }),
            Token::Not => Ok(Expression::Unary {
                operator: UnaryOperator::Not,
                value: Box::new(self.primary()?),
            }),
            Token::Open => {
                let expression = self.ternary()?;
                if !matches!(self.take(), Token::Close) {
                    bail!("RpmMacroExpressionError: unmatched '('");
                }
                Ok(expression)
            }
            Token::Function(name) => {
                let mut arguments = Vec::new();
                if matches!(self.peek(), Token::Close) {
                    self.take();
                } else {
                    loop {
                        arguments.push(self.ternary()?);
                        match self.take() {
                            Token::Comma => {}
                            Token::Close => break,
                            token => {
                                bail!("RpmMacroExpressionError: expected ',' or ')', got {token:?}")
                            }
                        }
                    }
                }
                Ok(Expression::Function { name, arguments })
            }
            Token::End => bail!("RpmMacroExpressionError: unexpected end of expression"),
            token => bail!("RpmMacroExpressionError: unexpected token {token:?}"),
        }
    }
}
