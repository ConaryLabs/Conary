// conary-core/src/scriptlet/rpm_runtime/lua/standard_api.rs
//! Target-root implementations of RPM's `os` and `posix` Lua surfaces.

mod helpers;

use self::helpers::{parse_mode, pathconf_value, sysconf_value, target_identity};
use super::context::LuaRuntimeContext;
use chrono::{Datelike, Local, TimeZone, Timelike, Utc};
use mlua::{Lua, MultiValue, Table, Value};
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::time::Duration;

pub(super) fn install_os(lua: &Lua, context: LuaRuntimeContext) -> mlua::Result<()> {
    let table = lua.create_table()?;
    table.set(
        "clock",
        lua.create_function(|_, ()| {
            let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
            let rc = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
            if rc != 0 {
                return Ok(0.0);
            }
            let usage = unsafe { usage.assume_init() };
            Ok(timeval_seconds(usage.ru_utime) + timeval_seconds(usage.ru_stime))
        })?,
    )?;
    table.set(
        "date",
        lua.create_function(|lua, (format, timestamp): (Option<String>, Option<i64>)| {
            let format = format.unwrap_or_else(|| "%c".to_string());
            let timestamp = timestamp.unwrap_or_else(|| Utc::now().timestamp());
            let utc = format.starts_with('!');
            let format = format.strip_prefix('!').unwrap_or(&format);
            if format == "*t" {
                if utc {
                    date_table(lua, Utc.timestamp_opt(timestamp, 0).single())
                } else {
                    date_table(lua, Local.timestamp_opt(timestamp, 0).single())
                }
            } else if utc {
                Ok(Value::String(
                    lua.create_string(
                        Utc.timestamp_opt(timestamp, 0)
                            .single()
                            .ok_or_else(|| mlua::Error::runtime("timestamp out of range"))?
                            .format(format)
                            .to_string(),
                    )?,
                ))
            } else {
                Ok(Value::String(
                    lua.create_string(
                        Local
                            .timestamp_opt(timestamp, 0)
                            .single()
                            .ok_or_else(|| mlua::Error::runtime("timestamp out of range"))?
                            .format(format)
                            .to_string(),
                    )?,
                ))
            }
        })?,
    )?;
    table.set(
        "difftime",
        lua.create_function(
            |_, (left, right): (f64, Option<f64>)| Ok(left - right.unwrap_or(0.0)),
        )?,
    )?;
    let getenv_context = context.clone();
    table.set(
        "getenv",
        lua.create_function(move |_, name: String| Ok(getenv_context.getenv(&name)))?,
    )?;
    let remove_context = context.clone();
    table.set(
        "remove",
        lua.create_function(move |_, path: String| {
            let path = remove_context
                .resolve_without_following_leaf(&path)
                .map_err(mlua::Error::external)?;
            let metadata = std::fs::symlink_metadata(&path).map_err(mlua::Error::external)?;
            if metadata.file_type().is_dir() {
                std::fs::remove_dir(&path).map_err(mlua::Error::external)?;
            } else {
                std::fs::remove_file(&path).map_err(mlua::Error::external)?;
            }
            Ok(true)
        })?,
    )?;
    let rename_context = context.clone();
    table.set(
        "rename",
        lua.create_function(move |_, (from, to): (String, String)| {
            let from = rename_context
                .resolve_without_following_leaf(&from)
                .map_err(mlua::Error::external)?;
            let to = rename_context
                .resolve_without_following_leaf(&to)
                .map_err(mlua::Error::external)?;
            std::fs::rename(from, to).map_err(mlua::Error::external)?;
            Ok(true)
        })?,
    )?;
    table.set(
        "setlocale",
        lua.create_function(|_, (locale, _category): (Option<String>, Option<String>)| {
            Ok(match locale.as_deref() {
                None | Some("") | Some("C") | Some("POSIX") => Some("C".to_string()),
                _ => None,
            })
        })?,
    )?;
    table.set(
        "time",
        lua.create_function(|_, fields: Option<Table>| {
            let Some(fields) = fields else {
                return Ok(Utc::now().timestamp());
            };
            let year = fields.get::<i32>("year")?;
            let month = fields.get::<u32>("month")?;
            let day = fields.get::<u32>("day")?;
            let hour = fields.get::<Option<u32>>("hour")?.unwrap_or(12);
            let minute = fields.get::<Option<u32>>("min")?.unwrap_or(0);
            let second = fields.get::<Option<u32>>("sec")?.unwrap_or(0);
            Local
                .with_ymd_and_hms(year, month, day, hour, minute, second)
                .single()
                .map(|value| value.timestamp())
                .ok_or_else(|| mlua::Error::runtime("date fields are out of range"))
        })?,
    )?;
    let tmp_context = context.clone();
    table.set(
        "tmpname",
        lua.create_function(move |_, ()| {
            let directory = tmp_context.resolve("/tmp").map_err(mlua::Error::external)?;
            std::fs::create_dir_all(&directory).map_err(mlua::Error::external)?;
            let file = tempfile::Builder::new()
                .prefix("lua_")
                .tempfile_in(directory)
                .map_err(mlua::Error::external)?;
            let host = file
                .into_temp_path()
                .keep()
                .map_err(mlua::Error::external)?;
            let relative = host
                .strip_prefix(tmp_context.root())
                .map_err(mlua::Error::external)?;
            Ok(format!("/{}", relative.to_string_lossy()))
        })?,
    )?;
    process_api(lua, &table, context)?;
    table.set(
        "exit",
        lua.create_function(|_, _: MultiValue| {
            Err::<Value, _>(mlua::Error::runtime("exit not permitted in this context"))
        })?,
    )?;
    lua.globals().set("os", table)
}

