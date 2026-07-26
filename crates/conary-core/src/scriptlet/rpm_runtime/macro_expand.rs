// conary-core/src/scriptlet/rpm_runtime/macro_expand.rs

//! RPM macro parser and scoped expansion engine.
//!
//! The grammar follows RPM's `rpmio/macro.cc`: macros are parsed structurally,
//! unknown calls remain literal, parameter frames own automatic macros, and
//! runtime effects are delegated to an explicit target-root authority.

mod builtins;
mod expression;
mod parameters;
mod runtime;

use self::builtins::Builtin;
use self::parameters::{ParameterContext, automatic_macros, split_words};
use self::runtime::RpmMacroRuntime;
use crate::ccs::native_lifecycle::{RpmMacroContext, RpmRuntimeMetadata};
use crate::scriptlet::ScriptletExecutor;
use anyhow::{Result, bail};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

const MAX_MACRO_DEPTH: usize = 64;
const QUOTE_MARKER: char = '\u{001f}';

#[derive(Debug, Clone)]
struct MacroDefinition {
    body: String,
    options: Option<String>,
    literal: bool,
    oneshot: bool,
}

#[derive(Debug, Default)]
struct MacroFrame {
    automatic: BTreeMap<String, String>,
    definitions: BTreeMap<String, Vec<MacroDefinition>>,
    call: MacroCallContext,
}

#[derive(Debug, Clone, Default)]
pub(super) struct MacroCallContext {
    pub(super) arguments: Vec<String>,
    pub(super) options: BTreeMap<String, String>,
}

#[derive(Debug, Default)]
struct MacroState {
    definitions: BTreeMap<String, Vec<MacroDefinition>>,
    frames: Vec<MacroFrame>,
}

/// A mutable RPM macro context. Clones share definitions and parameter frames,
/// matching the single context used throughout one RPM body-transform chain.
#[derive(Clone)]
pub(crate) struct RpmMacroEngine {
    state: Rc<RefCell<MacroState>>,
    runtime: Option<Rc<RpmMacroRuntime>>,
}

impl RpmMacroEngine {
    #[cfg(test)]
    pub(super) fn from_context(context: &RpmMacroContext) -> Self {
        Self::new(context, None)
    }

    pub(super) fn from_runtime(
        executor: &ScriptletExecutor,
        environment: &[(String, String)],
        metadata: &RpmRuntimeMetadata,
    ) -> Self {
        Self::new(
            &metadata.macro_context,
            Some(RpmMacroRuntime::new(executor, environment, metadata)),
        )
    }

    fn new(context: &RpmMacroContext, runtime: Option<RpmMacroRuntime>) -> Self {
        let engine = Self {
            state: Rc::new(RefCell::new(MacroState::default())),
            runtime: runtime.map(Rc::new),
        };

        // These definitions are shipped by upstream RPM's macros.in and are
        // part of the language contract rather than a distro-specific macro
        // configuration.
        engine.define_global("nil", "", None);
        engine.define_global(
            "defined",
            "%{expand:%%{?%{1}:1}%%{!?%{1}:0}}",
            Some(String::new()),
        );
        engine.define_global(
            "undefined",
            "%{expand:%%{?%{1}:0}%%{!?%{1}:1}}",
            Some(String::new()),
        );
        engine.define_global(
            "with",
            "%{expand:%%{?with_%{1}:1}%%{!?with_%{1}:0}}",
            Some(String::new()),
        );
        engine.define_global(
            "without",
            "%{expand:%%{?with_%{1}:0}%%{!?with_%{1}:1}}",
            Some(String::new()),
        );
        for definition in &context.definitions {
            engine.define_global(&definition.name, &definition.body, None);
        }
        engine
    }

    pub(super) fn expand(&self, input: &str) -> Result<String> {
        Ok(self
            .expand_depth(input, 0, false)?
            .replace(QUOTE_MARKER, ""))
    }

