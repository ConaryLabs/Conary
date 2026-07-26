// conary-core/src/scriptlet/rpm_runtime/query_format.rs

//! RPM header query-format parser used by scriptlet `QFORMAT` transforms.

mod extensions;
mod formatters;

use super::RpmMacroEngine;
#[cfg(test)]
use crate::ccs::native_lifecycle::RpmMacroContext;
use crate::ccs::native_lifecycle::{RpmHeaderContext, RpmHeaderFact, RpmHeaderValue};
use anyhow::{Context, Result, bail};
use base64::Engine as _;
use num_traits::FromPrimitive as _;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

#[cfg(test)]
pub(super) fn format(
    input: &str,
    header: &RpmHeaderContext,
    macros: &RpmMacroContext,
) -> Result<String> {
    let macros = RpmMacroEngine::from_context(macros);
    format_with_engine(input, header, &macros)
}

pub(super) fn format_with_engine(
    input: &str,
    header: &RpmHeaderContext,
    macros: &RpmMacroEngine,
) -> Result<String> {
    Formatter { header, macros }.render(input, None)
}

struct Formatter<'a> {
    header: &'a RpmHeaderContext,
    macros: &'a RpmMacroEngine,
}

#[derive(Debug, Clone, Copy)]
struct Iteration {
    index: usize,
}

#[derive(Debug)]
struct Placeholder<'a> {
    consumed: usize,
    width: Option<i32>,
    locked: bool,
    tag: &'a str,
    output_format: Option<&'a str>,
}

