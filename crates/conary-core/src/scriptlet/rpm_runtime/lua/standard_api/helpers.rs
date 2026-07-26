// conary-core/src/scriptlet/rpm_runtime/lua/standard_api/helpers.rs
//! Mode, identity, and POSIX configuration helpers.

use super::super::context::LuaRuntimeContext;
use mlua::{Lua, Value};
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;

pub(super) fn parse_mode(value: Value, current: u32) -> mlua::Result<u32> {
    match value {
        Value::Integer(value) => u32::try_from(value)
            .map(|value| value & 0o7777)
            .map_err(|_| mlua::Error::runtime("file mode is out of range")),
        Value::String(value) => {
            let value = value.to_str()?;
            if let Ok(mode) = u32::from_str_radix(value.as_ref(), 8) {
                return Ok(mode & 0o7777);
            }
            if value.len() == 9
                && value
                    .bytes()
                    .enumerate()
                    .all(|(index, byte)| byte == b'-' || byte == b"rwx"[index % 3])
            {
                let mut mode = 0u32;
                for (index, byte) in value.bytes().enumerate() {
                    if byte != b'-' {
                        mode |= 1 << (8 - index);
                    }
                }
                return Ok(mode);
            }
            symbolic_mode(value.as_ref(), current)
        }
        value => Err(mlua::Error::runtime(format!(
            "file mode cannot be {}",
            value.type_name()
        ))),
    }
}

fn symbolic_mode(specification: &str, current: u32) -> mlua::Result<u32> {
    let mut mode = current & 0o7777;
    for clause in specification.split(',') {
        let operation = clause
            .find(['+', '-', '='])
            .ok_or_else(|| mlua::Error::runtime(format!("invalid mode clause '{clause}'")))?;
        let classes = &clause[..operation];
        let operator = clause.as_bytes()[operation];
        let permissions = &clause[operation + 1..];
        let classes = if classes.is_empty() { "a" } else { classes };
        let mut mask = 0u32;
        for class in classes.bytes() {
            let shift = match class {
                b'u' => 6,
                b'g' => 3,
                b'o' => 0,
                b'a' => {
                    for shift in [6, 3, 0] {
                        mask |= permission_bits(permissions, shift)?;
                    }
                    continue;
                }
                _ => {
                    return Err(mlua::Error::runtime(format!(
                        "invalid mode class '{}'",
                        char::from(class)
                    )));
                }
            };
            mask |= permission_bits(permissions, shift)?;
        }
        let class_mask = classes.bytes().fold(0u32, |mask, class| {
            mask | match class {
                b'u' => 0o700,
                b'g' => 0o070,
                b'o' => 0o007,
                b'a' => 0o777,
                _ => 0,
            }
        });
        mode = match operator {
            b'+' => mode | mask,
            b'-' => mode & !mask,
            b'=' => (mode & !class_mask) | mask,
            _ => unreachable!("operation finder restricts operator"),
        };
    }
    Ok(mode)
}

fn permission_bits(permissions: &str, shift: u32) -> mlua::Result<u32> {
    let mut bits = 0u32;
    for permission in permissions.bytes() {
        bits |= match permission {
            b'r' => 0o4 << shift,
            b'w' => 0o2 << shift,
            b'x' | b'X' => 0o1 << shift,
            _ => {
                return Err(mlua::Error::runtime(format!(
                    "invalid mode permission '{}'",
                    char::from(permission)
                )));
            }
        };
    }
    Ok(bits)
}

pub(super) fn target_identity(
    context: &LuaRuntimeContext,
    identity: Option<Value>,
    group: bool,
) -> mlua::Result<u32> {
    let Some(identity) = identity else {
        return Ok(u32::MAX);
    };
    match identity {
        Value::Nil => Ok(u32::MAX),
        Value::Integer(value) => {
            u32::try_from(value).map_err(|_| mlua::Error::runtime("identity is out of range"))
        }
        Value::String(value) => {
            let value = value.to_str()?;
            if let Ok(value) = value.parse::<u32>() {
                return Ok(value);
            }
            let file = if group { "/etc/group" } else { "/etc/passwd" };
            let content = String::from_utf8(context.read(file).map_err(mlua::Error::external)?)
                .map_err(mlua::Error::external)?;
            content
                .lines()
                .filter_map(|line| {
                    let fields = line.split(':').collect::<Vec<_>>();
                    (fields.len() >= 3 && fields[0] == value.as_ref())
                        .then(|| fields[2].parse::<u32>().ok())
                        .flatten()
                })
                .next()
                .ok_or_else(|| mlua::Error::runtime(format!("unknown target identity '{value}'")))
        }
        value => Err(mlua::Error::runtime(format!(
            "identity cannot be {}",
            value.type_name()
        ))),
    }
}

pub(super) fn pathconf_value(
    lua: &Lua,
    path: &std::path::Path,
    selector: Option<&str>,
) -> mlua::Result<Value> {
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| mlua::Error::runtime("path contains NUL"))?;
    let selectors = [
        ("link_max", libc::_PC_LINK_MAX),
        ("max_canon", libc::_PC_MAX_CANON),
        ("max_input", libc::_PC_MAX_INPUT),
        ("name_max", libc::_PC_NAME_MAX),
        ("path_max", libc::_PC_PATH_MAX),
        ("pipe_buf", libc::_PC_PIPE_BUF),
        ("chown_restricted", libc::_PC_CHOWN_RESTRICTED),
        ("no_trunc", libc::_PC_NO_TRUNC),
        ("vdisable", libc::_PC_VDISABLE),
    ];
    if let Some(selector) = selector {
        let key = selectors
            .iter()
            .find_map(|(name, key)| (*name == selector).then_some(*key))
            .ok_or_else(|| {
                mlua::Error::runtime(format!("unknown pathconf selector '{selector}'"))
            })?;
        return Ok(Value::Integer(unsafe {
            libc::pathconf(path.as_ptr(), key)
        }));
    }
    let table = lua.create_table()?;
    for (name, key) in selectors {
        table.set(name, unsafe { libc::pathconf(path.as_ptr(), key) })?;
    }
    Ok(Value::Table(table))
}

pub(super) fn sysconf_value(lua: &Lua, selector: Option<&str>) -> mlua::Result<Value> {
    let selectors = [
        ("arg_max", libc::_SC_ARG_MAX),
        ("child_max", libc::_SC_CHILD_MAX),
        ("clk_tck", libc::_SC_CLK_TCK),
        ("ngroups_max", libc::_SC_NGROUPS_MAX),
        ("stream_max", libc::_SC_STREAM_MAX),
        ("tzname_max", libc::_SC_TZNAME_MAX),
        ("open_max", libc::_SC_OPEN_MAX),
        ("job_control", libc::_SC_JOB_CONTROL),
        ("saved_ids", libc::_SC_SAVED_IDS),
        ("version", libc::_SC_VERSION),
    ];
    if let Some(selector) = selector {
        let key = selectors
            .iter()
            .find_map(|(name, key)| (*name == selector).then_some(*key))
            .ok_or_else(|| {
                mlua::Error::runtime(format!("unknown sysconf selector '{selector}'"))
            })?;
        return Ok(Value::Integer(unsafe { libc::sysconf(key) }));
    }
    let table = lua.create_table()?;
    for (name, key) in selectors {
        table.set(name, unsafe { libc::sysconf(key) })?;
    }
    Ok(Value::Table(table))
}