    fn expand_depth(&self, input: &str, depth: usize, keep_quoted: bool) -> Result<String> {
        if depth >= MAX_MACRO_DEPTH {
            bail!("RpmMacroRecursionError: more than {MAX_MACRO_DEPTH} levels of macro expansion");
        }

        let mut output = String::with_capacity(input.len());
        let mut offset = 0usize;
        while offset < input.len() {
            let rest = &input[offset..];
            if !rest.starts_with('%') {
                let character = rest
                    .chars()
                    .next()
                    .expect("non-empty macro input has a character");
                output.push(character);
                offset += character.len_utf8();
                continue;
            }
            if rest.starts_with("%%") {
                output.push('%');
                offset += 2;
                continue;
            }
            if rest.starts_with("%(") {
                let end = matching_delimiter(input, offset + 1, b'(', b')').ok_or_else(|| {
                    anyhow::anyhow!(
                        "RpmMacroSyntaxError: unterminated shell expansion at byte {offset}"
                    )
                })?;
                let command = self.expand_depth(&input[offset + 2..end - 1], depth + 1, false)?;
                let runtime = self.runtime.as_ref().ok_or_else(|| {
                    anyhow::anyhow!(
                        "RpmMacroRuntimeContextMissing: shell expansion requires an install target"
                    )
                })?;
                output.push_str(&runtime.shell_expand(&command)?);
                offset = end;
                continue;
            }
            if rest.starts_with("%[") {
                let end = matching_delimiter(input, offset + 1, b'[', b']').ok_or_else(|| {
                    anyhow::anyhow!(
                        "RpmMacroSyntaxError: unterminated expression expansion at byte {offset}"
                    )
                })?;
                output.push_str(&expression::evaluate(
                    self,
                    &input[offset + 2..end - 1],
                    depth + 1,
                    true,
                )?);
                offset = end;
                continue;
            }
            if rest.starts_with("%{") {
                let end = matching_delimiter(input, offset + 1, b'{', b'}').ok_or_else(|| {
                    anyhow::anyhow!(
                        "RpmMacroSyntaxError: unterminated braced macro at byte {offset}"
                    )
                })?;
                let call = &input[offset + 2..end - 1];
                let literal = &input[offset..end];
                output.push_str(&self.expand_named(
                    call,
                    literal,
                    CallStyle::Braced,
                    depth + 1,
                    keep_quoted,
                )?);
                offset = end;
                continue;
            }

            let Some((name_end, modifiers)) = unbraced_name(input, offset + 1) else {
                output.push('%');
                offset += 1;
                continue;
            };
            let name = &input[modifiers.name_start..name_end];
            let builtin = Builtin::from_name(name);
            let definition = self.lookup(name);
            let parses_arguments = builtin.is_some()
                || definition
                    .as_ref()
                    .is_some_and(|definition| definition.options.is_some());
            let free_field = builtin.is_some_and(Builtin::consumes_free_field);
            let mut end = name_end;
            if free_field {
                end = free_field_end(
                    input,
                    name_end,
                    builtin.is_some_and(Builtin::consumes_all_definition_eols),
                );
            } else if !builtin.is_some_and(Builtin::takes_raw_argument)
                && parses_arguments
                && input
                    .as_bytes()
                    .get(name_end)
                    .is_some_and(u8::is_ascii_whitespace)
            {
                end = argument_line_end(input, name_end);
            }
            let literal = &input[offset..end];
            let call = &input[offset + 1..end];
            output.push_str(&self.expand_named(
                call,
                literal,
                CallStyle::Unbraced,
                depth + 1,
                keep_quoted,
            )?);
            offset = end;
        }
        Ok(output)
    }

    fn expand_named(
        &self,
        call: &str,
        literal: &str,
        style: CallStyle,
        depth: usize,
        keep_quoted: bool,
    ) -> Result<String> {
        let parsed = ParsedCall::parse(call, style)?;
        if parsed.name.is_empty() {
            return Ok(literal.to_string());
        }

        let definition = self.lookup(parsed.name);
        let builtin = Builtin::from_name(parsed.name);
        let defined = definition.is_some() || builtin.is_some();
        if parsed.conditional || parsed.name.starts_with('-') {
            let selected = if parsed.negated { !defined } else { defined };
            if !selected {
                return Ok(String::new());
            }
            return match parsed.arguments {
                Some(arguments) if !arguments.text.is_empty() => {
                    self.expand_depth(arguments.text, depth + 1, keep_quoted)
                }
                _ => match definition {
                    Some(definition) => {
                        self.expand_definition(parsed.name, &definition, &[], depth)
                    }
                    None if builtin.is_some() => {
                        builtin
                            .expect("checked builtin")
                            .execute(self, &[], "", depth, keep_quoted)
                    }
                    None => Ok(String::new()),
                },
            };
        }

        if let Some(builtin) = builtin {
            let raw = parsed.arguments.map_or("", |arguments| arguments.text);
            let arguments = if builtin.takes_raw_argument() {
                Vec::new()
            } else {
                self.expand_arguments(parsed.arguments, depth, keep_quoted)?
            };
            return builtin.execute(self, &arguments, raw, depth, keep_quoted);
        }

        let Some(definition) = definition else {
            if matches!(style, CallStyle::Braced) && call.contains('%') {
                let call = self.expand_depth(call, depth + 1, keep_quoted)?;
                return Ok(format!("%{{{call}}}"));
            }
            return Ok(literal.to_string());
        };
        let arguments = self.expand_arguments(parsed.arguments, depth, true)?;
        self.expand_definition(parsed.name, &definition, &arguments, depth)
    }