fn process_api(lua: &Lua, table: &Table, context: LuaRuntimeContext) -> mlua::Result<()> {
    super::process_api::install_os_execute(lua, table, context)
}

pub(super) fn install_posix(lua: &Lua, context: LuaRuntimeContext) -> mlua::Result<()> {
    let table = lua.create_table()?;
    install_posix_paths(lua, &table, context.clone())?;
    install_posix_environment(lua, &table, context.clone())?;
    install_posix_metadata(lua, &table, context.clone())?;
    install_posix_process(lua, &table, context)?;
    table.set("version", "rpm-bundled-target-1")?;
    lua.globals().set("posix", table)
}

fn install_posix_paths(lua: &Lua, table: &Table, context: LuaRuntimeContext) -> mlua::Result<()> {
    let access_context = context.clone();
    table.set(
        "access",
        lua.create_function(move |_, (path, mode): (String, Option<String>)| {
            let path = access_context
                .resolve(&path)
                .map_err(mlua::Error::external)?;
            let mode = mode.unwrap_or_else(|| "f".to_string());
            if !mode
                .bytes()
                .all(|byte| matches!(byte, b'r' | b'w' | b'x' | b'f'))
            {
                return Err(mlua::Error::runtime(format!(
                    "invalid access mode '{mode}'"
                )));
            }
            let path = CString::new(path.as_os_str().as_bytes())
                .map_err(|_| mlua::Error::runtime("path contains NUL"))?;
            let mut flags = libc::F_OK;
            if mode.contains('r') {
                flags |= libc::R_OK;
            }
            if mode.contains('w') {
                flags |= libc::W_OK;
            }
            if mode.contains('x') {
                flags |= libc::X_OK;
            }
            Ok(unsafe { libc::access(path.as_ptr(), flags) } == 0)
        })?,
    )?;
    let chdir_context = context.clone();
    table.set(
        "chdir",
        lua.create_function(move |_, path: String| {
            chdir_context.chdir(&path).map_err(mlua::Error::external)?;
            Ok(0)
        })?,
    )?;
    let chmod_context = context.clone();
    table.set(
        "chmod",
        lua.create_function(move |_, (path, mode): (String, Value)| {
            let path = chmod_context
                .resolve(&path)
                .map_err(mlua::Error::external)?;
            let current = std::fs::metadata(&path)
                .map_err(mlua::Error::external)?
                .permissions()
                .mode();
            let mode = parse_mode(mode, current)?;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
                .map_err(mlua::Error::external)?;
            Ok(0)
        })?,
    )?;
    let chown_context = context.clone();
    table.set(
        "chown",
        lua.create_function(
            move |_, (path, user, group): (String, Option<Value>, Option<Value>)| {
                let path = chown_context
                    .resolve(&path)
                    .map_err(mlua::Error::external)?;
                let uid = target_identity(&chown_context, user, false)?;
                let gid = target_identity(&chown_context, group, true)?;
                let path = CString::new(path.as_os_str().as_bytes())
                    .map_err(|_| mlua::Error::runtime("path contains NUL"))?;
                let rc = unsafe { libc::chown(path.as_ptr(), uid, gid) };
                if rc != 0 {
                    return Err(mlua::Error::external(std::io::Error::last_os_error()));
                }
                Ok(0)
            },
        )?,
    )?;
    let mkdir_context = context.clone();
    table.set(
        "mkdir",
        lua.create_function(move |_, (path, mode): (String, Option<u32>)| {
            let path = mkdir_context
                .resolve_without_following_leaf(&path)
                .map_err(mlua::Error::external)?;
            std::fs::create_dir(&path).map_err(mlua::Error::external)?;
            let mode = mode.unwrap_or(0o777) & !mkdir_context.umask();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
                .map_err(mlua::Error::external)?;
            Ok(0)
        })?,
    )?;
    let rmdir_context = context.clone();
    table.set(
        "rmdir",
        lua.create_function(move |_, path: String| {
            std::fs::remove_dir(
                rmdir_context
                    .resolve_without_following_leaf(&path)
                    .map_err(mlua::Error::external)?,
            )
            .map_err(mlua::Error::external)?;
            Ok(0)
        })?,
    )?;
    let fifo_context = context.clone();
    table.set(
        "mkfifo",
        lua.create_function(move |_, path: String| {
            let path = fifo_context
                .resolve_without_following_leaf(&path)
                .map_err(mlua::Error::external)?;
            let path = CString::new(path.as_os_str().as_bytes())
                .map_err(|_| mlua::Error::runtime("path contains NUL"))?;
            let rc = unsafe { libc::mkfifo(path.as_ptr(), 0o777 & !fifo_context.umask()) };
            if rc != 0 {
                return Err(mlua::Error::external(std::io::Error::last_os_error()));
            }
            Ok(0)
        })?,
    )?;
    let unlink_context = context.clone();
    table.set(
        "unlink",
        lua.create_function(move |_, path: String| {
            std::fs::remove_file(
                unlink_context
                    .resolve_without_following_leaf(&path)
                    .map_err(mlua::Error::external)?,
            )
            .map_err(mlua::Error::external)?;
            Ok(0)
        })?,
    )?;
    let link_context = context.clone();
    table.set(
        "link",
        lua.create_function(move |_, (old, new): (String, String)| {
            let old = link_context
                .resolve_without_following_leaf(&old)
                .map_err(mlua::Error::external)?;
            let new = link_context
                .resolve_without_following_leaf(&new)
                .map_err(mlua::Error::external)?;
            std::fs::hard_link(old, new).map_err(mlua::Error::external)?;
            Ok(0)
        })?,
    )?;
    let symlink_context = context.clone();
    table.set(
        "symlink",
        lua.create_function(move |_, (old, new): (String, String)| {
            let new = symlink_context
                .resolve_without_following_leaf(&new)
                .map_err(mlua::Error::external)?;
            std::os::unix::fs::symlink(old, new).map_err(mlua::Error::external)?;
            Ok(0)
        })?,
    )?;
    let readlink_context = context.clone();
    table.set(
        "readlink",
        lua.create_function(move |_, path: String| {
            let path = readlink_context
                .resolve_without_following_leaf(&path)
                .map_err(mlua::Error::external)?;
            Ok(std::fs::read_link(path)
                .map_err(mlua::Error::external)?
                .to_string_lossy()
                .to_string())
        })?,
    )?;
    let dir_context = context.clone();
    table.set(
        "dir",
        lua.create_function(move |lua, path: Option<String>| {
            let guest = path.as_deref().unwrap_or(".");
            let entries = match directory_entries(&dir_context, guest)? {
                Ok(entries) => entries,
                Err(error) => return posix_path_error(lua, guest, error),
            };
            Ok(MultiValue::from_vec(vec![Value::Table(directory_table(
                lua, entries,
            )?)]))
        })?,
    )?;
    let files_context = context.clone();
    table.set(
        "files",
        lua.create_function(move |lua, path: Option<String>| {
            let guest = path.as_deref().unwrap_or(".");
            let entries = match directory_entries(&files_context, guest)? {
                Ok(entries) => entries,
                Err(error) => return posix_path_error(lua, guest, error),
            };
            let entries = std::rc::Rc::new(std::cell::RefCell::new(entries.into_iter()));
            let iterator = lua.create_function(move |_, ()| Ok(entries.borrow_mut().next()))?;
            Ok(MultiValue::from_vec(vec![Value::Function(iterator)]))
        })?,
    )?;
    table.set("ctermid", lua.create_function(|_, ()| Ok("/dev/tty"))?)?;
    let temp_context = context.clone();
    table.set(
        "mkstemp",
        lua.create_function(move |_, template: String| {
            let marker = template
                .rfind("XXXXXX")
                .ok_or_else(|| mlua::Error::runtime("mkstemp template requires XXXXXX"))?;
            let guest_directory =
                template[..marker]
                    .rsplit_once('/')
                    .map_or(
                        ".",
                        |(directory, _)| {
                            if directory.is_empty() { "/" } else { directory }
                        },
                    );
            let prefix = template[..marker]
                .rsplit('/')
                .next()
                .unwrap_or("")
                .trim_end_matches("XXXXXX");
            let directory = temp_context
                .resolve(guest_directory)
                .map_err(mlua::Error::external)?;
            let file = tempfile::Builder::new()
                .prefix(prefix)
                .tempfile_in(directory)
                .map_err(mlua::Error::external)?;
            let (file, host) = file.keep().map_err(mlua::Error::external)?;
            let relative = host
                .strip_prefix(temp_context.root())
                .map_err(mlua::Error::external)?;
            let guest = format!("/{}", relative.to_string_lossy());
            Ok((super::file_api::from_std_file(guest.clone(), file), guest))
        })?,
    )?;
    table.set(
        "getcwd",
        lua.create_function(move |_, ()| Ok(context.cwd()))?,
    )
}