impl Formatter<'_> {
    fn render(&self, input: &str, iteration: Option<Iteration>) -> Result<String> {
        let mut output = String::with_capacity(input.len());
        let mut offset = 0usize;
        while offset < input.len() {
            let rest = &input[offset..];
            if let Some(after_escape) = rest.strip_prefix('\\') {
                let character = after_escape.chars().next().ok_or_else(|| {
                    anyhow::anyhow!("RpmQueryFormatSyntaxError: trailing escape at byte {offset}")
                })?;
                match character {
                    'a' => output.push('\u{0007}'),
                    'b' => output.push('\u{0008}'),
                    'n' => output.push('\n'),
                    't' => output.push('\t'),
                    'r' => output.push('\r'),
                    'f' => output.push('\u{000c}'),
                    'v' => output.push('\u{000b}'),
                    '\\' | '{' | '}' | '[' | ']' | '%' => output.push(character),
                    '0' => bail!(
                        "RpmQueryFormatSyntaxError: NUL escapes are forbidden at byte {offset}"
                    ),
                    other => output.push(other),
                }
                offset += 1 + character.len_utf8();
                continue;
            }
            if rest.starts_with("%%") {
                output.push('%');
                offset += 2;
                continue;
            }
            if rest.starts_with("%|") {
                let (consumed, branch) = self.parse_expression(rest)?;
                output.push_str(&self.render(branch, iteration)?);
                offset += consumed;
                continue;
            }
            if rest.starts_with('%') {
                let placeholder = parse_placeholder(rest).with_context(|| {
                    format!("RpmQueryFormatSyntaxError: placeholder at byte {offset}")
                })?;
                output.push_str(&self.render_placeholder(&placeholder, iteration)?);
                offset += placeholder.consumed;
                continue;
            }
            if let Some(after_open) = rest.strip_prefix('[') {
                let close = matching_square(after_open).ok_or_else(|| {
                    anyhow::anyhow!(
                        "RpmQueryFormatSyntaxError: unterminated iterator at byte {offset}"
                    )
                })?;
                let inner = &after_open[..close];
                let length = self.iterator_length(inner)?;
                for index in 0..length {
                    output.push_str(&self.render(inner, Some(Iteration { index }))?);
                }
                offset += 1 + close + 1;
                continue;
            }

            let character = rest
                .chars()
                .next()
                .expect("non-empty query remainder has a character");
            output.push(character);
            offset += character.len_utf8();
        }
        Ok(output)
    }

    fn render_placeholder(
        &self,
        placeholder: &Placeholder<'_>,
        iteration: Option<Iteration>,
    ) -> Result<String> {
        let Some(fact) = self.fact(placeholder.tag)? else {
            return Ok(apply_width("(none)", placeholder.width));
        };
        let value_index = if placeholder.locked {
            0
        } else {
            iteration.map_or(0, |iteration| iteration.index)
        };
        let value = self.format_fact(&fact, value_index, placeholder.output_format)?;
        Ok(apply_width(&value, placeholder.width))
    }

    fn format_fact(
        &self,
        fact: &RpmHeaderFact,
        index: usize,
        output_format: Option<&str>,
    ) -> Result<String> {
        if output_format == Some("arraysize") {
            return Ok(fact_len(fact).to_string());
        }
        if output_format == Some("tagname") {
            return Ok(fact.name.clone().unwrap_or_else(|| fact.tag.to_string()));
        }
        if output_format == Some("tagnum") {
            return Ok(fact.tag.to_string());
        }

        let scalar = fact_scalar(fact, index);
        let Some(scalar) = scalar else {
            return Ok("(none)".to_string());
        };
        match output_format.unwrap_or("string") {
            "string" => Ok(scalar.as_string()),
            "base64" => Ok(match &scalar {
                Scalar::Binary(value) => base64::engine::general_purpose::STANDARD.encode(value),
                _ => "(not a blob)".to_string(),
            }),
            "armor" => formatters::armor(&fact.value, index),
            "pgpsig" => formatters::pgpsig(&fact.value, index, &self.header.format_environment),
            "hex" => Ok(number_format(&scalar, |value| format!("{value:x}"))),
            "octal" => Ok(number_format(&scalar, |value| format!("{value:o}"))),
            "date" => match scalar {
                Scalar::Integer(value) => formatters::date(value, &self.header.format_environment),
                _ => Ok("(not a number)".to_string()),
            },
            "day" => match scalar {
                Scalar::Integer(value) => formatters::day(value, &self.header.format_environment),
                _ => Ok("(not a number)".to_string()),
            },
            "json" => scalar.json(),
            "shescape" => Ok(match &scalar {
                Scalar::Integer(value) => value.to_string(),
                Scalar::String(value) => shell_escape(value),
                Scalar::Binary(_) => "(invalid type)".to_string(),
            }),
            "perms" | "permissions" => Ok(number_format(&scalar, rpm_permissions)),
            "depflags" => Ok(number_format(&scalar, rpm_dependency_flags)),
            "deptype" => Ok(number_format(&scalar, rpm_dependency_type)),
            "fflags" => Ok(number_format(&scalar, rpm_file_flags)),
            "fstate" => Ok(number_format(&scalar, rpm_file_state)),
            "fstatus" => Ok(number_format(&scalar, |value| rpm_verify_flags(value, '.'))),
            "vflags" => Ok(number_format(&scalar, |value| {
                rpm_verify_flags(value, '\0')
            })),
            "triggertype" => Ok(number_format(&scalar, rpm_trigger_type)),
            "hashalgo" => Ok(number_format(&scalar, rpm_hash_algorithm)),
            "humansi" => Ok(number_format(&scalar, |value| human_size(value, 1000))),
            "humaniec" => Ok(number_format(&scalar, |value| human_size(value, 1024))),
            "expand" => match &scalar {
                Scalar::String(value) => self
                    .macros
                    .expand(value)
                    .map_err(|error| anyhow::anyhow!("RpmQueryFormatExpandFailed: {error:#}")),
                _ => Ok("(not a string)".to_string()),
            },
            "xml" => Ok(rpm_xml(&scalar)),
            formatter => bail!(
                "RpmQueryFormatFormatterError: unknown formatter '{formatter}' for tag {} ({})",
                fact.tag,
                fact.name.as_deref().unwrap_or("unnamed")
            ),
        }
    }

    fn iterator_length(&self, input: &str) -> Result<usize> {
        let placeholders = collect_placeholders(input)?;
        let mut lengths = BTreeSet::new();
        for placeholder in placeholders
            .iter()
            .filter(|placeholder| !placeholder.locked)
        {
            if let Some(fact) = self.fact(placeholder.tag)? {
                lengths.insert(fact_len(&fact));
            }
        }
        if lengths.is_empty() {
            bail!("RpmQueryFormatIteratorError: iterator contains no unlocked header tag");
        }
        if lengths.len() != 1 {
            bail!(
                "RpmQueryFormatIteratorError: unlocked header arrays have mismatched lengths {lengths:?}"
            );
        }
        Ok(*lengths
            .first()
            .expect("non-empty iterator length set has a first element"))
    }

    fn parse_expression<'a>(&self, input: &'a str) -> Result<(usize, &'a str)> {
        let expression = input
            .strip_prefix("%|")
            .expect("expression parser receives %| prefix");
        let question = expression.find("?{").ok_or_else(|| {
            anyhow::anyhow!("RpmQueryFormatExpressionError: missing '?{{' operator")
        })?;
        let tag = &expression[..question];
        if tag.is_empty() || !valid_tag_reference(tag) {
            bail!("RpmQueryFormatExpressionError: invalid condition tag '{tag}'");
        }
        let present_start = question + 2;
        let present_close =
            matching_plain_brace(&expression[present_start..]).ok_or_else(|| {
                anyhow::anyhow!("RpmQueryFormatExpressionError: unterminated present branch")
            })?;
        let after_present = present_start + present_close + 1;
        let absent = expression
            .get(after_present..)
            .and_then(|rest| rest.strip_prefix(":{"))
            .ok_or_else(|| {
                anyhow::anyhow!("RpmQueryFormatExpressionError: missing absent branch")
            })?;
        let absent_close = matching_plain_brace(absent).ok_or_else(|| {
            anyhow::anyhow!("RpmQueryFormatExpressionError: unterminated absent branch")
        })?;
        if absent.as_bytes().get(absent_close + 1) != Some(&b'|') {
            bail!("RpmQueryFormatExpressionError: expression is missing closing '|'");
        }
        let consumed = 2 + after_present + 2 + absent_close + 2;
        let present = &expression[present_start..present_start + present_close];
        let absent = &absent[..absent_close];
        Ok((
            consumed,
            if self.fact(tag)?.is_some() {
                present
            } else {
                absent
            },
        ))
    }

    fn fact(&self, query: &str) -> Result<Option<RpmHeaderFact>> {
        let normalized = query.to_ascii_uppercase();
        let query = normalized.strip_prefix("RPMTAG_").unwrap_or(&normalized);
        if let Ok(tag) = query.parse::<u32>() {
            if let Some(index_tag) = rpm::IndexTag::from_u32(tag) {
                match extensions::resolve(self.header, index_tag)? {
                    extensions::ExtensionFact::Value(fact) => return Ok(Some(fact)),
                    extensions::ExtensionFact::Missing => return Ok(None),
                    extensions::ExtensionFact::NotExtension => {}
                }
            }
            return Ok(self
                .header
                .facts
                .iter()
                .find(|fact| fact.tag == tag)
                .cloned());
        }
        if let Some(tag) = RPM_TAGS_BY_NAME.get(query).copied() {
            match extensions::resolve(self.header, tag)? {
                extensions::ExtensionFact::Value(fact) => return Ok(Some(fact)),
                extensions::ExtensionFact::Missing => return Ok(None),
                extensions::ExtensionFact::NotExtension => {}
            }
            return Ok(self
                .header
                .facts
                .iter()
                .find(|fact| {
                    fact.tag == tag as u32
                        || fact
                            .name
                            .as_deref()
                            .is_some_and(|name| name.eq_ignore_ascii_case(query))
                })
                .cloned());
        }
        if let Some(fact) = self.header.facts.iter().find(|fact| {
            fact.name
                .as_deref()
                .is_some_and(|name| name.eq_ignore_ascii_case(query))
        }) {
            return Ok(Some(fact.clone()));
        }
        bail!("RpmQueryFormatTagError: unknown tag '{query}'")
    }
}