    fn expand_arguments(
        &self,
        arguments: Option<CallArguments<'_>>,
        depth: usize,
        keep_quoted: bool,
    ) -> Result<Vec<String>> {
        let Some(arguments) = arguments else {
            return Ok(Vec::new());
        };
        let expanded = self.expand_depth(arguments.text, depth + 1, keep_quoted)?;
        match arguments.kind {
            ArgumentKind::Single => Ok(vec![expanded]),
            ArgumentKind::Words => split_words(&expanded),
        }
    }

    fn expand_definition(
        &self,
        name: &str,
        definition: &MacroDefinition,
        arguments: &[String],
        depth: usize,
    ) -> Result<String> {
        let Some(options) = definition.options.as_deref() else {
            if definition.literal {
                return Ok(definition.body.clone());
            }
            let result = self.expand_depth(&definition.body, depth + 1, false)?;
            if definition.oneshot {
                self.cache_oneshot(name, &result);
            }
            return Ok(result);
        };
        let ParameterContext {
            automatic,
            arguments,
            options,
        } = automatic_macros(name, options, arguments)?;
        self.state.borrow_mut().frames.push(MacroFrame {
            automatic,
            definitions: BTreeMap::new(),
            call: MacroCallContext { arguments, options },
        });
        let result = self.expand_depth(&definition.body, depth + 1, false);
        self.state
            .borrow_mut()
            .frames
            .pop()
            .expect("parameter frame was pushed");
        result
    }

    fn lookup(&self, name: &str) -> Option<MacroDefinition> {
        let state = self.state.borrow();
        for frame in state.frames.iter().rev() {
            if let Some(value) = frame.automatic.get(name) {
                return Some(MacroDefinition {
                    body: value.clone(),
                    options: None,
                    literal: true,
                    oneshot: false,
                });
            }
            if let Some(definition) = frame
                .definitions
                .get(name)
                .and_then(|definitions| definitions.last())
            {
                return Some(definition.clone());
            }
        }
        state
            .definitions
            .get(name)
            .and_then(|definitions| definitions.last())
            .cloned()
    }

    pub(super) fn definition(&self, name: &str) -> Option<String> {
        self.lookup(name).map(|definition| definition.body)
    }

    pub(super) fn is_defined(&self, name: &str) -> bool {
        Builtin::from_name(name).is_some() || self.lookup(name).is_some()
    }

    pub(super) fn is_parametric(&self, name: &str) -> bool {
        Builtin::from_name(name).is_some_and(Builtin::is_parametric)
            || self
                .lookup(name)
                .is_some_and(|definition| definition.options.is_some())
    }

    pub(super) fn define_body(&self, name: String, body: String) -> Result<()> {
        validate_definition_name(&name)?;
        if Builtin::from_name(&name).is_some() {
            bail!("RpmMacroDefinitionError: %{name} is a builtin");
        }
        self.state
            .borrow_mut()
            .definitions
            .entry(name)
            .or_default()
            .push(MacroDefinition {
                body,
                options: None,
                literal: false,
                oneshot: false,
            });
        Ok(())
    }

    pub(super) fn define_programmatic(&self, specification: &str) -> Result<()> {
        self.define_spec(specification, true, false, 0)
    }

    pub(super) fn expand_macro_args(&self, name: &str, arguments: &[String]) -> Result<String> {
        if let Some(builtin) = Builtin::from_name(name) {
            return builtin
                .execute(self, arguments, &arguments.join(" "), 1, false)
                .map(|value| value.replace(QUOTE_MARKER, ""));
        }
        let definition = self
            .lookup(name)
            .ok_or_else(|| anyhow::anyhow!("RpmMacroArgumentError: no such macro '%{name}'"))?;
        if definition.options.is_none() {
            bail!("RpmMacroArgumentError: macro '%{name}' is not parametric");
        }
        self.expand_definition(name, &definition, arguments, 1)
            .map(|value| value.replace(QUOTE_MARKER, ""))
    }