fn install_posix_environment(
    lua: &Lua,
    table: &Table,
    context: LuaRuntimeContext,
) -> mlua::Result<()> {
    let getenv_context = context.clone();
    table.set(
        "getenv",
        lua.create_function(move |_, name: String| Ok(getenv_context.getenv(&name)))?,
    )?;
    let putenv_context = context.clone();
    table.set(
        "putenv",
        lua.create_function(move |_, assignment: String| {
            let (name, value) = assignment
                .split_once('=')
                .ok_or_else(|| mlua::Error::runtime("putenv expects NAME=VALUE"))?;
            putenv_context
                .setenv(name, Some(value.to_string()))
                .map_err(mlua::Error::external)?;
            Ok(0)
        })?,
    )?;
    let setenv_context = context.clone();
    table.set(
        "setenv",
        lua.create_function(move |_, (name, value): (String, String)| {
            setenv_context
                .setenv(&name, Some(value))
                .map_err(mlua::Error::external)?;
            Ok(0)
        })?,
    )?;
    let unsetenv_context = context.clone();
    table.set(
        "unsetenv",
        lua.create_function(move |_, name: String| {
            unsetenv_context
                .setenv(&name, None)
                .map_err(mlua::Error::external)?;
            Ok(0)
        })?,
    )?;
    let login_context = context.clone();
    table.set(
        "getlogin",
        lua.create_function(move |_, ()| {
            Ok(login_context
                .getenv("LOGNAME")
                .or_else(|| login_context.getenv("USER"))
                .unwrap_or_else(|| "root".to_string()))
        })?,
    )?;
    table.set(
        "errno",
        lua.create_function(|_, ()| {
            let error = std::io::Error::last_os_error();
            Ok((error.to_string(), error.raw_os_error().unwrap_or(0)))
        })?,
    )
}

