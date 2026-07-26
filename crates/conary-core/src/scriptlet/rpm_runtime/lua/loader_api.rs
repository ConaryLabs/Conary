// conary-core/src/scriptlet/rpm_runtime/lua/loader_api.rs
//! Pure-Lua module loading confined to the RPM install target.

use super::context::LuaRuntimeContext;
use mlua::{Function, Lua, MultiValue, Table, Value};

const RPM_LUA_PATH: &str = "/usr/lib/rpm/lua/?.lua";

pub(super) fn install(lua: &Lua, context: LuaRuntimeContext) -> mlua::Result<()> {
    let package = lua.create_table()?;
    let loaded = lua.create_table()?;
    let preload = lua.create_table()?;
    package.set("loaded", loaded.clone())?;
    package.set("preload", preload.clone())?;
    package.set("path", RPM_LUA_PATH)?;
    package.set("cpath", "")?;
    package.set("config", "/\n;\n?\n!\n-\n")?;

    let search_context = context.clone();
    package.set(
        "searchpath",
        lua.create_function(
            move |lua,
                  (name, path, separator, replacement): (
                String,
                String,
                Option<String>,
                Option<String>,
            )| {
                searchpath(
                    lua,
                    &search_context,
                    &name,
                    &path,
                    separator.as_deref().unwrap_or("."),
                    replacement.as_deref().unwrap_or("/"),
                )
            },
        )?,
    )?;
    package.set(
        "loadlib",
        lua.create_function(|lua, (path, symbol): (String, String)| {
            Ok(MultiValue::from_vec(vec![
                Value::Nil,
                Value::String(lua.create_string(format!(
                    "target native Lua module '{path}' ({symbol}) cannot be loaded into Conary's bundled VM"
                ))?),
                Value::String(lua.create_string("open")?),
            ]))
        })?,
    )?;

    let searchers = lua.create_table()?;
    let preload_search = preload.clone();
    searchers.raw_set(
        1,
        lua.create_function(move |lua, name: String| {
            match preload_search.get::<Value>(name.as_str())? {
                Value::Function(function) => Ok(MultiValue::from_vec(vec![
                    Value::Function(function),
                    Value::String(lua.create_string(":preload:")?),
                ])),
                _ => Ok(MultiValue::from_vec(vec![Value::String(
                    lua.create_string(format!("\n\tno field package.preload['{name}']"))?,
                )])),
            }
        })?,
    )?;
    let file_search_context = context.clone();
    searchers.raw_set(
        2,
        lua.create_function(move |lua, name: String| {
            let path = match resolve_module(&file_search_context, &name, RPM_LUA_PATH) {
                Ok(path) => path,
                Err(error) => {
                    return Ok(MultiValue::from_vec(vec![Value::String(
                        lua.create_string(error)?,
                    )]));
                }
            };
            let function = load_target_function(lua, &file_search_context, &path, None)?;
            Ok(MultiValue::from_vec(vec![
                Value::Function(function),
                Value::String(lua.create_string(path)?),
            ]))
        })?,
    )?;
    package.set("searchers", searchers)?;
    lua.globals().set("package", package)?;

    let loadfile_context = context.clone();
    lua.globals().set(
        "loadfile",
        lua.create_function(
            move |lua, (path, _mode, environment): (String, Option<String>, Option<Table>)| {
                let function = load_target_function(lua, &loadfile_context, &path, environment)?;
                Ok(function)
            },
        )?,
    )?;
    let dofile_context = context.clone();
    lua.globals().set(
        "dofile",
        lua.create_function(move |lua, path: String| {
            load_target_function(lua, &dofile_context, &path, None)?.call::<MultiValue>(())
        })?,
    )?;

    let require_context = context;
    lua.globals().set(
        "require",
        lua.create_function(move |lua, name: String| {
            let loaded: Table = lua.globals().get::<Table>("package")?.get("loaded")?;
            let current = loaded.get::<Value>(name.as_str())?;
            if !matches!(current, Value::Nil | Value::Boolean(false)) {
                return Ok(current);
            }
            let preload: Table = lua.globals().get::<Table>("package")?.get("preload")?;
            let (loader, loader_data) = match preload.get::<Value>(name.as_str())? {
                Value::Function(function) => (function, ":preload:".to_string()),
                _ => {
                    let package: Table = lua.globals().get("package")?;
                    let path = package.get::<String>("path")?;
                    let path = resolve_module(&require_context, &name, &path)
                        .map_err(mlua::Error::runtime)?;
                    (
                        load_target_function(lua, &require_context, &path, None)?,
                        path,
                    )
                }
            };
            let result = loader.call::<Value>((name.clone(), loader_data))?;
            let result = if matches!(result, Value::Nil) {
                Value::Boolean(true)
            } else {
                result
            };
            loaded.set(name, result.clone())?;
            Ok(result)
        })?,
    )
}

fn load_target_function(
    lua: &Lua,
    context: &LuaRuntimeContext,
    guest: &str,
    environment: Option<Table>,
) -> mlua::Result<Function> {
    let bytes = context.read(guest).map_err(mlua::Error::external)?;
    let chunk = lua.load(bytes).set_name(format!("@{guest}"));
    let function = match environment {
        Some(environment) => chunk.set_environment(environment).into_function()?,
        None => chunk.into_function()?,
    };
    Ok(function)
}

fn resolve_module(context: &LuaRuntimeContext, name: &str, path: &str) -> Result<String, String> {
    let name = name.replace('.', "/");
    let mut errors = String::new();
    for template in path.split(';').filter(|template| !template.is_empty()) {
        let candidate = template.replace('?', &name);
        match context.resolve(&candidate) {
            Ok(path) if path.is_file() => return Ok(candidate),
            _ => errors.push_str(&format!("\n\tno file '{candidate}'")),
        }
    }
    Err(errors)
}

fn searchpath(
    lua: &Lua,
    context: &LuaRuntimeContext,
    name: &str,
    path: &str,
    separator: &str,
    replacement: &str,
) -> mlua::Result<MultiValue> {
    let name = name.replace(separator, replacement);
    let mut errors = String::new();
    for template in path.split(';').filter(|template| !template.is_empty()) {
        let candidate = template.replace('?', &name);
        match context.resolve(&candidate) {
            Ok(path) if path.is_file() => {
                return Ok(MultiValue::from_vec(vec![Value::String(
                    lua.create_string(candidate)?,
                )]));
            }
            _ => errors.push_str(&format!("\n\tno file '{candidate}'")),
        }
    }
    Ok(MultiValue::from_vec(vec![
        Value::Nil,
        Value::String(lua.create_string(errors)?),
    ]))
}