    pub(super) fn current_call_context(&self) -> MacroCallContext {
        self.state
            .borrow()
            .frames
            .last()
            .map(|frame| frame.call.clone())
            .unwrap_or_default()
    }

    pub(super) fn lua_runtime_context(&self) -> Option<(ScriptletExecutor, Vec<(String, String)>)> {
        self.runtime
            .as_ref()
            .map(|runtime| (runtime.executor(), runtime.environment_values()))
    }

    pub(super) fn split_arguments(input: &str) -> Result<Vec<String>> {
        split_words(input)
    }

    fn define_spec(
        &self,
        specification: &str,
        global: bool,
        expand_body: bool,
        depth: usize,
    ) -> Result<()> {
        let mut global = global;
        let mut expand_body = expand_body;
        let mut specification = specification.trim_start();
        if !global {
            loop {
                let end = specification
                    .find(char::is_whitespace)
                    .unwrap_or(specification.len());
                let option = &specification[..end];
                if !option.starts_with('-') {
                    break;
                }
                if option.len() == 1 {
                    bail!("RpmMacroDefinitionError: %define has no option after '-'");
                }
                for flag in option[1..].bytes() {
                    match flag {
                        b'e' => expand_body = true,
                        b'g' => global = true,
                        flag => bail!(
                            "RpmMacroDefinitionError: %define option '-{}' is invalid",
                            char::from(flag)
                        ),
                    }
                }
                specification = specification[end..].trim_start();
            }
        }
        let (name, options, body, literal, oneshot) = parse_definition(specification)?;
        if Builtin::from_name(&name).is_some() {
            bail!("RpmMacroDefinitionError: %{name} is a builtin");
        }
        let body = if expand_body && !literal {
            self.expand_depth(&body, depth + 1, true)?
        } else {
            body
        };
        let definition = MacroDefinition {
            body,
            options,
            literal,
            oneshot,
        };
        if global {
            self.state
                .borrow_mut()
                .definitions
                .entry(name)
                .or_default()
                .push(definition);
        } else {
            self.define_current(name, definition);
        }
        Ok(())
    }

    fn define_current(&self, name: String, definition: MacroDefinition) {
        let mut state = self.state.borrow_mut();
        if let Some(frame) = state.frames.last_mut() {
            frame.definitions.entry(name).or_default().push(definition);
        } else {
            state.definitions.entry(name).or_default().push(definition);
        }
    }

    fn define_global(&self, name: &str, body: &str, options: Option<String>) {
        self.push_global_definition(name, body, options, false);
    }

    fn define_global_literal(&self, name: &str, body: &str) {
        self.push_global_definition(name, body, None, true);
    }

    fn push_global_definition(
        &self,
        name: &str,
        body: &str,
        options: Option<String>,
        literal: bool,
    ) {
        self.state
            .borrow_mut()
            .definitions
            .entry(name.to_string())
            .or_default()
            .push(MacroDefinition {
                body: body.to_string(),
                options,
                literal,
                oneshot: false,
            });
    }

    fn cache_oneshot(&self, name: &str, expansion: &str) {
        let mut state = self.state.borrow_mut();
        for frame in state.frames.iter_mut().rev() {
            if let Some(definition) = frame
                .definitions
                .get_mut(name)
                .and_then(|definitions| definitions.last_mut())
            {
                definition.body = expansion.to_string();
                definition.literal = true;
                definition.oneshot = false;
                return;
            }
        }
        if let Some(definition) = state
            .definitions
            .get_mut(name)
            .and_then(|definitions| definitions.last_mut())
        {
            definition.body = expansion.to_string();
            definition.literal = true;
            definition.oneshot = false;
        }
    }

    pub(super) fn undefine(&self, name: &str) -> Result<()> {
        if Builtin::from_name(name).is_some() {
            bail!("RpmMacroDefinitionError: %{name} is a builtin");
        }
        validate_definition_name(name)?;
        let mut state = self.state.borrow_mut();
        if let Some(frame) = state.frames.last_mut()
            && let Some(definitions) = frame.definitions.get_mut(name)
        {
            definitions.pop();
            if definitions.is_empty() {
                frame.definitions.remove(name);
            }
            return Ok(());
        }
        if let Some(definitions) = state.definitions.get_mut(name) {
            definitions.pop();
            if definitions.is_empty() {
                state.definitions.remove(name);
            }
        }
        Ok(())
    }

    fn runtime(&self) -> Result<Rc<RpmMacroRuntime>> {
        self.runtime.clone().ok_or_else(|| {
            anyhow::anyhow!(
                "RpmMacroRuntimeContextMissing: builtin requires an install target context"
            )
        })
    }