fn install_posix_metadata(
    lua: &Lua,
    table: &Table,
    context: LuaRuntimeContext,
) -> mlua::Result<()> {
    let stat_context = context.clone();
    table.set(
        "stat",
        lua.create_function(move |lua, (path, selector): (String, Option<String>)| {
            let target = stat_context
                .resolve_without_following_leaf(&path)
                .map_err(mlua::Error::external)?;
            match std::fs::symlink_metadata(target) {
                Ok(metadata) => Ok(MultiValue::from_vec(vec![stat_value(
                    lua,
                    &metadata,
                    selector.as_deref(),
                )?])),
                Err(error) => posix_path_error(lua, &path, error),
            }
        })?,
    )?;
    let utime_context = context.clone();
    table.set(
        "utime",
        lua.create_function(
            move |_, (path, modified, accessed): (String, Option<i64>, Option<i64>)| {
                let path = utime_context
                    .resolve(&path)
                    .map_err(mlua::Error::external)?;
                let now = Utc::now().timestamp();
                let times = libc::utimbuf {
                    modtime: modified.unwrap_or(now),
                    actime: accessed.unwrap_or(now),
                };
                let path = CString::new(path.as_os_str().as_bytes())
                    .map_err(|_| mlua::Error::runtime("path contains NUL"))?;
                let rc = unsafe { libc::utime(path.as_ptr(), &times) };
                if rc != 0 {
                    return Err(mlua::Error::external(std::io::Error::last_os_error()));
                }
                Ok(0)
            },
        )?,
    )?;
    let pathconf_context = context.clone();
    table.set(
        "pathconf",
        lua.create_function(move |lua, (path, selector): (String, Option<String>)| {
            let path = pathconf_context
                .resolve(&path)
                .map_err(mlua::Error::external)?;
            pathconf_value(lua, &path, selector.as_deref())
        })?,
    )?;
    table.set(
        "sysconf",
        lua.create_function(|lua, selector: Option<String>| {
            sysconf_value(lua, selector.as_deref())
        })?,
    )?;
    let passwd_context = context.clone();
    table.set(
        "getpasswd",
        lua.create_function(move |lua, identity: Option<Value>| {
            account_entry(lua, &passwd_context, "/etc/passwd", identity, false)
        })?,
    )?;
    let group_context = context.clone();
    table.set(
        "getgroup",
        lua.create_function(move |lua, identity: Value| {
            account_entry(lua, &group_context, "/etc/group", Some(identity), true)
        })?,
    )?;
    table.set(
        "uname",
        lua.create_function(|_, format: Option<String>| {
            let mut value = std::mem::MaybeUninit::<libc::utsname>::zeroed();
            if unsafe { libc::uname(value.as_mut_ptr()) } != 0 {
                return Err(mlua::Error::external(std::io::Error::last_os_error()));
            }
            let value = unsafe { value.assume_init() };
            let fields = [
                ('s', c_chars(&value.sysname)),
                ('n', c_chars(&value.nodename)),
                ('r', c_chars(&value.release)),
                ('v', c_chars(&value.version)),
                ('m', c_chars(&value.machine)),
            ];
            let mut rendered = format.unwrap_or_else(|| "%s %n %r %v %m".to_string());
            for (key, value) in fields {
                rendered = rendered.replace(&format!("%{key}"), &value);
            }
            Ok(rendered)
        })?,
    )?;
    let umask_context = context;
    table.set(
        "umask",
        lua.create_function(move |_, value: Option<u32>| {
            if let Some(value) = value {
                umask_context
                    .replace_umask(value)
                    .map_err(mlua::Error::external)
            } else {
                Ok(umask_context.umask())
            }
        })?,
    )
}

