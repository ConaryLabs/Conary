// conary-core/src/scriptlet/rpm_runtime/lua/macro_api.rs
//! RPM macro-context bindings for bundled Lua execution.

use super::lua_error;
use crate::repository::versioning::{VersionScheme, compare_repo_versions};
use crate::scriptlet::rpm_runtime::RpmMacroEngine;
use mlua::{Function, Lua, LuaOptions, MetaMethod, MultiValue, StdLib, Table, Value, Variadic};
use std::cell::RefCell;
use std::rc::Rc;

const LUA_MEMORY_LIMIT_BYTES: usize = 64 * 1024 * 1024;
const QUOTE_MARKER: char = '\u{001f}';

pub(super) fn execute(engine: &RpmMacroEngine, body: &str) -> anyhow::Result<String> {
    let libraries = StdLib::TABLE | StdLib::STRING | StdLib::UTF8 | StdLib::MATH;
    let lua = Lua::new_with(libraries, LuaOptions::default()).map_err(lua_error)?;
    lua.set_memory_limit(LUA_MEMORY_LIMIT_BYTES)
        .map_err(lua_error)?;

    let shared = Rc::new(RefCell::new(engine.clone()));
    install_macros_table(&lua, Rc::clone(&shared)).map_err(lua_error)?;
    if let Some((executor, environment)) = engine.lua_runtime_context() {
        let context =
            super::context::LuaRuntimeContext::new_with_environment(executor, &environment);
        super::install_rpm_table(&lua, Rc::clone(&shared), context.clone(), &[])
            .map_err(lua_error)?;
        super::standard_api::install_posix(&lua, context.clone()).map_err(lua_error)?;
        super::standard_api::install_os(&lua, context.clone()).map_err(lua_error)?;
        super::file_api::install_io(&lua, context.clone(), &[]).map_err(lua_error)?;
        super::loader_api::install(&lua, context).map_err(lua_error)?;
        super::debug_api::install(&lua).map_err(lua_error)?;
    } else {
        let rpm = lua.create_table().map_err(lua_error)?;
        install_rpm_macro_functions(&lua, &rpm, shared).map_err(lua_error)?;
        lua.globals().set("rpm", rpm).map_err(lua_error)?;
    }
    let output = install_captured_output(&lua).map_err(lua_error)?;

    let context = engine.current_call_context();
    let options = lua.create_table().map_err(lua_error)?;
    for (name, value) in context.options {
        options.set(name, value).map_err(lua_error)?;
    }
    let arguments = lua.create_table().map_err(lua_error)?;
    for (index, argument) in context.arguments.iter().enumerate() {
        arguments
            .raw_set(index + 1, argument.as_str())
            .map_err(lua_error)?;
    }

    let source = format!("local opt, arg = ...;{body}");
    let returned = lua
        .load(source)
        .set_name("rpm-macro")
        .call::<MultiValue>((options, arguments))
        .map_err(lua_error)?;
    if !returned.is_empty() {
        let print: Function = lua.globals().get("print").map_err(lua_error)?;
        print.call::<()>(returned).map_err(lua_error)?;
    }
    Ok(output.borrow().clone())
}

fn install_captured_output(lua: &Lua) -> mlua::Result<Rc<RefCell<String>>> {
    let output = Rc::new(RefCell::new(String::new()));
    let print_output = Rc::clone(&output);
    let print = lua.create_function(move |lua, values: Variadic<Value>| {
        let tostring: Function = lua.globals().get("tostring")?;
        let mut rendered = Vec::with_capacity(values.len());
        for value in values {
            rendered.push(tostring.call::<String>(value)?);
        }
        let mut output = print_output.borrow_mut();
        output.push_str(&rendered.join("\t"));
        output.push('\n');
        Ok(())
    })?;
    lua.globals().set("print", print)?;

    let io = match lua.globals().get::<Value>("io")? {
        Value::Table(table) => table,
        _ => lua.create_table()?,
    };
    let write_output = Rc::clone(&output);
    let write = lua.create_function(move |lua, values: Variadic<Value>| {
        let tostring: Function = lua.globals().get("tostring")?;
        let mut output = write_output.borrow_mut();
        for value in values {
            output.push_str(&tostring.call::<String>(value)?);
        }
        Ok(())
    })?;
    io.set("write", write)?;
    lua.globals().set("io", io)?;
    Ok(output)
}