static RPM_TAGS_BY_NAME: LazyLock<BTreeMap<String, rpm::IndexTag>> = LazyLock::new(|| {
    (0..=rpm::IndexTag::RPMTAG_PAYLOAD_SHA3_256_ALT as u32)
        .filter_map(rpm::IndexTag::from_u32)
        .filter_map(|tag| {
            tag.to_string()
                .strip_prefix("RPMTAG_")
                .map(|name| (name.to_string(), tag))
        })
        .collect()
});

fn parse_placeholder(input: &str) -> Result<Placeholder<'_>> {
    let bytes = input.as_bytes();
    if bytes.first() != Some(&b'%') {
        bail!("placeholder does not start with '%'");
    }
    let mut offset = 1usize;
    let width_start = offset;
    if bytes.get(offset) == Some(&b'-') {
        offset += 1;
    }
    while bytes.get(offset).is_some_and(|byte| byte.is_ascii_digit()) {
        offset += 1;
    }
    let width = if offset == width_start {
        None
    } else {
        Some(
            input[width_start..offset]
                .parse::<i32>()
                .context("invalid width")?,
        )
    };
    if bytes.get(offset) != Some(&b'{') {
        bail!("expected '{{' after width");
    }
    let content_start = offset + 1;
    let close = input[content_start..]
        .find('}')
        .ok_or_else(|| anyhow::anyhow!("unterminated placeholder"))?;
    let content_end = content_start + close;
    let mut content = &input[content_start..content_end];
    let locked = content.starts_with('=') || content.starts_with('#');
    let array_size = content.starts_with('#');
    if locked {
        content = &content[1..];
    }
    let (tag, explicit_output_format) = content
        .split_once(':')
        .map_or((content, None), |(tag, format)| (tag, Some(format)));
    let output_format = explicit_output_format.or(array_size.then_some("arraysize"));
    if !valid_tag_reference(tag) {
        bail!("invalid tag reference '{tag}'");
    }
    if output_format.is_some_and(|formatter| formatter.is_empty()) {
        bail!("empty output formatter for tag '{tag}'");
    }
    Ok(Placeholder {
        consumed: content_end + 1,
        width,
        locked,
        tag,
        output_format,
    })
}