fn install_posix_process(
    lua: &Lua,
    table: &Table,
    _context: LuaRuntimeContext,
) -> mlua::Result<()> {
    for name in ["fork", "exec", "wait", "redirect2null"] {
        let name = name.to_string();
        table.set(
            name.clone(),
            lua.create_function(move |_, _: MultiValue| {
                Err::<Value, _>(mlua::Error::runtime(format!(
                    "posix.{name}() is no longer available, use rpm.spawn() or rpm.execute() instead"
                )))
            })?,
        )?;
    }
    table.set(
        "sleep",
        lua.create_function(|_, seconds: f64| {
            if seconds.is_sign_negative() || !seconds.is_finite() {
                return Err(mlua::Error::runtime("sleep duration is invalid"));
            }
            std::thread::sleep(Duration::from_secs_f64(seconds));
            Ok(0)
        })?,
    )?;
    table.set(
        "getprocessid",
        lua.create_function(|lua, selector: Option<String>| {
            let fields = [
                ("pid", unsafe { libc::getpid() } as i64),
                ("ppid", unsafe { libc::getppid() } as i64),
                ("uid", unsafe { libc::getuid() } as i64),
                ("euid", unsafe { libc::geteuid() } as i64),
                ("gid", unsafe { libc::getgid() } as i64),
                ("egid", unsafe { libc::getegid() } as i64),
                ("pgrp", unsafe { libc::getpgrp() } as i64),
            ];
            if let Some(selector) = selector {
                return fields
                    .iter()
                    .find_map(|(name, value)| (*name == selector).then_some(Value::Integer(*value)))
                    .ok_or_else(|| mlua::Error::runtime(format!("unknown selector '{selector}'")));
            }
            let table = lua.create_table()?;
            for (name, value) in fields {
                table.set(name, value)?;
            }
            Ok(Value::Table(table))
        })?,
    )?;
    table.set(
        "kill",
        lua.create_function(|_, (pid, signal): (i32, Option<i32>)| {
            let signal = signal.unwrap_or(15);
            if signal != 0 {
                return Err(mlua::Error::runtime(
                    "signals are not issued from the in-process RPM Lua VM",
                ));
            }
            let rc = unsafe { libc::kill(pid, 0) };
            Ok(if rc == 0 { 0 } else { -1 })
        })?,
    )?;
    table.set(
        "setuid",
        lua.create_function(|_, uid: u32| {
            if uid == unsafe { libc::geteuid() } {
                Ok(0)
            } else {
                Err(mlua::Error::runtime(
                    "setuid cannot change the in-process RPM Lua VM identity",
                ))
            }
        })?,
    )?;
    table.set(
        "setgid",
        lua.create_function(|_, gid: u32| {
            if gid == unsafe { libc::getegid() } {
                Ok(0)
            } else {
                Err(mlua::Error::runtime(
                    "setgid cannot change the in-process RPM Lua VM identity",
                ))
            }
        })?,
    )?;
    table.set(
        "ttyname",
        lua.create_function(|_, _: Option<i32>| Ok(Value::Nil))?,
    )?;
    table.set(
        "times",
        lua.create_function(|lua, ()| {
            let mut times = std::mem::MaybeUninit::<libc::tms>::zeroed();
            let elapsed = unsafe { libc::times(times.as_mut_ptr()) };
            let times = unsafe { times.assume_init() };
            let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as f64;
            let table = lua.create_table()?;
            table.set("utime", times.tms_utime as f64 / ticks)?;
            table.set("stime", times.tms_stime as f64 / ticks)?;
            table.set("cutime", times.tms_cutime as f64 / ticks)?;
            table.set("cstime", times.tms_cstime as f64 / ticks)?;
            table.set("elapsed", elapsed as f64 / ticks)?;
            Ok(table)
        })?,
    )
}

