// conary-core/src/scriptlet/rpm_runtime/lua/rpm_api.rs
//! Remaining RPM-specific Lua APIs from the pinned RPM runtime.

use super::context::LuaRuntimeContext;
use crate::repository::versioning::{VersionScheme, compare_repo_versions};
use crate::scriptlet::rpm_runtime::RpmMacroEngine;
use mlua::{
    Function, Lua, MetaMethod, RegistryKey, Table, UserData, UserDataFields, UserDataMethods,
    Value, Variadic,
};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

pub(super) fn install(
    lua: &Lua,
    table: &Table,
    engine: Rc<RefCell<RpmMacroEngine>>,
    context: LuaRuntimeContext,
) -> mlua::Result<()> {
    let load_engine = engine;
    table.set(
        "load",
        lua.create_function(move |_, path: String| {
            load_engine
                .borrow()
                .expand_macro_args("load", &[path])
                .map(|_| 0)
                .map_err(mlua::Error::external)
        })?,
    )?;
    let glob_context = context;
    table.set(
        "glob",
        lua.create_function(move |lua, (pattern, flags): (String, Option<String>)| {
            let values = target_glob(
                &glob_context,
                &pattern,
                flags.as_deref().unwrap_or("").contains('c'),
            )
            .map_err(mlua::Error::external)?;
            let table = lua.create_table()?;
            for (index, value) in values.into_iter().enumerate() {
                table.raw_set(index + 1, value)?;
            }
            Ok(table)
        })?,
    )?;
    table.set(
        "ver",
        lua.create_function(|_, values: Variadic<String>| {
            RpmVersion::from_arguments(&values).map_err(mlua::Error::runtime)
        })?,
    )?;
    install_hooks(lua, table)?;
    table.set(
        "interactive",
        lua.create_function(|_, ()| {
            Err::<Value, _>(mlua::Error::runtime("interactive RPM Lua requires a tty"))
        })?,
    )
}

fn install_hooks(lua: &Lua, table: &Table) -> mlua::Result<()> {
    let hooks = Rc::new(RefCell::new(BTreeMap::<u64, (String, RegistryKey)>::new()));
    let next_id = Rc::new(std::cell::Cell::new(1u64));

    let register_hooks = Rc::clone(&hooks);
    let register_id = Rc::clone(&next_id);
    table.set(
        "register",
        lua.create_function(move |lua, (name, function): (String, Function)| {
            let id = register_id.get();
            register_id.set(id.saturating_add(1));
            register_hooks
                .borrow_mut()
                .insert(id, (name, lua.create_registry_value(function)?));
            Ok(id)
        })?,
    )?;

    let unregister_hooks = Rc::clone(&hooks);
    table.set(
        "unregister",
        lua.create_function(move |lua, (name, id): (String, u64)| {
            let mut hooks = unregister_hooks.borrow_mut();
            let remove = hooks.get(&id).is_some_and(|(hook, _)| hook == &name);
            if remove && let Some((_, key)) = hooks.remove(&id) {
                lua.remove_registry_value(key)?;
            }
            Ok(())
        })?,
    )?;

    table.set(
        "call",
        lua.create_function(move |lua, (name, values): (String, Variadic<Value>)| {
            let arguments = lua.create_table()?;
            for (index, value) in values.into_iter().enumerate() {
                arguments.raw_set(index + 1, value)?;
            }
            let hooks = hooks.borrow();
            for (hook, key) in hooks.values() {
                if hook == &name {
                    let function: Function = lua.registry_value(key)?;
                    let _: Value = function.call(arguments.clone())?;
                }
            }
            Ok(())
        })?,
    )
}

#[derive(Clone)]
struct RpmVersion {
    epoch: Option<String>,
    version: String,
    release: Option<String>,
}