fn collect_placeholders(input: &str) -> Result<Vec<Placeholder<'_>>> {
    let mut placeholders = Vec::new();
    let mut offset = 0usize;
    while offset < input.len() {
        let rest = &input[offset..];
        if let Some(stripped) = rest.strip_prefix('\\') {
            let escaped = stripped
                .chars()
                .next()
                .ok_or_else(|| anyhow::anyhow!("trailing escape"))?;
            offset += 1 + escaped.len_utf8();
        } else if rest.starts_with("%%") {
            offset += 2;
        } else if rest.starts_with("%|") {
            // Nested expression branches are rendered later and therefore do
            // not establish the outer iterator's cardinality.
            let end = expression_end(rest)?;
            offset += end;
        } else if rest.starts_with('%') {
            let placeholder = parse_placeholder(rest)?;
            offset += placeholder.consumed;
            placeholders.push(placeholder);
        } else {
            let character = rest
                .chars()
                .next()
                .expect("non-empty query remainder has a character");
            offset += character.len_utf8();
        }
    }
    Ok(placeholders)
}

fn expression_end(input: &str) -> Result<usize> {
    let expression = input
        .strip_prefix("%|")
        .ok_or_else(|| anyhow::anyhow!("expression does not start with %|"))?;
    let question = expression
        .find("?{")
        .ok_or_else(|| anyhow::anyhow!("expression has no ternary"))?;
    let present_start = question + 2;
    let present_close = matching_plain_brace(&expression[present_start..])
        .ok_or_else(|| anyhow::anyhow!("unterminated present branch"))?;
    let absent = expression
        .get(present_start + present_close + 1..)
        .and_then(|rest| rest.strip_prefix(":{"))
        .ok_or_else(|| anyhow::anyhow!("missing absent branch"))?;
    let absent_close = matching_plain_brace(absent)
        .ok_or_else(|| anyhow::anyhow!("unterminated absent branch"))?;
    if absent.as_bytes().get(absent_close + 1) != Some(&b'|') {
        bail!("expression is missing closing '|'");
    }
    Ok(2 + present_start + present_close + 1 + 2 + absent_close + 2)
}

fn matching_square(input: &str) -> Option<usize> {
    let mut nested = 0usize;
    let mut escaped = false;
    for (offset, character) in input.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '[' => nested += 1,
            ']' if nested == 0 => return Some(offset),
            ']' => nested -= 1,
            _ => {}
        }
    }
    None
}

fn matching_plain_brace(input: &str) -> Option<usize> {
    let mut nested = 0usize;
    let mut escaped = false;
    for (offset, character) in input.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '{' => nested += 1,
            '}' if nested == 0 => return Some(offset),
            '}' => nested -= 1,
            _ => {}
        }
    }
    None
}

fn valid_tag_reference(tag: &str) -> bool {
    !tag.is_empty()
        && tag
            .bytes()
            .all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
}