    fn quote(&self, value: &str, keep_quoted: bool) -> String {
        if keep_quoted {
            format!("{QUOTE_MARKER}{value}{QUOTE_MARKER}")
        } else {
            value.to_string()
        }
    }
}

#[derive(Clone, Copy)]
enum CallStyle {
    Braced,
    Unbraced,
}

#[derive(Clone, Copy)]
enum ArgumentKind {
    Single,
    Words,
}

#[derive(Clone, Copy)]
struct CallArguments<'a> {
    text: &'a str,
    kind: ArgumentKind,
}

struct ParsedCall<'a> {
    name: &'a str,
    conditional: bool,
    negated: bool,
    arguments: Option<CallArguments<'a>>,
}

impl<'a> ParsedCall<'a> {
    fn parse(call: &'a str, style: CallStyle) -> Result<Self> {
        let bytes = call.as_bytes();
        let mut offset = 0usize;
        let mut conditional = false;
        let mut negated = false;
        while let Some(byte) = bytes.get(offset) {
            match byte {
                b'?' => {
                    conditional = true;
                    offset += 1;
                }
                b'!' => {
                    negated = !negated;
                    offset += 1;
                }
                _ => break,
            }
        }
        let name_start = offset;
        match style {
            CallStyle::Braced => {
                while bytes
                    .get(offset)
                    .is_some_and(|byte| *byte != b':' && !byte.is_ascii_whitespace())
                {
                    offset += 1;
                }
            }
            CallStyle::Unbraced => {
                if bytes.get(offset) == Some(&b'-') {
                    offset += 1;
                }
                while bytes
                    .get(offset)
                    .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
                {
                    offset += 1;
                }
                if bytes.get(offset..offset + 2) == Some(b"**") {
                    offset += 2;
                } else if matches!(bytes.get(offset), Some(b'*' | b'#')) {
                    offset += 1;
                }
            }
        }
        let name = &call[name_start..offset];
        let arguments = match style {
            CallStyle::Braced if bytes.get(offset) == Some(&b':') => Some(CallArguments {
                text: &call[offset + 1..],
                kind: ArgumentKind::Single,
            }),
            CallStyle::Braced if bytes.get(offset).is_some_and(u8::is_ascii_whitespace) => {
                Some(CallArguments {
                    text: &call[offset..],
                    kind: ArgumentKind::Words,
                })
            }
            CallStyle::Braced if offset != call.len() => None,
            CallStyle::Braced => None,
            CallStyle::Unbraced if offset < call.len() => Some(CallArguments {
                text: call[offset..].trim_end_matches(['\r', '\n']),
                kind: ArgumentKind::Words,
            }),
            CallStyle::Unbraced => None,
        };
        Ok(Self {
            name,
            conditional,
            negated,
            arguments,
        })
    }
}

struct UnbracedModifiers {
    name_start: usize,
}

fn unbraced_name(input: &str, mut offset: usize) -> Option<(usize, UnbracedModifiers)> {
    let bytes = input.as_bytes();
    while matches!(bytes.get(offset), Some(b'?' | b'!')) {
        offset += 1;
    }
    let name_start = offset;
    if bytes.get(offset) == Some(&b'-') {
        offset += 1;
    }
    while bytes
        .get(offset)
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
    {
        offset += 1;
    }
    if bytes.get(offset..offset + 2) == Some(b"**") {
        offset += 2;
    } else if matches!(bytes.get(offset), Some(b'*' | b'#')) {
        offset += 1;
    }
    (offset > name_start).then_some((offset, UnbracedModifiers { name_start }))
}

fn matching_delimiter(input: &str, start: usize, open: u8, close: u8) -> Option<usize> {
    let bytes = input.as_bytes();
    let mut level = 0usize;
    let mut offset = start;
    while offset < bytes.len() {
        match bytes[offset] {
            b'\\' => offset = (offset + 2).min(bytes.len()),
            byte if byte == open => {
                level += 1;
                offset += 1;
            }
            byte if byte == close => {
                level = level.checked_sub(1)?;
                offset += 1;
                if level == 0 {
                    return Some(offset);
                }
            }
            _ => offset += 1,
        }
    }
    None
}

fn argument_line_end(input: &str, start: usize) -> usize {
    input[start..].find('\n').map_or(input.len(), |relative| {
        let newline = start + relative;
        if newline > start && input.as_bytes()[newline - 1] == b'\\' {
            newline + 1
        } else {
            newline
        }
    })
}