impl RpmVersion {
    fn from_arguments(arguments: &[String]) -> Result<Self, String> {
        match arguments {
            [evr] => Self::parse(evr),
            [epoch, version, release] => {
                if version.is_empty() {
                    return Err("RPM version component cannot be empty".to_string());
                }
                Ok(Self {
                    epoch: (!epoch.is_empty()).then(|| epoch.clone()),
                    version: version.clone(),
                    release: (!release.is_empty()).then(|| release.clone()),
                })
            }
            _ => Err(format!(
                "rpm.ver expects one EVR or epoch, version, release; got {} arguments",
                arguments.len()
            )),
        }
    }

    fn parse(evr: &str) -> Result<Self, String> {
        let (epoch, rest) = evr
            .split_once(':')
            .map_or((None, evr), |(epoch, rest)| (Some(epoch.to_string()), rest));
        let (version, release) = rest
            .rsplit_once('-')
            .map_or((rest, None), |(version, release)| {
                (version, Some(release.to_string()))
            });
        if version.is_empty() {
            return Err(format!("invalid empty RPM version in '{evr}'"));
        }
        Ok(Self {
            epoch,
            version: version.to_string(),
            release,
        })
    }

    fn evr(&self) -> String {
        let mut value = String::new();
        if let Some(epoch) = &self.epoch {
            value.push_str(epoch);
            value.push(':');
        }
        value.push_str(&self.version);
        if let Some(release) = &self.release {
            value.push('-');
            value.push_str(release);
        }
        value
    }
}

impl UserData for RpmVersion {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("e", |_, version| Ok(version.epoch.clone()));
        fields.add_field_method_get("v", |_, version| Ok(version.version.clone()));
        fields.add_field_method_get("r", |_, version| Ok(version.release.clone()));
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method(MetaMethod::ToString, |_, version, ()| Ok(version.evr()));
        methods.add_meta_function(
            MetaMethod::Eq,
            |_, (left, right): (mlua::AnyUserData, mlua::AnyUserData)| {
                let left = left.borrow::<Self>()?;
                let right = right.borrow::<Self>()?;
                Ok(compare(&left, &right)? == std::cmp::Ordering::Equal)
            },
        );
        methods.add_meta_function(
            MetaMethod::Lt,
            |_, (left, right): (mlua::AnyUserData, mlua::AnyUserData)| {
                let left = left.borrow::<Self>()?;
                let right = right.borrow::<Self>()?;
                Ok(compare(&left, &right)? == std::cmp::Ordering::Less)
            },
        );
        methods.add_meta_function(
            MetaMethod::Le,
            |_, (left, right): (mlua::AnyUserData, mlua::AnyUserData)| {
                let left = left.borrow::<Self>()?;
                let right = right.borrow::<Self>()?;
                Ok(compare(&left, &right)? != std::cmp::Ordering::Greater)
            },
        );
    }
}

fn compare(left: &RpmVersion, right: &RpmVersion) -> mlua::Result<std::cmp::Ordering> {
    compare_repo_versions(VersionScheme::Rpm, &left.evr(), &right.evr())
        .map_err(mlua::Error::external)
}

fn target_glob(
    context: &LuaRuntimeContext,
    pattern: &str,
    no_check: bool,
) -> anyhow::Result<Vec<String>> {
    let matcher = glob::Pattern::new(pattern)?;
    let cwd = context.cwd();
    let mut matches = Vec::new();
    for entry in walkdir::WalkDir::new(context.root())
        .follow_links(false)
        .into_iter()
    {
        let entry = entry?;
        let relative = entry.path().strip_prefix(context.root())?;
        let absolute = if relative.as_os_str().is_empty() {
            "/".to_string()
        } else {
            format!("/{}", relative.to_string_lossy())
        };
        let candidate = if pattern.starts_with('/') {
            absolute
        } else if cwd == "/" {
            absolute.trim_start_matches('/').to_string()
        } else {
            let prefix = format!("{}/", cwd.trim_end_matches('/'));
            let Some(relative) = absolute.strip_prefix(&prefix) else {
                continue;
            };
            relative.to_string()
        };
        if matcher.matches(&candidate) {
            matches.push(candidate);
        }
    }
    matches.sort();
    if matches.is_empty() && no_check {
        matches.push(pattern.to_string());
    }
    Ok(matches)
}