fn directory_table(lua: &Lua, entries: Vec<String>) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    for (index, entry) in entries.into_iter().enumerate() {
        table.raw_set(index + 1, entry)?;
    }
    Ok(table)
}

fn directory_entries(
    context: &LuaRuntimeContext,
    guest: &str,
) -> mlua::Result<std::io::Result<Vec<String>>> {
    let path = context.resolve(guest).map_err(mlua::Error::external)?;
    let mut values = vec![".".to_string(), "..".to_string()];
    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) => return Ok(Err(error)),
    };
    for entry in entries {
        let Ok(entry) = entry else {
            // RPM's bundled lposix treats a failed readdir(3) exactly like
            // end-of-directory; only opendir(3) exposes an error tuple.
            break;
        };
        values.push(entry.file_name().to_string_lossy().to_string());
    }
    Ok(Ok(values))
}

fn posix_path_error(lua: &Lua, path: &str, error: std::io::Error) -> mlua::Result<MultiValue> {
    let errno = error.raw_os_error().unwrap_or(0);
    let description = if errno == 0 {
        error.to_string()
    } else {
        // RPM 6.0.1's lposix `pusherror` returns strerror(errno), not a Lua
        // exception or Rust's parenthesized OS-error suffix.
        nix::errno::Errno::from_raw(errno).desc().to_string()
    };
    Ok(MultiValue::from_vec(vec![
        Value::Nil,
        Value::String(lua.create_string(format!("{path}: {description}"))?),
        Value::Integer(i64::from(errno)),
    ]))
}

