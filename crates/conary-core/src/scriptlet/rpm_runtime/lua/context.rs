// conary-core/src/scriptlet/rpm_runtime/lua/context.rs
//! Shared target-root, environment, and process state for RPM's Lua APIs.

use crate::scriptlet::ScriptletExecutor;
use anyhow::{Context, Result, bail};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

const DEFAULT_PATH: &str = "/usr/sbin:/usr/bin:/sbin:/bin";

#[derive(Clone)]
pub(super) struct LuaRuntimeContext {
    executor: ScriptletExecutor,
    cwd: Rc<RefCell<String>>,
    environment: Rc<RefCell<BTreeMap<String, String>>>,
    umask: Rc<Cell<u32>>,
}

impl LuaRuntimeContext {
    pub(super) fn new(executor: ScriptletExecutor) -> Self {
        Self::new_with_environment(executor, &[])
    }

    pub(super) fn new_with_environment(
        executor: ScriptletExecutor,
        environment: &[(String, String)],
    ) -> Self {
        let mut values = BTreeMap::from([
            ("HOME".to_string(), "/root".to_string()),
            ("TERM".to_string(), "dumb".to_string()),
            ("LANG".to_string(), "C.UTF-8".to_string()),
            ("SHELL".to_string(), "/bin/sh".to_string()),
            ("PATH".to_string(), DEFAULT_PATH.to_string()),
        ]);
        values.extend(environment.iter().cloned());
        Self {
            executor,
            cwd: Rc::new(RefCell::new("/".to_string())),
            environment: Rc::new(RefCell::new(values)),
            umask: Rc::new(Cell::new(0o022)),
        }
    }

    pub(super) fn executor(&self) -> &ScriptletExecutor {
        &self.executor
    }

    pub(super) fn root(&self) -> &Path {
        &self.executor.root
    }

    pub(super) fn cwd(&self) -> String {
        self.cwd.borrow().clone()
    }

    pub(super) fn chdir(&self, guest: &str) -> Result<()> {
        let cwd = self.cwd();
        let path = super::super::target::resolve_guest_path(self.root(), &cwd, guest)?;
        if !path.is_dir() {
            bail!(
                "RpmEmbeddedLuaPathError: '{}' is not a directory in the install target",
                guest
            );
        }
        *self.cwd.borrow_mut() = self.guest_path(guest)?;
        Ok(())
    }

    pub(super) fn resolve(&self, guest: &str) -> Result<PathBuf> {
        super::super::target::resolve_guest_path(self.root(), &self.cwd(), guest)
    }

    pub(super) fn guest_path(&self, guest: &str) -> Result<String> {
        super::super::target::guest_path(self.root(), &self.cwd(), guest)
    }

    pub(super) fn read(&self, guest: &str) -> Result<Vec<u8>> {
        let path = self.resolve(guest)?;
        std::fs::read(&path)
            .with_context(|| format!("read target file '{}' ({})", guest, path.display()))
    }

    pub(super) fn write(&self, guest: &str, bytes: &[u8], append: bool) -> Result<()> {
        use std::io::Write;
        let path = self.resolve(guest)?;
        let mut options = std::fs::OpenOptions::new();
        options
            .write(true)
            .create(true)
            .append(append)
            .truncate(!append);
        let mut file = options
            .open(&path)
            .with_context(|| format!("open target file '{}' ({})", guest, path.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("write target file '{}' ({})", guest, path.display()))
    }

    pub(super) fn getenv(&self, name: &str) -> Option<String> {
        self.environment.borrow().get(name).cloned()
    }

    pub(super) fn setenv(&self, name: &str, value: Option<String>) -> Result<()> {
        if name.is_empty() || name.contains('=') || name.contains('\0') {
            bail!("RpmEmbeddedLuaEnvironmentError: invalid environment name '{name}'");
        }
        let mut environment = self.environment.borrow_mut();
        if let Some(value) = value {
            if value.contains('\0') {
                bail!("RpmEmbeddedLuaEnvironmentError: value for '{name}' contains NUL");
            }
            environment.insert(name.to_string(), value);
        } else {
            environment.remove(name);
        }
        Ok(())
    }

    pub(super) fn command_environment(&self) -> Vec<(String, String)> {
        self.environment
            .borrow()
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect()
    }

    pub(super) fn umask(&self) -> u32 {
        self.umask.get()
    }

    pub(super) fn replace_umask(&self, value: u32) -> Result<u32> {
        if value > 0o777 {
            bail!("RpmEmbeddedLuaModeError: umask {value:o} is out of range");
        }
        Ok(self.umask.replace(value))
    }
}
