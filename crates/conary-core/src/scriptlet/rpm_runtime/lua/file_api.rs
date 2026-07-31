// conary-core/src/scriptlet/rpm_runtime/lua/file_api.rs
//! Target-root file objects for RPM and standard Lua I/O.

use super::context::LuaRuntimeContext;
use flate2::read::GzDecoder;
use mlua::{
    AnyUserData, Lua, MetaMethod, MultiValue, Table, UserData, UserDataMethods, Value, Variadic,
};
use std::cell::RefCell;
use std::fs::{File, OpenOptions};
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::rc::Rc;

#[derive(Clone)]
pub(super) struct LuaFile {
    path: String,
    inner: Rc<RefCell<Option<FileBackend>>>,
    rpm_default_read_all: bool,
}

enum FileBackend {
    File(File),
    Memory(Cursor<Vec<u8>>),
}

impl LuaFile {
    fn open(
        context: &LuaRuntimeContext,
        guest: &str,
        mode: &str,
        rpm_default_read_all: bool,
    ) -> anyhow::Result<Self> {
        let path = context.resolve(guest)?;
        Self::open_resolved(&path, guest, mode, rpm_default_read_all, context.umask())
    }

    fn open_resolved(
        path: &Path,
        guest: &str,
        mode: &str,
        rpm_default_read_all: bool,
        umask: u32,
    ) -> anyhow::Result<Self> {
        let backend = if let Some(compression) = mode.split_once('.').map(|(_, value)| value) {
            if !mode.starts_with('r') {
                anyhow::bail!("compressed RPM file modes are read-only");
            }
            let bytes = std::fs::read(path)?;
            let bytes = match compression {
                "gzip" | "gz" => {
                    let mut decoded = Vec::new();
                    GzDecoder::new(bytes.as_slice()).read_to_end(&mut decoded)?;
                    decoded
                }
                "xz" | "lzma" => {
                    let mut decoded = Vec::new();
                    liblzma::read::XzDecoder::new(bytes.as_slice()).read_to_end(&mut decoded)?;
                    decoded
                }
                "zstd" | "zst" => zstd::stream::decode_all(bytes.as_slice())?,
                other => anyhow::bail!("unknown RPM file compression mode '{other}'"),
            };
            FileBackend::Memory(Cursor::new(bytes))
        } else {
            let mut options = OpenOptions::new();
            let read = mode.starts_with('r') || mode.contains('+');
            let write = mode.starts_with('w') || mode.starts_with('a') || mode.contains('+');
            options
                .read(read)
                .write(write)
                .create(mode.starts_with('w') || mode.starts_with('a'))
                .truncate(mode.starts_with('w'))
                .append(mode.starts_with('a'))
                .mode(0o666 & !umask);
            FileBackend::File(options.open(path)?)
        };
        Ok(Self {
            path: guest.to_string(),
            inner: Rc::new(RefCell::new(Some(backend))),
            rpm_default_read_all,
        })
    }

    fn memory(path: &str, bytes: Vec<u8>, rpm_default_read_all: bool) -> Self {
        Self {
            path: path.to_string(),
            inner: Rc::new(RefCell::new(Some(FileBackend::Memory(Cursor::new(bytes))))),
            rpm_default_read_all,
        }
    }

    fn close(&self) -> i32 {
        i32::from(self.inner.borrow_mut().take().is_none()) - 1
    }

    fn with_backend<T>(
        &self,
        function: impl FnOnce(&mut FileBackend) -> std::io::Result<T>,
    ) -> mlua::Result<T> {
        let mut inner = self.inner.borrow_mut();
        let backend = inner
            .as_mut()
            .ok_or_else(|| mlua::Error::runtime("attempt to use a closed file"))?;
        function(backend).map_err(mlua::Error::external)
    }

    fn read_value(&self, lua: &Lua, format: Value) -> mlua::Result<Value> {
        match format {
            Value::Nil if self.rpm_default_read_all => self.read_all(lua),
            Value::Nil => self.read_line(lua, false),
            Value::Integer(size) if size >= 0 => self.read_count(lua, size as usize),
            Value::String(format) => {
                let format = format.to_str()?;
                let format = format.as_ref();
                let selector = format
                    .strip_prefix('*')
                    .unwrap_or(format)
                    .as_bytes()
                    .first();
                match selector {
                    Some(b'a') => self.read_all(lua),
                    Some(b'l') => self.read_line(lua, false),
                    Some(b'L') => self.read_line(lua, true),
                    Some(b'n') => self.read_number(lua),
                    _ => Err(mlua::Error::runtime(format!(
                        "invalid file read format '{format}'"
                    ))),
                }
            }
            value => Err(mlua::Error::runtime(format!(
                "invalid file read argument {}",
                value.type_name()
            ))),
        }
    }

