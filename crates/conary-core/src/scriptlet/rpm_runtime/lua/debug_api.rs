// conary-core/src/scriptlet/rpm_runtime/lua/debug_api.rs
//! Safe debug-library projection that preserves Conary's VM deadline hook.

use mlua::{Function, Lua, MultiValue, Table, Value};

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let table = lua.create_table()?;
    table.set(
        "traceback",
        lua.create_function(|lua, (message, level): (Option<Value>, Option<usize>)| {
            let mut output = match message {
                Some(Value::String(message)) => message.to_str()?.to_string(),
                Some(value) if !matches!(value, Value::Nil) => {
                    return Ok(value);
                }
                _ => String::new(),
            };
            output.push_str("\nstack traceback:");
            let mut depth = level.unwrap_or(1);
            while let Some(line) = lua.inspect_stack(depth, |debug| {
                let source = debug.source();
                let name = debug.names().name.unwrap_or_default();
                format!(
                    "\n\t{}:{}: in {}",
                    source.short_src.unwrap_or_default(),
                    debug.current_line().unwrap_or(0),
                    if name.is_empty() { "?" } else { name.as_ref() }
                )
            }) {
                output.push_str(&line);
                depth += 1;
            }
            Ok(Value::String(lua.create_string(output)?))
        })?,
    )?;
    table.set(
        "getinfo",
        lua.create_function(
            |lua, (target, _what): (Value, Option<String>)| match target {
                Value::Function(function) => Ok(function_info(lua, &function)),
                Value::Integer(level) if level >= 0 => Ok(lua
                    .inspect_stack(level as usize, |debug| {
                        let table = lua.create_table()?;
                        let source = debug.source();
                        let names = debug.names();
                        table.set("name", names.name.as_deref())?;
                        table.set("namewhat", names.name_what.unwrap_or(""))?;
                        table.set("what", source.what)?;
                        table.set("source", source.source.as_deref())?;
                        table.set("short_src", source.short_src.as_deref())?;
                        table.set("linedefined", source.line_defined.unwrap_or(0))?;
                        table.set("lastlinedefined", source.last_line_defined.unwrap_or(0))?;
                        table.set("currentline", debug.current_line().unwrap_or(0))?;
                        table.set("func", debug.function())?;
                        Ok::<_, mlua::Error>(table)
                    })
                    .transpose()?),
                value => Err(mlua::Error::runtime(format!(
                    "debug.getinfo expects function or stack level, got {}",
                    value.type_name()
                ))),
            },
        )?,
    )?;
    table.set(
        "getmetatable",
        lua.create_function(|_, value: Value| match value {
            Value::Table(table) => Ok(table.metatable().map(Value::Table).unwrap_or(Value::Nil)),
            Value::UserData(_) => Ok(Value::Nil),
            _ => Ok(Value::Nil),
        })?,
    )?;
    table.set(
        "setmetatable",
        lua.create_function(
            |_, (value, metatable): (Value, Option<Table>)| match value {
                Value::Table(table) => {
                    table.set_metatable(metatable)?;
                    Ok(Value::Table(table))
                }
                value => Err(mlua::Error::runtime(format!(
                    "debug.setmetatable cannot change {}",
                    value.type_name()
                ))),
            },
        )?,
    )?;
    table.set(
        "getupvalue",
        lua.create_function(|lua, (function, index): (Function, usize)| {
            if index == 1 {
                Ok(MultiValue::from_vec(vec![
                    Value::String(lua.create_string("_ENV")?),
                    function
                        .environment()
                        .map(Value::Table)
                        .unwrap_or(Value::Nil),
                ]))
            } else {
                Ok(MultiValue::new())
            }
        })?,
    )?;
    table.set(
        "setupvalue",
        lua.create_function(|_, (function, index, value): (Function, usize, Value)| {
            if index == 1
                && let Value::Table(environment) = value
            {
                function.set_environment(environment)?;
                return Ok(Some("_ENV"));
            }
            Ok(None)
        })?,
    )?;
    table.set(
        "upvaluejoin",
        lua.create_function(
            |_, (left, left_index, right, right_index): (Function, usize, Function, usize)| {
                if left_index == 1
                    && right_index == 1
                    && let Some(environment) = right.environment()
                {
                    left.set_environment(environment)?;
                    return Ok(());
                }
                Err(mlua::Error::runtime("upvalue index has no managed value"))
            },
        )?,
    )?;
    table.set(
        "upvalueid",
        lua.create_function(|_, (function, index): (Function, usize)| {
            if index == 1 {
                Ok(Value::Table(function.environment().ok_or_else(|| {
                    mlua::Error::runtime("function has no _ENV upvalue")
                })?))
            } else {
                Ok(Value::Nil)
            }
        })?,
    )?;
    for name in ["getlocal", "setlocal", "getuservalue", "setuservalue"] {
        table.set(
            name,
            lua.create_function(|_, _: MultiValue| Ok(Value::Nil))?,
        )?;
    }
    table.set(
        "getregistry",
        lua.create_function(|lua, ()| lua.create_table())?,
    )?;
    table.set(
        "gethook",
        lua.create_function(|_, ()| Ok((Value::Nil, "", 10_000)))?,
    )?;
    table.set(
        "sethook",
        lua.create_function(|_, _: MultiValue| {
            Err::<Value, _>(mlua::Error::runtime(
                "debug.sethook cannot replace the RPM Lua execution deadline",
            ))
        })?,
    )?;
    table.set(
        "debug",
        lua.create_function(|_, ()| {
            Err::<Value, _>(mlua::Error::runtime("interactive debug requires a tty"))
        })?,
    )?;
    lua.globals().set("debug", table)
}

fn function_info(lua: &Lua, function: &Function) -> Option<Table> {
    let info = function.info();
    let table = lua.create_table().ok()?;
    table.set("name", info.name).ok()?;
    table.set("namewhat", info.name_what.unwrap_or("")).ok()?;
    table.set("what", info.what).ok()?;
    table.set("source", info.source).ok()?;
    table.set("short_src", info.short_src).ok()?;
    table
        .set("linedefined", info.line_defined.unwrap_or(0))
        .ok()?;
    table
        .set("lastlinedefined", info.last_line_defined.unwrap_or(0))
        .ok()?;
    table.set("func", function.clone()).ok()?;
    Some(table)
}
