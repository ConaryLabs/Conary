// conary-core/src/scriptlet/rpm_runtime/lua/process_api.rs
//! Target-sandboxed process APIs for RPM's bundled Lua runtime.

use super::context::LuaRuntimeContext;
use crate::scriptlet::native_command::{NativeCommandOutput, NativeCommandStatus};
use mlua::{Lua, MultiValue, Table, Value, Variadic};
use std::collections::BTreeMap;

pub(super) fn install_rpm(
    lua: &Lua,
    table: &Table,
    context: LuaRuntimeContext,
) -> mlua::Result<()> {
    let execute_context = context.clone();
    table.set(
        "execute",
        lua.create_function(move |lua, argv: Variadic<String>| {
            if argv.is_empty() {
                return Err(mlua::Error::runtime(
                    "RpmEmbeddedLuaApiTypeError: rpm.execute requires a program",
                ));
            }
            let output = execute(&execute_context, &argv, &[])?;
            log_output("rpm-embedded-lua", &output);
            command_result(lua, output.status)
        })?,
    )?;

    table.set(
        "spawn",
        lua.create_function(move |lua, (argv, actions): (Table, Option<Table>)| {
            let argv = sequence_strings(&argv)?;
            if argv.is_empty() {
                return Err(mlua::Error::runtime(
                    "RpmEmbeddedLuaApiTypeError: rpm.spawn requires a command table",
                ));
            }
            let actions = parse_spawn_actions(actions)?;
            let stdin = match actions.get("stdin") {
                Some(path) => context.read(path).map_err(mlua::Error::external)?,
                None => Vec::new(),
            };
            let output = execute(&context, &argv, &stdin)?;
            if let Some(path) = actions.get("stdout") {
                context
                    .write(path, &output.stdout, true)
                    .map_err(mlua::Error::external)?;
            }
            if let Some(path) = actions.get("stderr") {
                context
                    .write(path, &output.stderr, true)
                    .map_err(mlua::Error::external)?;
            }
            if !actions.contains_key("stdout") || !actions.contains_key("stderr") {
                log_output("rpm-embedded-lua", &output);
            }
            command_result(lua, output.status)
        })?,
    )?;
    table.set(
        "redirect2null",
        lua.create_function(|_, _: MultiValue| {
            Err::<Value, _>(mlua::Error::runtime(
                "rpm.redirect2null() is no longer available, use rpm.spawn() or rpm.execute() instead",
            ))
        })?,
    )
}

pub(super) fn install_os_execute(
    lua: &Lua,
    table: &Table,
    context: LuaRuntimeContext,
) -> mlua::Result<()> {
    table.set(
        "execute",
        lua.create_function(move |lua, command: Option<String>| {
            let Some(command) = command else {
                let available = context
                    .executor()
                    .execute_native_command_capture(
                        "rpm-embedded-lua-os-shell-check",
                        &[
                            "/bin/sh".to_string(),
                            "-c".to_string(),
                            "exit 0".to_string(),
                        ],
                        &[],
                        &[],
                    )
                    .is_ok();
                return Ok(MultiValue::from_vec(vec![Value::Boolean(available)]));
            };
            let output = execute(
                &context,
                &["/bin/sh".to_string(), "-c".to_string(), command],
                &[],
            )?;
            log_output("rpm-embedded-lua-os", &output);
            os_command_result(lua, output.status)
        })?,
    )
}

fn execute(
    context: &LuaRuntimeContext,
    argv: &[String],
    stdin: &[u8],
) -> mlua::Result<NativeCommandOutput> {
    let environment = context.command_environment();
    let environment = environment
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect::<Vec<_>>();
    context
        .executor()
        .execute_native_command_capture("rpm-embedded-lua", argv, stdin, &environment)
        .map_err(mlua::Error::external)
}

fn sequence_strings(table: &Table) -> mlua::Result<Vec<String>> {
    let mut values = Vec::with_capacity(table.raw_len());
    for index in 1..=table.raw_len() {
        values.push(match table.raw_get::<Value>(index)? {
            Value::String(value) => value.to_str()?.to_string(),
            Value::Integer(value) => value.to_string(),
            Value::Number(value) => value.to_string(),
            value => {
                return Err(mlua::Error::runtime(format!(
                    "bad argument #{index} (cannot convert {} to string)",
                    value.type_name()
                )));
            }
        });
    }
    Ok(values)
}

fn parse_spawn_actions(actions: Option<Table>) -> mlua::Result<BTreeMap<String, String>> {
    let mut parsed = BTreeMap::new();
    let Some(actions) = actions else {
        return Ok(parsed);
    };
    for pair in actions.pairs::<String, String>() {
        let (name, path) = pair?;
        if !matches!(name.as_str(), "stdin" | "stdout" | "stderr") {
            return Err(mlua::Error::runtime(format!(
                "invalid spawn directive: {name}"
            )));
        }
        parsed.insert(name, path);
    }
    Ok(parsed)
}

fn log_output(phase: &str, output: &NativeCommandOutput) {
    crate::scriptlet::runtime::log_script_output(
        phase,
        &String::from_utf8_lossy(&output.stdout),
        &String::from_utf8_lossy(&output.stderr),
    );
}

fn command_result(lua: &Lua, status: NativeCommandStatus) -> mlua::Result<MultiValue> {
    Ok(match status {
        NativeCommandStatus::Success => MultiValue::from_vec(vec![Value::Integer(0)]),
        NativeCommandStatus::ExitCode(code) => MultiValue::from_vec(vec![
            Value::Nil,
            Value::String(lua.create_string(format!("exit code: {code}"))?),
            Value::Integer(i64::from(code)),
        ]),
        NativeCommandStatus::Signal(signal) => MultiValue::from_vec(vec![
            Value::Nil,
            Value::String(lua.create_string(format!("exit signal: {signal}"))?),
            Value::Integer(i64::from(signal)),
        ]),
    })
}

fn os_command_result(lua: &Lua, status: NativeCommandStatus) -> mlua::Result<MultiValue> {
    Ok(match status {
        NativeCommandStatus::Success => MultiValue::from_vec(vec![
            Value::Boolean(true),
            Value::String(lua.create_string("exit")?),
            Value::Integer(0),
        ]),
        NativeCommandStatus::ExitCode(code) => MultiValue::from_vec(vec![
            Value::Nil,
            Value::String(lua.create_string("exit")?),
            Value::Integer(i64::from(code)),
        ]),
        NativeCommandStatus::Signal(signal) => MultiValue::from_vec(vec![
            Value::Nil,
            Value::String(lua.create_string("signal")?),
            Value::Integer(i64::from(signal)),
        ]),
    })
}