    fn read_all(&self, lua: &Lua) -> mlua::Result<Value> {
        let mut bytes = Vec::new();
        self.with_backend(|backend| backend.read_to_end(&mut bytes))?;
        Ok(Value::String(lua.create_string(bytes)?))
    }

    fn read_count(&self, lua: &Lua, size: usize) -> mlua::Result<Value> {
        let mut bytes = vec![0; size];
        let read = self.with_backend(|backend| backend.read(&mut bytes))?;
        bytes.truncate(read);
        if read == 0 && size != 0 {
            Ok(Value::Nil)
        } else {
            Ok(Value::String(lua.create_string(bytes)?))
        }
    }

    fn read_line(&self, lua: &Lua, keep_newline: bool) -> mlua::Result<Value> {
        let mut bytes = Vec::new();
        let read = self.with_backend(|backend| {
            let mut byte = [0u8; 1];
            loop {
                let count = backend.read(&mut byte)?;
                if count == 0 {
                    break;
                }
                bytes.push(byte[0]);
                if byte[0] == b'\n' {
                    break;
                }
            }
            Ok(bytes.len())
        })?;
        if read == 0 {
            return Ok(Value::Nil);
        }
        if !keep_newline {
            while matches!(bytes.last(), Some(b'\n' | b'\r')) {
                bytes.pop();
            }
        }
        Ok(Value::String(lua.create_string(bytes)?))
    }

    fn read_number(&self, lua: &Lua) -> mlua::Result<Value> {
        let mut token = Vec::new();
        self.with_backend(|backend| {
            let mut byte = [0u8; 1];
            loop {
                let position = backend.stream_position()?;
                if backend.read(&mut byte)? == 0 {
                    break;
                }
                if byte[0].is_ascii_whitespace() {
                    if token.is_empty() {
                        continue;
                    }
                    break;
                }
                if !(byte[0].is_ascii_digit() || b"+-.eExXabcdefABCDEF".contains(&byte[0])) {
                    backend.seek(SeekFrom::Start(position))?;
                    break;
                }
                token.push(byte[0]);
            }
            Ok(())
        })?;
        let token = String::from_utf8(token).map_err(mlua::Error::external)?;
        if let Ok(integer) = token.parse::<i64>() {
            Ok(Value::Integer(integer))
        } else if let Ok(number) = token.parse::<f64>() {
            Ok(Value::Number(number))
        } else {
            let _ = lua;
            Ok(Value::Nil)
        }
    }

    fn write_values(&self, lua: &Lua, values: Variadic<Value>) -> mlua::Result<usize> {
        let tostring: mlua::Function = lua.globals().get("tostring")?;
        let mut bytes = Vec::new();
        for value in values {
            bytes.extend_from_slice(tostring.call::<String>(value)?.as_bytes());
        }
        self.with_backend(|backend| backend.write_all(&bytes))?;
        Ok(bytes.len())
    }

    fn seek(&self, mode: Option<String>, offset: Option<i64>) -> mlua::Result<u64> {
        let offset = offset.unwrap_or(0);
        let position = match mode.as_deref().unwrap_or("cur") {
            "set" => SeekFrom::Start(
                u64::try_from(offset)
                    .map_err(|_| mlua::Error::runtime("negative set seek offset"))?,
            ),
            "cur" => SeekFrom::Current(offset),
            "end" => SeekFrom::End(offset),
            mode => return Err(mlua::Error::runtime(format!("invalid seek mode '{mode}'"))),
        };
        self.with_backend(|backend| backend.seek(position))
    }
}

pub(super) fn from_std_file(path: String, file: File) -> LuaFile {
    LuaFile {
        path,
        inner: Rc::new(RefCell::new(Some(FileBackend::File(file)))),
        rpm_default_read_all: false,
    }
}

impl Read for FileBackend {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::File(file) => file.read(buffer),
            Self::Memory(cursor) => cursor.read(buffer),
        }
    }
}

impl Write for FileBackend {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::File(file) => file.write(buffer),
            Self::Memory(cursor) => cursor.write(buffer),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::File(file) => file.flush(),
            Self::Memory(cursor) => cursor.flush(),
        }
    }
}