fn fact_len(fact: &RpmHeaderFact) -> usize {
    match &fact.value {
        RpmHeaderValue::Null => 0,
        RpmHeaderValue::Integer(values) => values.len(),
        RpmHeaderValue::StringArray(values) | RpmHeaderValue::I18nString(values) => values.len(),
        RpmHeaderValue::Binary(_) | RpmHeaderValue::String(_) => 1,
    }
}

enum Scalar<'a> {
    Binary(Vec<u8>),
    Integer(u64),
    String(&'a str),
}

impl Scalar<'_> {
    fn as_string(&self) -> String {
        match self {
            Self::Binary(value) => hex::encode(value),
            Self::Integer(value) => value.to_string(),
            Self::String(value) => (*value).to_string(),
        }
    }

    fn json(&self) -> Result<String> {
        match self {
            Self::Binary(value) => {
                serde_json::to_string(&base64::engine::general_purpose::STANDARD.encode(value))
            }
            Self::Integer(value) => Ok(value.to_string()),
            Self::String(value) => serde_json::to_string(value),
        }
        .context("RpmQueryFormatJsonEncodingFailed")
    }
}

fn fact_scalar(fact: &RpmHeaderFact, index: usize) -> Option<Scalar<'_>> {
    match &fact.value {
        RpmHeaderValue::Null => None,
        RpmHeaderValue::Binary(value) => base64::engine::general_purpose::STANDARD
            .decode(value)
            .ok()
            .map(Scalar::Binary),
        RpmHeaderValue::Integer(values) => values.get(index).copied().map(Scalar::Integer),
        RpmHeaderValue::String(value) => (index == 0).then_some(Scalar::String(value)),
        RpmHeaderValue::StringArray(values) | RpmHeaderValue::I18nString(values) => {
            values.get(index).map(|value| Scalar::String(value))
        }
    }
}

fn apply_width(value: &str, width: Option<i32>) -> String {
    let Some(width) = width else {
        return value.to_string();
    };
    let target = width.unsigned_abs() as usize;
    // RPM applies C `printf("%*s")` width semantics to the rendered UTF-8
    // bytes, not Unicode scalar-value display width.
    let padding = target.saturating_sub(value.len());
    if width < 0 {
        format!("{value}{}", " ".repeat(padding))
    } else {
        format!("{}{value}", " ".repeat(padding))
    }
}

fn number_format(scalar: &Scalar<'_>, formatter: impl FnOnce(u64) -> String) -> String {
    match scalar {
        Scalar::Integer(value) => formatter(*value),
        _ => "(not a number)".to_string(),
    }
}