pub(super) fn install_macros_table(
    lua: &Lua,
    engine: Rc<RefCell<RpmMacroEngine>>,
) -> mlua::Result<()> {
    let table = lua.create_table()?;
    let metatable = lua.create_table()?;

    let read_engine = Rc::clone(&engine);
    metatable.set(
        MetaMethod::Index.name(),
        lua.create_function(move |lua, (_table, name): (Table, String)| {
            if !read_engine.borrow().is_defined(&name) {
                return Ok(Value::Nil);
            }
            if read_engine.borrow().is_parametric(&name) {
                let call_engine = Rc::clone(&read_engine);
                let function_name = name;
                let function = lua.create_function(move |_, argument: Value| {
                    call_macro(&call_engine.borrow(), &function_name, argument)
                })?;
                return Ok(Value::Function(function));
            }
            let value = read_engine
                .borrow()
                .expand(&format!("%{{{name}}}"))
                .map_err(mlua::Error::external)?;
            Ok(Value::String(lua.create_string(value)?))
        })?,
    )?;

    metatable.set(
        MetaMethod::NewIndex.name(),
        lua.create_function(
            move |_, (_table, name, value): (Table, String, Value)| {
                match value {
                    Value::Nil => engine
                        .borrow()
                        .undefine(&name)
                        .map_err(mlua::Error::external)?,
                    Value::String(body) => engine
                        .borrow()
                        .define_body(name, body.to_str()?.to_string())
                        .map_err(mlua::Error::external)?,
                    other => {
                        return Err(mlua::Error::runtime(format!(
                            "RpmEmbeddedLuaApiTypeError: macros assignment requires string or nil, got {}",
                            other.type_name()
                        )));
                    }
                }
                Ok(())
            },
        )?,
    )?;
    table.set_metatable(Some(metatable))?;
    lua.globals().set("macros", table)
}

fn call_macro(engine: &RpmMacroEngine, name: &str, argument: Value) -> mlua::Result<String> {
    match argument {
        Value::String(argument) => engine
            .expand(&format!("%{{{name} {}}}", argument.to_str()?))
            .map_err(mlua::Error::external),
        Value::Table(arguments) => {
            let mut values = Vec::with_capacity(arguments.raw_len());
            for index in 1..=arguments.raw_len() {
                values.push(lua_string_value(arguments.raw_get(index)?, index)?);
            }
            engine
                .expand_macro_args(name, &values)
                .map_err(mlua::Error::external)
        }
        other => Err(mlua::Error::runtime(format!(
            "RpmEmbeddedLuaApiTypeError: macros.{name} expects string or table, got {}",
            other.type_name()
        ))),
    }
}

fn lua_string_value(value: Value, index: usize) -> mlua::Result<String> {
    match value {
        Value::String(value) => Ok(value.to_str()?.to_string()),
        Value::Integer(value) => Ok(value.to_string()),
        Value::Number(value) => Ok(value.to_string()),
        value => Err(mlua::Error::runtime(format!(
            "bad argument #{index} (cannot convert {} to string)",
            value.type_name()
        ))),
    }
}

pub(super) fn install_rpm_macro_functions(
    lua: &Lua,
    table: &Table,
    engine: Rc<RefCell<RpmMacroEngine>>,
) -> mlua::Result<()> {
    let expand_engine = Rc::clone(&engine);
    table.set(
        "expand",
        lua.create_function(move |_, input: String| {
            expand_engine
                .borrow()
                .expand(&input)
                .map_err(mlua::Error::external)
        })?,
    )?;
    let define_engine = Rc::clone(&engine);
    table.set(
        "define",
        lua.create_function(move |_, definition: String| {
            define_engine
                .borrow()
                .define_programmatic(&definition)
                .map_err(mlua::Error::external)
        })?,
    )?;
    let undefine_engine = Rc::clone(&engine);
    table.set(
        "undefine",
        lua.create_function(move |_, name: String| {
            undefine_engine
                .borrow()
                .undefine(&name)
                .map_err(mlua::Error::external)
        })?,
    )?;
    let defined_engine = engine;
    table.set(
        "isdefined",
        lua.create_function(move |_, name: String| {
            let engine = defined_engine.borrow();
            Ok((engine.is_defined(&name), engine.is_parametric(&name)))
        })?,
    )?;
    table.set(
        "vercmp",
        lua.create_function(|_, (left, right): (String, String)| {
            let ordering = compare_repo_versions(VersionScheme::Rpm, &left, &right)
                .map_err(mlua::Error::external)?;
            Ok(match ordering {
                std::cmp::Ordering::Less => -1,
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
            })
        })?,
    )?;
    table.set(
        "splitargs",
        lua.create_function(|lua, input: String| {
            let table = lua.create_table()?;
            for (index, argument) in RpmMacroEngine::split_arguments(&input)
                .map_err(mlua::Error::external)?
                .into_iter()
                .enumerate()
            {
                table.raw_set(index + 1, argument)?;
            }
            Ok(table)
        })?,
    )?;
    table.set(
        "unsplitargs",
        lua.create_function(|_, arguments: Table| {
            let mut values = Vec::with_capacity(arguments.raw_len());
            for index in 1..=arguments.raw_len() {
                let value = lua_string_value(arguments.raw_get(index)?, index)?;
                values.push(format!("{QUOTE_MARKER}{value}{QUOTE_MARKER}"));
            }
            Ok(values.join(" "))
        })?,
    )
}