impl Seek for FileBackend {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        match self {
            Self::File(file) => file.seek(position),
            Self::Memory(cursor) => cursor.seek(position),
        }
    }
}

impl UserData for LuaFile {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("read", |lua, file, format: Option<Value>| {
            file.read_value(lua, format.unwrap_or(Value::Nil))
        });
        methods.add_method("write", |lua, file, values: Variadic<Value>| {
            file.write_values(lua, values)?;
            Ok(file.clone())
        });
        methods.add_method("flush", |_, file, ()| {
            file.with_backend(FileBackend::flush)?;
            Ok(true)
        });
        methods.add_method("close", |_, file, ()| Ok(file.close()));
        methods.add_method(
            "seek",
            |_, file, (mode, offset): (Option<String>, Option<i64>)| file.seek(mode, offset),
        );
        methods.add_method("lines", |lua, file, ()| {
            let mut lines = Vec::new();
            loop {
                match file.read_line(lua, false)? {
                    Value::String(line) => lines.push(line.to_str()?.to_string()),
                    Value::Nil => break,
                    _ => unreachable!("read_line returns string or nil"),
                }
            }
            let lines = Rc::new(RefCell::new(lines.into_iter()));
            lua.create_function(move |_, ()| Ok(lines.borrow_mut().next()))
        });
        methods.add_meta_method(MetaMethod::ToString, |_, file, ()| Ok(file.path.clone()));
    }
}

pub(super) fn install_io(lua: &Lua, context: LuaRuntimeContext, stdin: &[u8]) -> mlua::Result<()> {
    let table = lua.create_table()?;
    let input = Rc::new(RefCell::new(LuaFile::memory(
        "stdin",
        stdin.to_vec(),
        false,
    )));
    let output = Rc::new(RefCell::new(None::<LuaFile>));

    let open_context = context.clone();
    table.set(
        "open",
        lua.create_function(move |lua, (path, mode): (String, Option<String>)| {
            let mode = mode.unwrap_or_else(|| "r".to_string());
            if !valid_standard_io_mode(&mode) {
                return Err(mlua::Error::runtime(
                    "bad argument #2 to 'open' (invalid mode)",
                ));
            }
            let resolved = open_context.resolve(&path).map_err(mlua::Error::external)?;
            match LuaFile::open_resolved(&resolved, &path, &mode, false, open_context.umask()) {
                Ok(file) => Ok(MultiValue::from_vec(vec![Value::UserData(
                    lua.create_userdata(file)?,
                )])),
                Err(error) => match error.downcast_ref::<std::io::Error>() {
                    Some(io_error) => io_open_error(lua, &path, io_error),
                    None => Err(mlua::Error::external(error)),
                },
            }
        })?,
    )?;
    let lines_context = context.clone();
    let default_input = Rc::clone(&input);
    table.set(
        "lines",
        lua.create_function(move |lua, path: Option<String>| {
            let file = match path {
                Some(path) => LuaFile::open(&lines_context, &path, "r", false)
                    .map_err(mlua::Error::external)?,
                None => default_input.borrow().clone(),
            };
            let mut lines = Vec::new();
            loop {
                match file.read_line(lua, false)? {
                    Value::String(line) => lines.push(line.to_str()?.to_string()),
                    Value::Nil => break,
                    _ => unreachable!("read_line returns string or nil"),
                }
            }
            let lines = Rc::new(RefCell::new(lines.into_iter()));
            lua.create_function(move |_, ()| Ok(lines.borrow_mut().next()))
        })?,
    )?;
    let read_input = Rc::clone(&input);
    table.set(
        "read",
        lua.create_function(move |lua, formats: Variadic<Value>| {
            read_formats(lua, &read_input.borrow(), formats)
        })?,
    )?;
    let write_output = Rc::clone(&output);
    table.set(
        "write",
        lua.create_function(move |lua, values: Variadic<Value>| {
            if let Some(file) = write_output.borrow().as_ref() {
                file.write_values(lua, values)?;
            } else {
                let tostring: mlua::Function = lua.globals().get("tostring")?;
                let mut rendered = String::new();
                for value in values {
                    rendered.push_str(&tostring.call::<String>(value)?);
                }
                tracing::info!(target: "conary::rpm_lua", "{rendered}");
            }
            Ok(true)
        })?,
    )?;
    install_default_file_switches(lua, &table, context.clone(), input, output)?;
    let tmp_context = context;
    table.set(
        "tmpfile",
        lua.create_function(move |_, ()| {
            let directory = tmp_context.resolve("/tmp").map_err(mlua::Error::external)?;
            std::fs::create_dir_all(&directory).map_err(mlua::Error::external)?;
            let file = tempfile::tempfile_in(directory).map_err(mlua::Error::external)?;
            Ok(LuaFile {
                path: "<tmpfile>".to_string(),
                inner: Rc::new(RefCell::new(Some(FileBackend::File(file)))),
                rpm_default_read_all: false,
            })
        })?,
    )?;
    table.set(
        "type",
        lua.create_function(|_, value: Value| match value {
            Value::UserData(value) if value.is::<LuaFile>() => {
                let file = value.borrow::<LuaFile>()?;
                Ok(Some(if file.inner.borrow().is_some() {
                    "file"
                } else {
                    "closed file"
                }))
            }
            _ => Ok(None),
        })?,
    )?;
    table.set(
        "close",
        lua.create_function(|_, file: AnyUserData| Ok(file.borrow::<LuaFile>()?.close()))?,
    )?;
    table.set("flush", lua.create_function(|_, ()| Ok(true))?)?;
    lua.globals().set("io", table)
}

