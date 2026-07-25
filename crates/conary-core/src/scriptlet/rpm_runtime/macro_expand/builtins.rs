// conary-core/src/scriptlet/rpm_runtime/macro_expand/builtins.rs

//! Exact dispatch for the builtin table in RPM's `rpmio/macro.cc`.

use super::RpmMacroEngine;
use anyhow::{Context, Result, bail};
use mlua::{Lua, LuaOptions, StdLib, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Builtin {
    Patch,
    Source,
    Basename,
    Define,
    Dirname,
    Dnl,
    Dump,
    Echo,
    Error,
    Exists,
    Expand,
    Expr,
    Getconfdir,
    Getenv,
    Getncpus,
    Global,
    Gsub,
    Len,
    Load,
    Lower,
    Lua,
    Macrobody,
    Quote,
    Rep,
    Reverse,
    Rpmversion,
    Shrink,
    Span,
    Sub,
    Suffix,
    Trace,
    U2p,
    Shescape,
    Uncompress,
    Undefine,
    Upper,
    Url2path,
    Verbose,
    Warn,
    Xdg,
}

impl Builtin {
    pub(super) fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "P" => Self::Patch,
            "S" => Self::Source,
            "basename" => Self::Basename,
            "define" => Self::Define,
            "dirname" => Self::Dirname,
            "dnl" => Self::Dnl,
            "dump" => Self::Dump,
            "echo" => Self::Echo,
            "error" => Self::Error,
            "exists" => Self::Exists,
            "expand" => Self::Expand,
            "expr" => Self::Expr,
            "getconfdir" => Self::Getconfdir,
            "getenv" => Self::Getenv,
            "getncpus" => Self::Getncpus,
            "global" => Self::Global,
            "gsub" => Self::Gsub,
            "len" => Self::Len,
            "load" => Self::Load,
            "lower" => Self::Lower,
            "lua" => Self::Lua,
            "macrobody" => Self::Macrobody,
            "quote" => Self::Quote,
            "rep" => Self::Rep,
            "reverse" => Self::Reverse,
            "rpmversion" => Self::Rpmversion,
            "shrink" => Self::Shrink,
            "span" => Self::Span,
            "sub" => Self::Sub,
            "suffix" => Self::Suffix,
            "trace" => Self::Trace,
            "u2p" => Self::U2p,
            "shescape" => Self::Shescape,
            "uncompress" => Self::Uncompress,
            "undefine" => Self::Undefine,
            "upper" => Self::Upper,
            "url2path" => Self::Url2path,
            "verbose" => Self::Verbose,
            "warn" => Self::Warn,
            "xdg" => Self::Xdg,
            _ => return None,
        })
    }

    pub(super) fn consumes_free_field(self) -> bool {
        matches!(self, Self::Define | Self::Dnl | Self::Global)
    }

    pub(super) fn is_parametric(self) -> bool {
        !matches!(
            self,
            Self::Dump | Self::Getconfdir | Self::Rpmversion | Self::Trace
        )
    }

    pub(super) fn consumes_all_definition_eols(self) -> bool {
        matches!(self, Self::Define | Self::Global)
    }

    pub(super) fn takes_raw_argument(self) -> bool {
        matches!(
            self,
            Self::Define | Self::Dnl | Self::Global | Self::Lua | Self::Verbose
        )
    }

    pub(super) fn execute(
        self,
        engine: &RpmMacroEngine,
        arguments: &[String],
        raw: &str,
        depth: usize,
        keep_quoted: bool,
    ) -> Result<String> {
        self.validate_arity(arguments, raw)?;
        match self {
            Self::Patch | Self::Source => {
                let prefix = if self == Self::Source {
                    "%SOURCE"
                } else {
                    "%PATCH"
                };
                engine.expand_depth(&format!("{prefix}{}", arguments[0]), depth + 1, false)
            }
            Self::Basename => Ok(posix_basename(&arguments[0])),
            Self::Define => {
                engine.define_spec(raw, false, false, depth)?;
                Ok(String::new())
            }
            Self::Dirname => Ok(posix_dirname(&arguments[0])),
            Self::Dnl => Ok(String::new()),
            Self::Dump => {
                tracing::debug!(target: "conary::rpm_macro", "RPM macro table requested");
                Ok(String::new())
            }
            Self::Echo => {
                tracing::info!(target: "conary::rpm_macro", "{}", arguments[0]);
                Ok(String::new())
            }
            Self::Error => bail!("RpmMacroError: {}", arguments[0]),
            Self::Exists => Ok(if engine.runtime()?.target_exists(&arguments[0])? {
                "1"
            } else {
                "0"
            }
            .to_string()),
            Self::Expand => engine.expand_depth(&arguments[0], depth + 1, keep_quoted),
            Self::Expr => super::expression::evaluate(engine, &arguments[0], depth + 1, false),
            Self::Getconfdir => Ok("/usr/lib/rpm".to_string()),
            Self::Getenv => Ok(engine
                .runtime()?
                .environment(&arguments[0])
                .unwrap_or("")
                .to_string()),
            Self::Getncpus => {
                let argument = arguments.first().map(String::as_str).unwrap_or("total");
                if !matches!(argument, "total" | "proc" | "thread") {
                    bail!("RpmMacroArgumentError: getncpus argument '{argument}' is invalid");
                }
                let runtime = engine.runtime()?;
                let mut cpus = runtime.available_parallelism().max(1);
                if argument != "total" {
                    let size_macro = if argument == "proc" {
                        "_smp_tasksize_proc"
                    } else {
                        "_smp_tasksize_thread"
                    };
                    let task_size = if engine.is_defined(size_macro) {
                        engine
                            .expand_depth(&format!("%{{{size_macro}}}"), depth + 1, false)?
                            .trim()
                            .parse::<u64>()
                            .unwrap_or(512)
                    } else {
                        512
                    }
                    .max(1);
                    let memory_limited = (runtime.total_memory_mib() / task_size).max(1) as usize;
                    cpus = cpus.min(memory_limited);
                }
                Ok(cpus.to_string())
            }
            Self::Global => {
                engine.define_spec(raw, true, true, depth)?;
                Ok(String::new())
            }
            Self::Gsub
            | Self::Len
            | Self::Lower
            | Self::Rep
            | Self::Reverse
            | Self::Sub
            | Self::Upper => string_function(self, arguments),
            Self::Load => {
                let content = engine.runtime()?.read_target_file(&arguments[0])?;
                load_macro_file(engine, &arguments[0], &content, depth)?;
                Ok(String::new())
            }
            Self::Lua => super::super::lua::execute_macro_lua(engine, raw),
            Self::Macrobody => engine.definition(&arguments[0]).ok_or_else(|| {
                anyhow::anyhow!("RpmMacroArgumentError: no such macro '{}'", arguments[0])
            }),
            Self::Quote => Ok(arguments
                .iter()
                .map(|argument| engine.quote(argument, keep_quoted))
                .collect::<Vec<_>>()
                .join(" ")),
            Self::Rpmversion => Ok(engine.runtime()?.rpm_version().to_string()),
            Self::Shrink => Ok(arguments[0]
                .split_ascii_whitespace()
                .collect::<Vec<_>>()
                .join(" ")),
            Self::Span => Ok(arguments[0].clone()),
            Self::Suffix => Ok(suffix(&arguments[0]).to_string()),
            Self::Trace => Ok(String::new()),
            Self::U2p | Self::Url2path => url_to_path(&arguments[0]),
            Self::Shescape => Ok(arguments
                .iter()
                .map(|argument| format!("'{}'", argument.replace('\'', "'\\''")))
                .collect::<Vec<_>>()
                .join(" ")),
            Self::Uncompress => {
                let command = engine.expand_depth("%__rpmuncompress ", depth + 1, false)?;
                Ok(format!("{command}{}", arguments[0]))
            }
            Self::Undefine => {
                engine.undefine(&arguments[0])?;
                Ok(String::new())
            }
            Self::Verbose => {
                let runtime = engine.runtime()?;
                let value = raw.trim_start();
                if value.is_empty() {
                    Ok(if runtime.verbose() { "1" } else { "0" }.to_string())
                } else if runtime.verbose() {
                    engine.expand_depth(value, depth + 1, keep_quoted)
                } else {
                    Ok(String::new())
                }
            }
            Self::Warn => {
                tracing::warn!(target: "conary::rpm_macro", "{}", arguments[0]);
                Ok(String::new())
            }
            Self::Xdg => {
                let (environment, fallback) = match arguments[0].as_str() {
                    "cache" => ("XDG_CACHE_HOME", ".cache"),
                    "config" => ("XDG_CONFIG_HOME", ".config"),
                    "data" => ("XDG_DATA_HOME", ".local/share"),
                    "state" => ("XDG_STATE_HOME", ".local/state"),
                    other => bail!("RpmMacroArgumentError: xdg argument '{other}' is invalid"),
                };
                let runtime = engine.runtime()?;
                Ok(runtime
                    .environment(environment)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| {
                        format!("{}/{}", runtime.environment("HOME").unwrap_or(""), fallback)
                    }))
            }
        }
    }

    fn validate_arity(self, arguments: &[String], _raw: &str) -> Result<()> {
        let has_argument = if self.takes_raw_argument() {
            true
        } else {
            !arguments.is_empty()
        };
        let requires_argument = !matches!(
            self,
            Self::Dump
                | Self::Getconfdir
                | Self::Getncpus
                | Self::Rpmversion
                | Self::Trace
                | Self::Verbose
        );
        let forbids_argument = matches!(
            self,
            Self::Dump | Self::Getconfdir | Self::Rpmversion | Self::Trace
        );
        if requires_argument && !has_argument {
            bail!("RpmMacroArgumentError: %{self:?} expects an argument");
        }
        if forbids_argument && has_argument {
            bail!("RpmMacroArgumentError: %{self:?} does not accept an argument");
        }
        Ok(())
    }
}

