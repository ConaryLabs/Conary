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
mod tests {
    use super::execute_embedded_lua;
    use crate::ccs::native_lifecycle::{
        RpmCriticality, RpmHeaderContext, RpmMacroContext, RpmMacroDefinition,
        RpmMacroDefinitionSource, RpmProgram, RpmRuntimeMetadata,
    };
    use crate::scriptlet::{PackageFormat, SandboxMode, ScriptletExecutor};
    use std::path::Path;
    use std::time::Duration;

    fn runtime() -> RpmRuntimeMetadata {
        RpmRuntimeMetadata {
            program: RpmProgram::EmbeddedLua,
            body_transforms: Vec::new(),
            critical: false,
            criticality: RpmCriticality::WarningOnly,
            raw_flags: 0,
            unknown_flags: 0,
            install_prefixes: vec!["/opt/demo".to_string()],
            macro_context: RpmMacroContext {
                definitions: vec![RpmMacroDefinition {
                    name: "name".to_string(),
                    body: "demo".to_string(),
                    source: RpmMacroDefinitionSource::PackageHeader,
                }],
            },
            header_context: RpmHeaderContext::default(),
            package_rpm_version: Some("6.0.0".to_string()),
        }
    }

    #[test]
    fn embedded_lua_exposes_rpm_arguments_prefixes_macros_and_input() {
        let executor = ScriptletExecutor::new(Path::new("/"), "demo", "1.0", PackageFormat::Rpm)
            .with_sandbox_mode(SandboxMode::Always);
        execute_embedded_lua(
            &executor,
            "post-install",
            r#"
                assert(arg[1] == "<lua>")
                assert(arg[2] == 2)
                assert(RPM_INSTALL_PREFIX[1] == "/opt/demo")
                assert(rpm.expand("%{name}") == "demo")
                assert(macros.name == "demo")
                assert(rpm.vercmp("1.0", "2.0") == -1)
                assert(rpm.next_line() == "/usr/lib/demo")
                assert(rpm.next_line() == nil)
                assert(rpm.b64decode(rpm.b64encode("payload")) == "payload")
                assert(rpm.execute("/bin/true") == 0)
                assert(rpm.spawn({"/bin/true"}) == 0)
                local ok, message, code = rpm.execute("/bin/sh", "-c", "exit 7")
                assert(ok == nil)
                assert(message == "exit code: 7")
                assert(code == 7)
            "#,
            &["2".to_string()],
            b"/usr/lib/demo\n",
            &runtime(),
            Duration::from_secs(5),
        )
        .expect("embedded Lua");
    }

    #[test]
    fn embedded_lua_uses_target_root_for_files_modules_and_posix_state() {
        let root = tempfile::tempdir().expect("target root");
        std::fs::create_dir_all(root.path().join("usr/lib/rpm/lua")).expect("module directory");
        std::fs::create_dir_all(root.path().join("tmp")).expect("tmp directory");
        std::fs::write(
            root.path().join("usr/lib/rpm/lua/demo.lua"),
            "return { value = 42 }",
        )
        .expect("module");
        std::fs::write(root.path().join("macros.test"), "%loaded target\n").expect("macro file");
        let executor = ScriptletExecutor::new(root.path(), "demo", "1.0", PackageFormat::Rpm);

        execute_embedded_lua(
            &executor,
            "post-install",
            r#"
                assert(posix.mkdir("/var", 493) == 0)
                local out = assert(io.open("/var/value", "w"))
                out:write("payload")
                out:close()
                local input = assert(rpm.open("/var/value"))
                assert(input:read() == "payload")
                input:close()
                assert(posix.stat("/var/value", "size") == 7)
                assert(rpm.glob("/var/*")[1] == "/var/value")
                assert(posix.chdir("/var") == 0)
                assert(posix.getcwd() == "/var")
                assert(posix.putenv("RPM_LUA_TARGET=yes") == 0)
                assert(os.getenv("RPM_LUA_TARGET") == "yes")
                assert(require("demo").value == 42)
                assert(package.searchpath("demo", package.path) == "/usr/lib/rpm/lua/demo.lua")
                assert(type(debug.getinfo(function() end)) == "table")
                rpm.load("/macros.test")
                assert(rpm.expand("%{loaded}") == "target")
                rpm.define("pair() %1:%2")
                assert(macros.pair({"one", "two"}) == "one:two")
                local v = rpm.ver("1:4.16-5")
                assert(tostring(v) == "1:4.16-5" and v.e == "1" and v.v == "4.16" and v.r == "5")
                local called = false
                local token = rpm.register("demo-hook", function(values)
                    called = values[1] == "ok"
                end)
                rpm.call("demo-hook", "ok")
                rpm.unregister("demo-hook", token)
                assert(called)
            "#,
            &[],
            &[],
            &runtime(),
            Duration::from_secs(5),
        )
        .expect("target-root embedded Lua");
        assert_eq!(
            std::fs::read(root.path().join("var/value")).expect("written value"),
            b"payload"
        );
    }

    #[test]
    fn embedded_lua_spawn_redirections_are_target_confined() {
        let output = tempfile::NamedTempFile::new().expect("stdout");
        let error = tempfile::NamedTempFile::new().expect("stderr");
        let output_path = output.path().to_string_lossy();
        let error_path = error.path().to_string_lossy();
        let body = format!(
            r#"
                assert(rpm.spawn(
                    {{"/bin/sh", "-c", "printf stdout; printf stderr >&2"}},
                    {{stdout={output_path:?}, stderr={error_path:?}}}
                ) == 0)
            "#
        );
        let executor = ScriptletExecutor::new(Path::new("/"), "demo", "1.0", PackageFormat::Rpm)
            .with_sandbox_mode(SandboxMode::Always);

        execute_embedded_lua(
            &executor,
            "post-install",
            &body,
            &[],
            &[],
            &runtime(),
            Duration::from_secs(5),
        )
        .expect("spawn redirection");
        assert_eq!(
            std::fs::read_to_string(output.path()).expect("stdout"),
            "stdout"
        );
        assert_eq!(
            std::fs::read_to_string(error.path()).expect("stderr"),
            "stderr"
        );
    }
}