fn shell_escape(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn rpm_dependency_flags(value: u64) -> String {
    let mut output = String::new();
    if value & (1 << 1) != 0 {
        output.push('<');
    }
    if value & (1 << 2) != 0 {
        output.push('>');
    }
    if value & (1 << 3) != 0 {
        output.push('=');
    }
    output
}

fn rpm_dependency_type(value: u64) -> String {
    let mut types = Vec::new();
    for (bit, name) in [
        (1 << 9, "pre"),
        (1 << 10, "post"),
        (1 << 11, "preun"),
        (1 << 12, "postun"),
        (1 << 13, "verify"),
        (1 << 8, "interp"),
        (1 << 24, "rpmlib"),
    ] {
        if value & bit != 0 {
            types.push(name);
        }
    }
    if value & ((1 << 14) | (1 << 15)) != 0 {
        types.push("auto");
    }
    for (bit, name) in [
        (1 << 6, "prereq"),
        (1 << 7, "pretrans"),
        (1 << 5, "posttrans"),
        (1 << 20, "preuntrans"),
        (1 << 21, "postuntrans"),
        (1 << 28, "config"),
        (1 << 19, "missingok"),
        (1 << 29, "meta"),
    ] {
        if value & bit != 0 {
            types.push(name);
        }
    }
    if types.is_empty() {
        "manual".to_string()
    } else {
        types.join(",")
    }
}

fn rpm_file_flags(value: u64) -> String {
    [
        (1 << 1, 'd'),
        (1, 'c'),
        (1 << 5, 's'),
        (1 << 3, 'm'),
        (1 << 4, 'n'),
        (1 << 6, 'g'),
        (1 << 7, 'l'),
        (1 << 8, 'r'),
        (1 << 12, 'a'),
    ]
    .into_iter()
    .filter_map(|(bit, marker)| (value & bit != 0).then_some(marker))
    .collect()
}

fn rpm_file_state(value: u64) -> String {
    match value {
        0 => "normal",
        1 => "replaced",
        2 => "not installed",
        3 => "net shared",
        4 => "wrong color",
        255 | u64::MAX => "missing",
        _ => "(unknown)",
    }
    .to_string()
}

fn rpm_verify_flags(value: u64, padding: char) -> String {
    let read_link_failed = value & (1 << 28) != 0;
    let read_failed = value & (1 << 29) != 0;
    [
        (1 << 1, 'S', false),
        (1 << 6, 'M', false),
        (1, '5', read_failed),
        (1 << 7, 'D', false),
        (1 << 2, 'L', read_link_failed),
        (1 << 3, 'U', false),
        (1 << 4, 'G', false),
        (1 << 5, 'T', false),
        (1 << 8, 'P', false),
    ]
    .into_iter()
    .filter_map(|(bit, marker, unreadable)| {
        if unreadable {
            Some('?')
        } else if value & bit != 0 {
            Some(marker)
        } else if padding == '\0' {
            None
        } else {
            Some(padding)
        }
    })
    .collect()
}

fn rpm_trigger_type(value: u64) -> String {
    for (bit, kind) in [
        (1 << 25, "prein"),
        (1 << 16, "in"),
        (1 << 17, "un"),
        (1 << 18, "postun"),
    ] {
        if value & bit != 0 {
            return kind.to_string();
        }
    }
    String::new()
}

fn rpm_hash_algorithm(value: u64) -> String {
    match value {
        1 => "MD5",
        2 => "SHA1",
        3 => "RIPEMD160",
        5 => "MD2",
        6 => "TIGER192",
        7 => "HAVAL-5-160",
        8 => "SHA256",
        9 => "SHA384",
        10 => "SHA512",
        11 => "SHA224",
        12 => "SHA3-256",
        14 => "SHA3-512",
        _ => "Unknown",
    }
    .to_string()
}

fn rpm_xml(scalar: &Scalar<'_>) -> String {
    let (kind, value) = match scalar {
        Scalar::Binary(value) => (
            "base64",
            base64::engine::general_purpose::STANDARD.encode(value),
        ),
        Scalar::Integer(value) => ("integer", value.to_string()),
        Scalar::String(value) => ("string", (*value).to_string()),
    };
    if value.is_empty() {
        format!("\t<{kind}/>")
    } else {
        let escaped = value
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        format!("\t<{kind}>{escaped}</{kind}>")
    }
}

fn rpm_permissions(mode: u64) -> String {
    let mut output = ['-'; 10];
    output[0] = match mode & 0o170000 {
        0o100000 => '-',
        0o040000 => 'd',
        0o120000 => 'l',
        0o010000 => 'p',
        0o140000 => 's',
        0o020000 => 'c',
        0o060000 => 'b',
        _ => '?',
    };
    for (bit, index, marker) in [
        (0o400, 1, 'r'),
        (0o200, 2, 'w'),
        (0o100, 3, 'x'),
        (0o040, 4, 'r'),
        (0o020, 5, 'w'),
        (0o010, 6, 'x'),
        (0o004, 7, 'r'),
        (0o002, 8, 'w'),
        (0o001, 9, 'x'),
    ] {
        if mode & bit != 0 {
            output[index] = marker;
        }
    }
    if mode & 0o4000 != 0 {
        output[3] = if mode & 0o100 != 0 { 's' } else { 'S' };
    }
    if mode & 0o2000 != 0 {
        output[6] = if mode & 0o010 != 0 { 's' } else { 'S' };
    }
    if mode & 0o1000 != 0 {
        output[9] = if mode & 0o001 != 0 { 't' } else { 'T' };
    }
    output.into_iter().collect()
}

fn human_size(value: u64, base: u64) -> String {
    const UNITS: [&str; 7] = ["", "K", "M", "G", "T", "P", "E"];
    let mut number = value as f64;
    let mut unit = 0usize;
    while number >= base as f64 && unit + 1 < UNITS.len() {
        number /= base as f64;
        unit += 1;
    }
    let decimals = usize::from(number > 0.05 && number < 9.95);
    format!("{number:.decimals$}{}", UNITS[unit])
}

#[cfg(test)]
mod tests;
