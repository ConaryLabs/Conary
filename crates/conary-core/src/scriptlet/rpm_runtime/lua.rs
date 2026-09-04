// conary-core/src/scriptlet/rpm_runtime/lua.rs

//! Bundled Lua 5.4 runtime for RPM `<lua>` transaction scriptlets.

mod context;
mod debug_api;
mod file_api;
mod loader_api;
mod macro_api;
mod process_api;
mod rpm_api;
mod standard_api;

use super::RpmMacroEngine;
use crate::ccs::native_lifecycle::RpmRuntimeMetadata;
use crate::error::{Error, Result};
use crate::scriptlet::ScriptletExecutor;
use base64::Engine as _;
use mlua::{HookTriggers, Lua, LuaOptions, StdLib, Value, Variadic, VmState};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::{Duration, Instant};

const LUA_MEMORY_LIMIT_BYTES: usize = 64 * 1024 * 1024;
const LUA_HOOK_INSTRUCTIONS: u32 = 10_000;

pub(super) fn execute_macro_lua(engine: &RpmMacroEngine, body: &str) -> anyhow::Result<String> {
    macro_api::execute(engine, body)
}

pub(crate) fn execute_embedded_lua(
    executor: &ScriptletExecutor,
    phase: &str,
    body: &str,
    args: &[String],
    stdin: &[u8],
    runtime: &RpmRuntimeMetadata,
    timeout: Duration,
) -> Result<()> {
    let libraries =
        StdLib::COROUTINE | StdLib::TABLE | StdLib::STRING | StdLib::UTF8 | StdLib::MATH;
    let lua = Lua::new_with(libraries, LuaOptions::default()).map_err(lua_error)?;
    lua.set_memory_limit(LUA_MEMORY_LIMIT_BYTES)
        .map_err(lua_error)?;
    let deadline = Instant::now() + timeout;
    lua.set_hook(
        HookTriggers::new().every_nth_instruction(LUA_HOOK_INSTRUCTIONS),
        move |_, _| {
            if Instant::now() >= deadline {
                Err(mlua::Error::runtime(
                    "RpmEmbeddedLuaTimeout: bundled Lua instruction deadline expired",
                ))
            } else {
                Ok(VmState::Continue)
            }
        },
    )
    .map_err(lua_error)?;

    install_arg_table(&lua, args).map_err(lua_error)?;
    install_prefix_table(&lua, &runtime.install_prefixes).map_err(lua_error)?;
    install_print(&lua).map_err(lua_error)?;

    let context = context::LuaRuntimeContext::new(executor.clone());
    let macro_engine = Rc::new(RefCell::new(RpmMacroEngine::from_runtime(
        executor,
        &[],
        runtime,
    )));
    macro_api::install_macros_table(&lua, Rc::clone(&macro_engine)).map_err(lua_error)?;
    install_rpm_table(&lua, macro_engine, context.clone(), stdin).map_err(lua_error)?;
    standard_api::install_posix(&lua, context.clone()).map_err(lua_error)?;
    standard_api::install_os(&lua, context.clone()).map_err(lua_error)?;
    file_api::install_io(&lua, context.clone(), stdin).map_err(lua_error)?;
    loader_api::install(&lua, context).map_err(lua_error)?;
    debug_api::install(&lua).map_err(lua_error)?;

    lua.load(body)
        .set_name(format!("rpm:{phase}"))
        .exec()
        .map_err(lua_error)
}

fn install_arg_table(lua: &Lua, args: &[String]) -> mlua::Result<()> {
    let table = lua.create_table()?;
    table.raw_set(1, "<lua>")?;
    for (index, argument) in args.iter().enumerate() {
        if let Ok(number) = argument.parse::<i64>() {
            table.raw_set(index + 2, number)?;
        } else {
            table.raw_set(index + 2, argument.as_str())?;
        }
    }
    lua.globals().set("arg", table)
}

fn install_prefix_table(lua: &Lua, prefixes: &[String]) -> mlua::Result<()> {
    if prefixes.is_empty() {
        return Ok(());
    }
    let table = lua.create_table()?;
    for (index, prefix) in prefixes.iter().enumerate() {
        table.raw_set(index + 1, prefix.as_str())?;
    }
    lua.globals().set("RPM_INSTALL_PREFIX", table)
}

fn install_print(lua: &Lua) -> mlua::Result<()> {
    let print = lua.create_function(|lua, values: Variadic<Value>| {
        let tostring: mlua::Function = lua.globals().get("tostring")?;
        let mut rendered = Vec::with_capacity(values.len());
        for value in values {
            rendered.push(tostring.call::<String>(value)?);
        }
        tracing::info!(target: "conary::rpm_lua", "{}", rendered.join("\t"));
        Ok(())
    })?;
    lua.globals().set("print", print)
}

fn install_rpm_table(
    lua: &Lua,
    engine: Rc<RefCell<RpmMacroEngine>>,
    context: context::LuaRuntimeContext,
    stdin: &[u8],
) -> mlua::Result<()> {
    let table = lua.create_table()?;

    table.set(
        "b64encode",
        lua.create_function(|_, (input, line_len): (mlua::String, Option<usize>)| {
            let encoded = base64::engine::general_purpose::STANDARD.encode(input.as_bytes());
            Ok(match line_len {
                Some(line_len) if line_len > 0 => encoded
                    .as_bytes()
                    .chunks(line_len)
                    .map(|chunk| String::from_utf8_lossy(chunk))
                    .collect::<Vec<_>>()
                    .join("\n"),
                _ => encoded,
            })
        })?,
    )?;
    table.set(
        "b64decode",
        lua.create_function(|lua, input: String| {
            Ok(
                match base64::engine::general_purpose::STANDARD.decode(input) {
                    Ok(value) => Value::String(lua.create_string(&value)?),
                    Err(_) => Value::Nil,
                },
            )
        })?,
    )?;
    macro_api::install_rpm_macro_functions(lua, &table, Rc::clone(&engine))?;

    let lines = Rc::new(RefCell::new(
        String::from_utf8_lossy(stdin)
            .lines()
            .map(str::to_string)
            .collect::<VecDeque<_>>(),
    ));
    for function_name in ["next_line", "next_file"] {
        let lines = Rc::clone(&lines);
        table.set(
            function_name,
            lua.create_function(move |_, ()| Ok(lines.borrow_mut().pop_front()))?,
        )?;
    }

    process_api::install_rpm(lua, &table, context.clone())?;
    file_api::install_rpm_open(lua, &table, context.clone())?;
    rpm_api::install(lua, &table, engine, context)?;
    lua.globals().set("rpm", table)
}

fn lua_error(error: mlua::Error) -> Error {
    Error::scriptlet(
        crate::error::ScriptletFailureKind::ScriptExited,
        format!("RpmEmbeddedLuaExecutionFailed: {error}"),
    )
}

#[cfg(test)]
mod tests;