fn install_default_file_switches(
    lua: &Lua,
    table: &Table,
    context: LuaRuntimeContext,
    input: Rc<RefCell<LuaFile>>,
    output: Rc<RefCell<Option<LuaFile>>>,
) -> mlua::Result<()> {
    let input_context = context.clone();
    table.set(
        "input",
        lua.create_function(move |_, value: Option<Value>| {
            if let Some(value) = value {
                *input.borrow_mut() = file_argument(&input_context, value, "r")?;
            }
            Ok(input.borrow().clone())
        })?,
    )?;
    table.set(
        "output",
        lua.create_function(move |_, value: Option<Value>| {
            if let Some(value) = value {
                *output.borrow_mut() = Some(file_argument(&context, value, "w")?);
            }
            Ok(output.borrow().clone())
        })?,
    )
}

fn file_argument(context: &LuaRuntimeContext, value: Value, mode: &str) -> mlua::Result<LuaFile> {
    match value {
        Value::String(path) => LuaFile::open(context, path.to_str()?.as_ref(), mode, false)
            .map_err(mlua::Error::external),
        Value::UserData(file) => Ok(file.borrow::<LuaFile>()?.clone()),
        value => Err(mlua::Error::runtime(format!(
            "expected filename or file, got {}",
            value.type_name()
        ))),
    }
}

fn read_formats(lua: &Lua, file: &LuaFile, formats: Variadic<Value>) -> mlua::Result<MultiValue> {
    let formats = if formats.is_empty() {
        vec![Value::Nil]
    } else {
        formats.into_iter().collect()
    };
    let mut values = Vec::with_capacity(formats.len());
    for format in formats {
        let value = file.read_value(lua, format)?;
        let eof = matches!(value, Value::Nil);
        values.push(value);
        if eof {
            break;
        }
    }
    Ok(MultiValue::from_vec(values))
}

fn valid_standard_io_mode(mode: &str) -> bool {
    let mut bytes = mode.bytes().peekable();
    if !matches!(bytes.next(), Some(b'r' | b'w' | b'a')) {
        return false;
    }
    if bytes.peek() == Some(&b'+') {
        bytes.next();
    }
    bytes.all(|byte| byte == b'b')
}

fn io_open_error(lua: &Lua, path: &str, error: &std::io::Error) -> mlua::Result<MultiValue> {
    let errno = error.raw_os_error().unwrap_or(0);
    let description = if errno == 0 {
        error.to_string()
    } else {
        nix::errno::Errno::from_raw(errno).desc().to_string()
    };
    Ok(MultiValue::from_vec(vec![
        Value::Nil,
        Value::String(lua.create_string(format!("{path}: {description}"))?),
        Value::Integer(i64::from(errno)),
    ]))
}

pub(super) fn install_rpm_open(
    lua: &Lua,
    table: &Table,
    context: LuaRuntimeContext,
) -> mlua::Result<()> {
    table.set(
        "open",
        lua.create_function(move |_, (path, mode): (String, Option<String>)| {
            LuaFile::open(&context, &path, mode.as_deref().unwrap_or("r"), true)
                .map_err(mlua::Error::external)
        })?,
    )
}