fn stat_value(
    lua: &Lua,
    metadata: &std::fs::Metadata,
    selector: Option<&str>,
) -> mlua::Result<Value> {
    let file_type = if metadata.file_type().is_symlink() {
        "link"
    } else if metadata.is_dir() {
        "directory"
    } else if metadata.is_file() {
        "regular"
    } else {
        "other"
    };
    let values = [
        (
            "mode",
            Value::String(lua.create_string(mode_string(metadata.mode()))?),
        ),
        ("ino", Value::Integer(metadata.ino() as i64)),
        ("dev", Value::Integer(metadata.dev() as i64)),
        ("nlink", Value::Integer(metadata.nlink() as i64)),
        ("uid", Value::Integer(metadata.uid() as i64)),
        ("gid", Value::Integer(metadata.gid() as i64)),
        ("size", Value::Integer(metadata.size() as i64)),
        ("atime", Value::Integer(metadata.atime())),
        ("mtime", Value::Integer(metadata.mtime())),
        ("ctime", Value::Integer(metadata.ctime())),
        ("type", Value::String(lua.create_string(file_type)?)),
        ("_mode", Value::Integer(metadata.mode() as i64)),
    ];
    if let Some(selector) = selector {
        return values
            .into_iter()
            .find_map(|(name, value)| (name == selector).then_some(value))
            .ok_or_else(|| mlua::Error::runtime(format!("unknown stat selector '{selector}'")));
    }
    let table = lua.create_table()?;
    for (name, value) in values {
        table.set(name, value)?;
    }
    Ok(Value::Table(table))
}

fn mode_string(mode: u32) -> String {
    let bits = [
        (libc::S_IRUSR, 'r'),
        (libc::S_IWUSR, 'w'),
        (libc::S_IXUSR, 'x'),
        (libc::S_IRGRP, 'r'),
        (libc::S_IWGRP, 'w'),
        (libc::S_IXGRP, 'x'),
        (libc::S_IROTH, 'r'),
        (libc::S_IWOTH, 'w'),
        (libc::S_IXOTH, 'x'),
    ];
    bits.into_iter()
        .map(|(bit, character)| if mode & bit != 0 { character } else { '-' })
        .collect()
}