fn free_field_end(input: &str, start: usize, consume_all_eols: bool) -> usize {
    let bytes = input.as_bytes();
    let mut offset = start;
    let mut braces = 0usize;
    let mut parentheses = 0usize;
    let mut brackets = 0usize;
    while offset < bytes.len() {
        if bytes[offset] == b'\\' {
            offset = (offset + 2).min(bytes.len());
            continue;
        }
        if bytes[offset] == b'%' {
            match bytes.get(offset + 1) {
                Some(b'{') => {
                    braces += 1;
                    offset += 2;
                    continue;
                }
                Some(b'(') => {
                    parentheses += 1;
                    offset += 2;
                    continue;
                }
                Some(b'[') => {
                    brackets += 1;
                    offset += 2;
                    continue;
                }
                Some(b'%') => {
                    offset += 2;
                    continue;
                }
                _ => {}
            }
        }
        match bytes[offset] {
            b'{' if braces > 0 => braces += 1,
            b'}' if braces > 0 => braces -= 1,
            b'(' if parentheses > 0 => parentheses += 1,
            b')' if parentheses > 0 => parentheses -= 1,
            b'[' if brackets > 0 => brackets += 1,
            b']' if brackets > 0 => brackets -= 1,
            b'\r' | b'\n' if braces == 0 && parentheses == 0 && brackets == 0 => {
                offset += 1;
                if bytes.get(offset - 1) == Some(&b'\r') && bytes.get(offset) == Some(&b'\n') {
                    offset += 1;
                }
                if consume_all_eols {
                    while matches!(bytes.get(offset), Some(b'\r' | b'\n')) {
                        offset += 1;
                    }
                }
                return offset;
            }
            _ => {}
        }
        offset += 1;
    }
    offset
}

fn parse_definition(specification: &str) -> Result<(String, Option<String>, String, bool, bool)> {
    let specification = specification.trim_start();
    let bytes = specification.as_bytes();
    let mut offset = 0usize;
    while bytes
        .get(offset)
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
    {
        offset += 1;
    }
    let name = &specification[..offset];
    validate_definition_name(name)?;
    let options = if bytes.get(offset) == Some(&b'(') {
        let close = specification[offset + 1..]
            .find(')')
            .map(|relative| offset + 1 + relative)
            .ok_or_else(|| {
                anyhow::anyhow!("RpmMacroDefinitionError: %{name} has unterminated options")
            })?;
        let options = specification[offset + 1..close].to_string();
        offset = close + 1;
        Some(options)
    } else {
        None
    };
    let mut literal = false;
    let mut oneshot = false;
    if bytes.get(offset) == Some(&b'<') {
        let close = specification[offset + 1..]
            .find('>')
            .map(|relative| offset + 1 + relative)
            .ok_or_else(|| {
                anyhow::anyhow!("RpmMacroDefinitionError: %{name} has unterminated modifiers")
            })?;
        let modifiers = &specification[offset + 1..close];
        for modifier in modifiers.bytes() {
            match modifier {
                b'l' => literal = true,
                b'o' => oneshot = true,
                modifier => bail!(
                    "RpmMacroDefinitionError: %{name} has illegal modifier '{}'",
                    char::from(modifier)
                ),
            }
        }
        offset = close + 1;
    }
    if options.is_some() && (literal || oneshot) {
        bail!("RpmMacroDefinitionError: %{name} has modifiers incompatible with parameters");
    }
    if literal && oneshot {
        bail!("RpmMacroDefinitionError: %{name} has incompatible modifiers");
    }
    let body =
        specification[offset..].trim_matches(|character: char| character.is_ascii_whitespace());
    let body =
        if body.starts_with('{') && matching_delimiter(body, 0, b'{', b'}') == Some(body.len()) {
            body[1..body.len() - 1].to_string()
        } else {
            body.to_string()
        };
    if body.is_empty() {
        bail!("RpmMacroDefinitionError: %{name} has an empty body");
    }
    Ok((name.to_string(), options, body, literal, oneshot))
}

fn validate_definition_name(name: &str) -> Result<()> {
    if name.is_empty()
        || !(name.as_bytes()[0].is_ascii_alphabetic() || (name.starts_with('_') && name.len() > 1))
        || !name
            .bytes()
            .all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
    {
        bail!("RpmMacroDefinitionError: illegal macro name '{name}'");
    }
    Ok(())
}

#[cfg(test)]
mod tests;
