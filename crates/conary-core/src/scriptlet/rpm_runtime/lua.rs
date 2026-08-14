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
    use crate::scriptlet::test_support::materialized_root;
    use crate::scriptlet::{PackageFormat, SandboxMode, ScriptletExecutor};
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    fn runtime() -> RpmRuntimeMetadata {
        RpmRuntimeMetadata {
            program: RpmProgram::EmbeddedLua,
            body_transforms: Vec::new(),
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
        let Some(root) = materialized_root(
            "scriptlet::rpm_runtime::lua::tests::embedded_lua_exposes_rpm_arguments_prefixes_macros_and_input",
            &["/bin/true", "/bin/sh"],
        ) else {
            return;
        };
        let executor = ScriptletExecutor::new(root.path(), "demo", "1.0", PackageFormat::Rpm)
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
        std::os::unix::fs::symlink("loop", root.path().join("loop")).expect("symlink loop");
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
                assert(select('#', posix.stat("/var/value", "size")) == 1)
                local missing, message, errno = posix.stat("/media")
                assert(missing == nil)
                assert(message == "/media: No such file or directory")
                assert(errno == 2)
                assert(posix.stat("/loop", "type") == "link")
                local confined, confinement_error = pcall(posix.stat, "/loop/child")
                assert(confined == false)
                assert(string.find(tostring(confinement_error),
                    "RpmTargetPathError: symlink depth exceeds 64", 1, true))
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
    fn embedded_lua_posix_directories_preserve_the_pinned_open_contract() {
        let root = tempfile::tempdir().expect("target root");
        std::fs::create_dir_all(root.path().join("present")).expect("present directory");
        std::fs::write(root.path().join("present/entry"), b"value").expect("directory entry");
        std::os::unix::fs::symlink("loop", root.path().join("loop")).expect("symlink loop");
        let executor =
            ScriptletExecutor::new(root.path(), "glibc", "2.43-2.fc44", PackageFormat::Rpm);

        execute_embedded_lua(
            &executor,
            "pre-install",
            r#"
                local entries = assert(posix.dir("/present"))
                local names = {}
                for _, entry in ipairs(entries) do names[entry] = true end
                assert(names["."] and names[".."] and names.entry)

                local iter = assert(posix.files("/present"))
                names = {}
                for entry in iter do names[entry] = true end
                assert(names["."] and names[".."] and names.entry)

                local missing, message, errno =
                    posix.files("/usr/lib64/glibc-hwcaps")
                assert(missing == nil)
                assert(message ==
                    "/usr/lib64/glibc-hwcaps: No such file or directory")
                assert(errno == 2)

                local missing_dir, dir_message, dir_errno =
                    posix.dir("/usr/lib64/glibc-hwcaps")
                assert(missing_dir == nil)
                assert(dir_message ==
                    "/usr/lib64/glibc-hwcaps: No such file or directory")
                assert(dir_errno == 2)

                -- This is the exact optional-directory control-flow shape in
                -- Fedora glibc's embedded-Lua pre-install body.
                repeat
                    local optional = posix.files("/usr/lib64/glibc-hwcaps")
                    if optional ~= nil then
                        for _ in optional do error("missing path was iterated") end
                    end
                until true

                local confined, confinement_error =
                    pcall(posix.files, "/loop/child")
                assert(confined == false)
                assert(string.find(tostring(confinement_error),
                    "RpmTargetPathError: symlink depth exceeds 64", 1, true))
            "#,
            &["1".to_string()],
            &[],
            &runtime(),
            Duration::from_secs(5),
        )
        .expect("pinned posix directory contract");
    }

    #[test]
    fn embedded_lua_file_read_uses_the_pinned_selector_grammar() {
        let root = tempfile::tempdir().expect("target root");
        std::fs::write(root.path().join("values"), b"42\nsecond\n").expect("fixture values");
        let executor = ScriptletExecutor::new(root.path(), "demo", "1.0", PackageFormat::Rpm);

        execute_embedded_lua(
            &executor,
            "post-install",
            r#"
                local function read(format)
                    local file = assert(io.open("/values", "r"))
                    local value = file:read(format)
                    file:close()
                    return value
                end

                assert(read("*number") == 42)
                assert(read("*line") == "42")
                assert(read("*Line") == "42\n")
                assert(read("*all") == "42\nsecond\n")
                assert(read("all-compatible-suffix") == "42\nsecond\n")

                local ok, message = pcall(read, "*unknown")
                assert(ok == false)
                assert(string.find(tostring(message),
                    "invalid file read format '*unknown'", 1, true))
            "#,
            &[],
            &[],
            &runtime(),
            Duration::from_secs(5),
        )
        .expect("pinned file:read selector grammar");
    }

    #[test]
    fn embedded_lua_io_open_preserves_standard_and_rpm_failure_contracts() {
        let root = tempfile::tempdir().expect("target root");
        std::os::unix::fs::symlink("loop", root.path().join("loop")).expect("symlink loop");
        let executor = ScriptletExecutor::new(
            root.path(),
            "crypto-policies",
            "20251128",
            PackageFormat::Rpm,
        );

        execute_embedded_lua(
            &executor,
            "post-install",
            r#"
                local missing, message, errno =
                    io.open("/proc/sys/crypto/fips_enabled", "r")
                assert(missing == nil)
                assert(message ==
                    "/proc/sys/crypto/fips_enabled: No such file or directory")
                assert(errno == 2)

                local valid_mode, mode_error = pcall(io.open, "/missing", "rx")
                assert(valid_mode == false)
                assert(string.find(tostring(mode_error), "invalid mode", 1, true))

                local rpm_ok, rpm_error = pcall(rpm.open, "/missing", "r")
                assert(rpm_ok == false)
                assert(string.find(tostring(rpm_error),
                    "No such file or directory", 1, true))

                local confined, confinement_error =
                    pcall(io.open, "/loop/child", "r")
                assert(confined == false)
                assert(string.find(tostring(confinement_error),
                    "RpmTargetPathError: symlink depth exceeds 64", 1, true))
            "#,
            &["1".to_string()],
            &[],
            &runtime(),
            Duration::from_secs(5),
        )
        .expect("standard and RPM open contracts");
    }

    #[test]
    fn embedded_lua_posix_filesystem_calls_preserve_pinned_result_tuples() {
        let root = tempfile::tempdir().expect("target root");
        std::fs::create_dir_all(root.path().join("present")).expect("present directory");
        std::fs::create_dir_all(root.path().join("empty")).expect("empty directory");
        std::fs::write(root.path().join("present/entry"), b"value").expect("present entry");
        std::os::unix::fs::symlink("loop", root.path().join("loop")).expect("symlink loop");
        let executor = ScriptletExecutor::new(root.path(), "demo", "1.0", PackageFormat::Rpm);

        execute_embedded_lua(
            &executor,
            "post-install",
            r#"
                local function path_error(call, path, description, errno)
                    local result, message, code = call()
                    assert(result == nil)
                    assert(message == path .. ": " .. description)
                    assert(code == errno)
                end

                local function plain_error(call, description, errno)
                    local result, message, code = call()
                    assert(result == nil)
                    assert(message == description)
                    assert(code == errno)
                end

                assert(posix.access("/present", "r x") == 0)
                path_error(function() return posix.access("/missing") end,
                    "/missing", "No such file or directory", 2)
                path_error(function() return posix.chdir("/missing") end,
                    "/missing", "No such file or directory", 2)
                path_error(function() return posix.chdir("/present/entry") end,
                    "/present/entry", "Not a directory", 20)
                assert(posix.getcwd() == "/")
                path_error(function() return posix.chmod("/missing", "u+x") end,
                    "/missing", "No such file or directory", 2)
                path_error(function() return posix.chown("/missing", nil, nil) end,
                    "/missing", "No such file or directory", 2)
                path_error(function() return posix.mkdir("/present") end,
                    "/present", "File exists", 17)
                assert(posix.umask(0) == 18)
                assert(posix.mkdir("/unmasked") == 0)
                path_error(function() return posix.rmdir("/missing") end,
                    "/missing", "No such file or directory", 2)
                path_error(function() return posix.mkfifo("/present/entry") end,
                    "/present/entry", "File exists", 17)
                path_error(function() return posix.unlink("/missing") end,
                    "/missing", "No such file or directory", 2)
                plain_error(function() return posix.link("/missing", "/hard-link") end,
                    "No such file or directory", 2)
                plain_error(function()
                    return posix.symlink("target", "/present/entry")
                end, "File exists", 17)
                path_error(function() return posix.readlink("/missing") end,
                    "/missing", "No such file or directory", 2)
                path_error(function() return posix.utime("/missing") end,
                    "/missing", "No such file or directory", 2)

                local confined, confinement_error =
                    pcall(posix.unlink, "/loop/child")
                assert(confined == false)
                assert(string.find(tostring(confinement_error),
                    "RpmTargetPathError: symlink depth exceeds 64", 1, true))
            "#,
            &[],
            &[],
            &runtime(),
            Duration::from_secs(5),
        )
        .expect("pinned lposix filesystem result contracts");

        assert_eq!(
            std::fs::metadata(root.path().join("unmasked"))
                .expect("unmasked directory")
                .permissions()
                .mode()
                & 0o777,
            0o777
        );
    }

    #[test]
    fn embedded_lua_runs_the_fedora_crypto_policies_post_install_body() {
        let root = tempfile::tempdir().expect("target root");
        std::fs::create_dir_all(root.path().join("etc/crypto-policies/back-ends"))
            .expect("backend directory");
        std::fs::create_dir_all(root.path().join("etc/crypto-policies/state"))
            .expect("state directory");
        std::fs::create_dir_all(root.path().join("usr/share/crypto-policies/DEFAULT"))
            .expect("default policy directory");
        std::fs::write(
            root.path()
                .join("usr/share/crypto-policies/DEFAULT/openssl.txt"),
            b"backend",
        )
        .expect("policy backend");
        let executor = ScriptletExecutor::new(
            root.path(),
            "crypto-policies",
            "20251128-3.git19878fe.fc44",
            PackageFormat::Rpm,
        );

        execute_embedded_lua(
            &executor,
            "post-install",
            r#"if not posix.access("/etc/crypto-policies/config") then
    local policy = "DEFAULT"
    local cf = io.open("/proc/sys/crypto/fips_enabled", "r")
    if cf then
        if cf:read() == "1" then
            policy = "FIPS"
        end
        cf:close()
    end
    cf = io.open("/etc/crypto-policies/config", "w")
    if cf then
        cf:write(policy.."\n")
        cf:close()
    end
    cf = io.open("/etc/crypto-policies/state/current", "w")
    if cf then
        cf:write(policy.."\n")
        cf:close()
    end
    local policypath = "/usr/share/crypto-policies/"..policy
    for fn in posix.files(policypath) do
        if fn ~= "." and fn ~= ".." then
            local backend = fn:gsub(".*/", ""):gsub("%..*", "")
            local cfgfn = "/etc/crypto-policies/back-ends/"..backend..".config"
            posix.unlink(cfgfn)
            posix.symlink(policypath.."/"..fn, cfgfn)
        end
    end
else
    if posix.access("/var/lib/rpm-state/crypto-policies/autopolicy-reapplication-needed") then
        os.execute("/usr/libexec/fips-crypto-policy-overlay >/dev/null 2>/dev/null || :")
        posix.unlink("/var/lib/rpm-state/crypto-policies/autopolicy-reapplication-needed")
    end
end"#,
            &["1".to_string()],
            &[],
            &runtime(),
            Duration::from_secs(5),
        )
        .expect("Fedora crypto-policies post-install body");

        assert_eq!(
            std::fs::read(root.path().join("etc/crypto-policies/config")).expect("selected policy"),
            b"DEFAULT\n"
        );
        assert_eq!(
            std::fs::read(root.path().join("etc/crypto-policies/state/current"))
                .expect("current policy"),
            b"DEFAULT\n"
        );
        assert_eq!(
            std::fs::read_link(
                root.path()
                    .join("etc/crypto-policies/back-ends/openssl.config")
            )
            .expect("backend link"),
            std::path::PathBuf::from("/usr/share/crypto-policies/DEFAULT/openssl.txt")
        );
    }

    #[test]
    fn embedded_lua_runs_the_fedora_bash_shell_registration_body() {
        let root = tempfile::tempdir().expect("target root");
        std::fs::create_dir(root.path().join("etc")).expect("etc directory");
        std::fs::write(root.path().join("etc/shells"), b"/bin/sh\n").expect("shell registry");
        let executor =
            ScriptletExecutor::new(root.path(), "bash", "5.3.9-3.fc44", PackageFormat::Rpm);
        let body = r#"
            nl='\n'
            sh='/bin/sh'..nl
            bash='/bin/bash'..nl
            f=io.open('/etc/shells','a+')
            if f then
              local shells=nl..f:read('*all')..nl
              if not shells:find(nl..sh) then f:write(sh) end
              if not shells:find(nl..bash) then f:write(bash) end
              f:close()
            end
        "#;

        for _ in 0..2 {
            execute_embedded_lua(
                &executor,
                "post-install",
                body,
                &["1".to_string()],
                &[],
                &runtime(),
                Duration::from_secs(5),
            )
            .expect("Fedora Bash post-install body");
        }

        assert_eq!(
            std::fs::read(root.path().join("etc/shells")).expect("updated shell registry"),
            b"/bin/sh\n/bin/bash\n"
        );
    }

    #[test]
    fn embedded_lua_leaf_sensitive_filesystem_calls_preserve_symlink_entries() {
        let root = tempfile::tempdir().expect("target root");
        std::fs::create_dir_all(root.path().join("usr/bin")).expect("usr bin");
        std::fs::write(root.path().join("usr/bin/kept"), b"kept").expect("target file");
        std::os::unix::fs::symlink("bin", root.path().join("usr/sbin")).expect("usr sbin symlink");
        std::os::unix::fs::symlink("usr", root.path().join("alias")).expect("ancestor alias");
        let executor =
            ScriptletExecutor::new(root.path(), "filesystem", "3.18", PackageFormat::Rpm);

        execute_embedded_lua(
            &executor,
            "post-transaction",
            r#"
                assert(posix.stat("/usr/sbin", "type") == "link")
                assert(posix.readlink("/usr/sbin") == "bin")

                -- This is the decisive branch in Fedora filesystem's
                -- usrmerge post-transaction script. A leaf-following stat
                -- incorrectly enters it and deletes /usr/bin.
                local st = posix.stat("/usr/sbin")
                if st and st.type ~= "link" then
                    os.remove("/usr/sbin")
                    posix.symlink("bin", "/usr/sbin")
                end
                assert(posix.stat("/usr/bin", "type") == "directory")
                assert(posix.readlink("/usr/sbin") == "bin")

                assert(os.remove("/usr/sbin"))
                assert(posix.stat("/usr/bin", "type") == "directory")
                assert(posix.stat("/usr/bin/kept", "size") == 4)

                assert(posix.symlink("bin", "/usr/sbin") == 0)
                assert(posix.unlink("/usr/sbin") == 0)
                assert(posix.stat("/usr/bin", "type") == "directory")

                assert(posix.symlink("bin", "/usr/sbin") == 0)
                assert(os.rename("/usr/sbin", "/usr/admin"))
                assert(posix.readlink("/usr/admin") == "bin")
                assert(posix.stat("/usr/bin", "type") == "directory")

                assert(posix.link("/usr/admin", "/usr/admin-hard") == 0)
                assert(posix.stat("/usr/admin-hard", "type") == "link")
                assert(posix.readlink("/usr/admin-hard") == "bin")

                assert(posix.symlink("bin", "/alias/sbin") == 0)
                assert(posix.readlink("/usr/sbin") == "bin")
            "#,
            &[],
            &[],
            &runtime(),
            Duration::from_secs(5),
        )
        .expect("leaf-sensitive target-root APIs");

        assert!(root.path().join("usr/bin").is_dir());
        assert_eq!(
            std::fs::read_link(root.path().join("usr/admin")).expect("renamed symlink"),
            std::path::PathBuf::from("bin")
        );
        assert_eq!(
            std::fs::read_link(root.path().join("usr/admin-hard")).expect("linked symlink"),
            std::path::PathBuf::from("bin")
        );
        assert_eq!(
            std::fs::read_link(root.path().join("usr/sbin")).expect("ancestor-resolved symlink"),
            std::path::PathBuf::from("bin")
        );
    }

    #[test]
    fn embedded_lua_rejects_malformed_passwd_uid() {
        let root = tempfile::tempdir().expect("target root");
        std::fs::create_dir_all(root.path().join("etc")).expect("account database directory");
        std::fs::write(
            root.path().join("etc/passwd"),
            "broken:x:not-a-uid:1001:Broken:/home/broken:/bin/sh\n",
        )
        .expect("passwd database");
        let executor = ScriptletExecutor::new(root.path(), "demo", "1.0", PackageFormat::Rpm);

        let error = execute_embedded_lua(
            &executor,
            "post-install",
            r#"posix.getpasswd("broken")"#,
            &[],
            &[],
            &runtime(),
            Duration::from_secs(5),
        )
        .unwrap_err()
        .to_string();

        assert!(
            error.contains("RpmEmbeddedLuaInvalidAccountEntry"),
            "{error}"
        );
        assert!(error.contains("/etc/passwd"), "{error}");
        assert!(error.contains("invalid uid 'not-a-uid'"), "{error}");
    }

    #[test]
    fn embedded_lua_rejects_malformed_group_gid() {
        let root = tempfile::tempdir().expect("target root");
        std::fs::create_dir_all(root.path().join("etc")).expect("account database directory");
        std::fs::write(root.path().join("etc/group"), "broken:x:not-a-gid:\n")
            .expect("group database");
        let executor = ScriptletExecutor::new(root.path(), "demo", "1.0", PackageFormat::Rpm);

        let error = execute_embedded_lua(
            &executor,
            "post-install",
            r#"posix.getgroup("broken")"#,
            &[],
            &[],
            &runtime(),
            Duration::from_secs(5),
        )
        .unwrap_err()
        .to_string();

        assert!(
            error.contains("RpmEmbeddedLuaInvalidAccountEntry"),
            "{error}"
        );
        assert!(error.contains("/etc/group"), "{error}");
        assert!(error.contains("invalid gid 'not-a-gid'"), "{error}");
    }

    #[test]
    fn embedded_lua_spawn_redirections_are_target_confined() {
        let Some(root) = materialized_root(
            "scriptlet::rpm_runtime::lua::tests::embedded_lua_spawn_redirections_are_target_confined",
            &["/bin/sh"],
        ) else {
            return;
        };
        let output_path = "/tmp/stdout";
        let error_path = "/tmp/stderr";
        let body = format!(
            r#"
                assert(rpm.spawn(
                    {{"/bin/sh", "-c", "printf stdout; printf stderr >&2"}},
                    {{stdout={output_path:?}, stderr={error_path:?}}}
                ) == 0)
            "#
        );
        let executor = ScriptletExecutor::new(root.path(), "demo", "1.0", PackageFormat::Rpm)
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
            std::fs::read_to_string(root.host_path(output_path)).expect("stdout"),
            "stdout"
        );
        assert_eq!(
            std::fs::read_to_string(root.host_path(error_path)).expect("stderr"),
            "stderr"
        );
    }
}