#[derive(Debug, thiserror::Error)]
#[error(
    "RpmEmbeddedLuaInvalidAccountEntry: {database} entry '{account}' has invalid {field} '{value}': {source}"
)]
struct InvalidAccountEntryId {
    database: String,
    account: String,
    field: &'static str,
    value: String,
    #[source]
    source: std::num::ParseIntError,
}

fn parse_account_id(
    database: &str,
    account: &str,
    field: &'static str,
    value: &str,
) -> mlua::Result<u32> {
    value.parse::<u32>().map_err(|source| {
        mlua::Error::external(InvalidAccountEntryId {
            database: database.to_string(),
            account: account.to_string(),
            field,
            value: value.to_string(),
            source,
        })
    })
}

fn account_entry(
    lua: &Lua,
    context: &LuaRuntimeContext,
    file: &str,
    identity: Option<Value>,
    group: bool,
) -> mlua::Result<Value> {
    let content = String::from_utf8(context.read(file).map_err(mlua::Error::external)?)
        .map_err(mlua::Error::external)?;
    let identity = match identity {
        None | Some(Value::Nil) => "0".to_string(),
        Some(Value::String(value)) => value.to_str()?.to_string(),
        Some(Value::Integer(value)) => value.to_string(),
        Some(value) => {
            return Err(mlua::Error::runtime(format!(
                "identity cannot be {}",
                value.type_name()
            )));
        }
    };
    for line in content.lines().filter(|line| !line.starts_with('#')) {
        let fields = line.split(':').collect::<Vec<_>>();
        if fields.len() < if group { 4 } else { 7 } {
            continue;
        }
        if fields[0] != identity && fields[2] != identity {
            continue;
        }
        let table = lua.create_table()?;
        table.set("name", fields[0])?;
        if group {
            table.set("gid", parse_account_id(file, fields[0], "gid", fields[2])?)?;
            for (index, member) in fields[3]
                .split(',')
                .filter(|member| !member.is_empty())
                .enumerate()
            {
                table.raw_set(index + 1, member)?;
            }
        } else {
            table.set("uid", parse_account_id(file, fields[0], "uid", fields[2])?)?;
            table.set("gid", parse_account_id(file, fields[0], "gid", fields[3])?)?;
            table.set("gecos", fields[4])?;
            table.set("dir", fields[5])?;
            table.set("shell", fields[6])?;
            table.set("passwd", fields[1])?;
        }
        return Ok(Value::Table(table));
    }
    Ok(Value::Nil)
}

fn c_chars(value: &[libc::c_char]) -> String {
    let length = value
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(value.len());
    String::from_utf8_lossy(
        &value[..length]
            .iter()
            .map(|byte| *byte as u8)
            .collect::<Vec<_>>(),
    )
    .to_string()
}

fn timeval_seconds(value: libc::timeval) -> f64 {
    value.tv_sec as f64 + value.tv_usec as f64 / 1_000_000.0
}

fn date_table<T>(lua: &Lua, value: Option<chrono::DateTime<T>>) -> mlua::Result<Value>
where
    T: TimeZone,
    T::Offset: std::fmt::Display,
{
    let value = value.ok_or_else(|| mlua::Error::runtime("timestamp out of range"))?;
    let table = lua.create_table()?;
    table.set("year", value.year())?;
    table.set("month", value.month())?;
    table.set("day", value.day())?;
    table.set("hour", value.hour())?;
    table.set("min", value.minute())?;
    table.set("sec", value.second())?;
    table.set("wday", value.weekday().num_days_from_sunday() + 1)?;
    table.set("yday", value.ordinal())?;
    table.set("isdst", false)?;
    Ok(Value::Table(table))
}
