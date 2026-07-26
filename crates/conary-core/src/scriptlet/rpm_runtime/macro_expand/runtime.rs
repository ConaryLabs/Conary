// conary-core/src/scriptlet/rpm_runtime/macro_expand/runtime.rs
//! Target-root authority for RPM macro builtins with runtime effects.

use crate::ccs::native_lifecycle::RpmRuntimeMetadata;
use crate::scriptlet::ScriptletExecutor;
use anyhow::{Context, Result};
use std::collections::BTreeMap;

const DEFAULT_PATH: &str = "/usr/sbin:/usr/bin:/sbin:/bin";
const EMULATED_RPM_VERSION: &str = "6.1.90";

#[derive(Clone)]
pub(super) struct RpmMacroRuntime {
    executor: ScriptletExecutor,
    environment: BTreeMap<String, String>,
    rpm_version: String,
    verbose: bool,
}

impl RpmMacroRuntime {
    pub(super) fn new(
        executor: &ScriptletExecutor,
        environment: &[(String, String)],
        metadata: &RpmRuntimeMetadata,
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
            executor: executor.clone(),
            environment: values,
            rpm_version: EMULATED_RPM_VERSION.to_string(),
            verbose: metadata.header_context.format_environment.verbose,
        }
    }

    pub(super) fn shell_expand(&self, command: &str) -> Result<String> {
        let environment = self
            .environment
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect::<Vec<_>>();
        let output = self.executor.execute_native_command_capture(
            "rpm-macro-shell",
            &["/bin/sh".to_string(), "-c".to_string(), command.to_string()],
            &[],
            &environment,
        )?;
        let mut stdout = String::from_utf8(output.stdout)
            .context("RpmMacroShellOutputInvalidUtf8: shell expansion output")?;
        while stdout.ends_with('\r') || stdout.ends_with('\n') {
            stdout.pop();
        }
        Ok(stdout)
    }

    pub(super) fn environment(&self, name: &str) -> Option<&str> {
        self.environment.get(name).map(String::as_str)
    }

    pub(super) fn executor(&self) -> ScriptletExecutor {
        self.executor.clone()
    }

    pub(super) fn environment_values(&self) -> Vec<(String, String)> {
        self.environment
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect()
    }

    pub(super) fn rpm_version(&self) -> &str {
        &self.rpm_version
    }

    pub(super) fn verbose(&self) -> bool {
        self.verbose
    }

    pub(super) fn target_exists(&self, guest: &str) -> Result<bool> {
        Ok(super::super::target::resolve_guest_path(&self.executor.root, "/", guest)?.exists())
    }

    pub(super) fn read_target_file(&self, guest: &str) -> Result<String> {
        let path = super::super::target::resolve_guest_path(&self.executor.root, "/", guest)?;
        std::fs::read_to_string(&path)
            .with_context(|| format!("RpmMacroLoadFailed: {}", path.display()))
    }

    pub(super) fn available_parallelism(&self) -> usize {
        std::thread::available_parallelism().map_or(1, |parallelism| parallelism.get())
    }

    pub(super) fn total_memory_mib(&self) -> u64 {
        let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        let pages = unsafe { libc::sysconf(libc::_SC_PHYS_PAGES) };
        let page_size = if page_size > 0 {
            page_size as u64
        } else {
            4096
        };
        if pages <= 0 {
            return 0;
        }
        (pages as u64).saturating_mul(page_size) / (1024 * 1024) + 1
    }
}