fn string_function(function: Builtin, arguments: &[String]) -> Result<String> {
    let lua = Lua::new_with(StdLib::STRING | StdLib::TABLE, LuaOptions::default())
        .map_err(string_lua_error)
        .context("RpmMacroStringFunctionError: create bundled Lua string runtime")?;
    let table = lua.create_table().map_err(string_lua_error)?;
    for (index, argument) in arguments.iter().enumerate() {
        let numeric = match function {
            Builtin::Sub => index > 0,
            Builtin::Rep => index == 1,
            Builtin::Gsub => index == 3,
            _ => false,
        };
        if numeric {
            table
                .raw_set(
                    index + 1,
                    argument.parse::<i64>().with_context(|| {
                        format!("RpmMacroStringFunctionError: '{argument}' is not an integer")
                    })?,
                )
                .map_err(string_lua_error)?;
        } else {
            table
                .raw_set(index + 1, argument.as_str())
                .map_err(string_lua_error)?;
        }
    }
    lua.globals()
        .set("__conary_rpm_macro_args", table)
        .map_err(string_lua_error)?;
    let function_name = match function {
        Builtin::Gsub => "gsub",
        Builtin::Len => "len",
        Builtin::Lower => "lower",
        Builtin::Rep => "rep",
        Builtin::Reverse => "reverse",
        Builtin::Sub => "sub",
        Builtin::Upper => "upper",
        _ => unreachable!("caller restricts string builtins"),
    };
    let source = format!("return (string.{function_name}(table.unpack(__conary_rpm_macro_args)))");
    match lua
        .load(&source)
        .eval::<Value>()
        .map_err(string_lua_error)?
    {
        Value::String(value) => Ok(value.to_str().map_err(string_lua_error)?.to_string()),
        Value::Integer(value) => Ok(value.to_string()),
        Value::Number(value) => Ok(value.to_string()),
        value => bail!(
            "RpmMacroStringFunctionError: string.{function_name} returned {}",
            value.type_name()
        ),
    }
}

fn string_lua_error(error: mlua::Error) -> anyhow::Error {
    anyhow::anyhow!("RpmMacroStringFunctionError: {error}")
}

fn posix_basename(input: &str) -> String {
    if input.is_empty() {
        return ".".to_string();
    }
    let trimmed = input.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/".to_string();
    }
    trimmed.rsplit('/').next().unwrap_or(trimmed).to_string()
}

fn posix_dirname(input: &str) -> String {
    if input.is_empty() {
        return ".".to_string();
    }
    let trimmed = input.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/".to_string();
    }
    let Some(separator) = trimmed.rfind('/') else {
        return ".".to_string();
    };
    let directory = trimmed[..separator].trim_end_matches('/');
    if directory.is_empty() {
        if trimmed.starts_with('/') {
            "/".to_string()
        } else {
            ".".to_string()
        }
    } else {
        directory.to_string()
    }
}

fn suffix(input: &str) -> &str {
    input.rsplit_once('.').map_or("", |(_, suffix)| suffix)
}

fn url_to_path(input: &str) -> Result<String> {
    if let Ok(url) = url::Url::parse(input) {
        let path = url.path();
        return Ok(if path.is_empty() { "/" } else { path }.to_string());
    }
    Ok(input.to_string())
}

fn load_macro_file(
    engine: &RpmMacroEngine,
    guest_path: &str,
    content: &str,
    depth: usize,
) -> Result<()> {
    engine.define_global_literal("__file_name", guest_path);
    engine.define_global_literal("__file_lineno", "0");
    let result = (|| {
        for (line, record) in macro_file_records(content) {
            engine.undefine("__file_lineno")?;
            engine.define_global_literal("__file_lineno", &line.to_string());
            let definition = record.trim_start();
            if let Some(definition) = definition.strip_prefix('%')
                && !definition.starts_with('%')
            {
                engine.define_spec(definition, true, false, depth)?;
            }
        }
        Ok(())
    })();
    engine.undefine("__file_lineno")?;
    engine.undefine("__file_name")?;
    result
}

fn macro_file_records(content: &str) -> Vec<(usize, String)> {
    let mut records = Vec::new();
    let mut record = String::new();
    let mut record_start = 1usize;
    let mut line_number = 0usize;
    for line in content.split_inclusive('\n') {
        line_number += 1;
        let new_record = record.is_empty();
        if new_record {
            record_start = line_number;
        }
        record.push_str(line);
        let macro_record = record.trim_start().starts_with('%');
        if macro_record && macro_record_incomplete(&record) {
            continue;
        }
        let record = std::mem::take(&mut record)
            .trim_end_matches(['\r', '\n'])
            .to_string();
        records.push((line_number.max(record_start), record));
    }
    if !record.is_empty() {
        records.push((
            line_number.max(record_start),
            record.trim_end_matches(['\r', '\n']).to_string(),
        ));
    }
    records
}

fn macro_record_incomplete(record: &str) -> bool {
    let bytes = record.as_bytes();
    let mut braces = 0usize;
    let mut parentheses = 0usize;
    let mut brackets = 0usize;
    let mut offset = 0usize;
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
            _ => {}
        }
        offset += 1;
    }
    let before_eol = record.trim_end_matches(['\r', '\n']);
    before_eol.ends_with('\\') || braces > 0 || parentheses > 0 || brackets > 0
}
