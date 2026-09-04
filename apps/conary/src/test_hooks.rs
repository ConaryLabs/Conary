// apps/conary/src/test_hooks.rs
//! Single typed owner for Conary CLI integration-test controls.

#[cfg(any(not(feature = "test-hooks"), test))]
use std::ffi::OsStr;
use std::ffi::OsString;
use std::path::PathBuf;

use thiserror::Error;

#[cfg(any(not(feature = "test-hooks"), test))]
const PREFIX: &str = "CONARY_TEST_";

#[cfg(any(feature = "test-hooks", test))]
// The disabled-feature unit-test build retains the complete name inventory so
// test helpers can share constants, even though only a subset has writers.
pub(crate) mod names {
    pub(crate) const BOOT_ID: &str = "CONARY_TEST_BOOT_ID";
    pub(crate) const DB: &str = "CONARY_TEST_DB";
    pub(crate) const FAIL_GENERATION_REBUILD: &str = "CONARY_TEST_FAIL_GENERATION_REBUILD";
    pub(crate) const FAIL_NATIVE_LIFECYCLE: &str = "CONARY_TEST_FAIL_NATIVE_LIFECYCLE";
    pub(crate) const HOLD_DURING_REMOVE_MS: &str = "CONARY_TEST_HOLD_DURING_REMOVE_MS";
    pub(crate) const HOLD_DURING_ROLLBACK_MS: &str = "CONARY_TEST_HOLD_DURING_ROLLBACK_MS";
    pub(crate) const KEY: &str = "CONARY_TEST_KEY";
    pub(crate) const KEYLESS: &str = "CONARY_TEST_KEYLESS";
    pub(crate) const NATIVE_HANDOFF_FAIL_AFTER: &str = "CONARY_TEST_NATIVE_HANDOFF_FAIL_AFTER";
    pub(crate) const PACKAGE: &str = "CONARY_TEST_PACKAGE";
    pub(crate) const PROC_CMDLINE_PATH: &str = "CONARY_TEST_PROC_CMDLINE_PATH";
    pub(crate) const SKIP_GENERATION_MOUNT: &str = "CONARY_TEST_SKIP_GENERATION_MOUNT";
    pub(crate) const TRY_KEEP_FAIL_AFTER_BACKUP: &str = "CONARY_TEST_TRY_KEEP_FAIL_AFTER_BACKUP";
    pub(crate) const TRY_LAUNCHER: &str = "CONARY_TEST_TRY_LAUNCHER";
    pub(crate) const TRY_MOUNTINFO_PATH: &str = "CONARY_TEST_TRY_MOUNTINFO_PATH";
    pub(crate) const TRY_REFRESH_FAIL_NAMESPACE_COMMIT_CLEANUP: &str =
        "CONARY_TEST_TRY_REFRESH_FAIL_NAMESPACE_COMMIT_CLEANUP";
    pub(crate) const TRY_REFRESH_FAIL_NAMESPACE_SWITCH: &str =
        "CONARY_TEST_TRY_REFRESH_FAIL_NAMESPACE_SWITCH";
    pub(crate) const TRY_REMOVE_DIR_FAIL: &str = "CONARY_TEST_TRY_REMOVE_DIR_FAIL";
    pub(crate) const TRY_RESTORE_DB_FAIL: &str = "CONARY_TEST_TRY_RESTORE_DB_FAIL";
    pub(crate) const TRY_SYNC_PARENT_LOG: &str = "CONARY_TEST_TRY_SYNC_PARENT_LOG";
    pub(crate) const TRY_UMOUNT_FAIL: &str = "CONARY_TEST_TRY_UMOUNT_FAIL";
    pub(crate) const TRY_UMOUNT_LOG: &str = "CONARY_TEST_TRY_UMOUNT_LOG";
    pub(crate) const TRY_WATCH_COOK_STARTED_FILE: &str = "CONARY_TEST_TRY_WATCH_COOK_STARTED_FILE";
    pub(crate) const TRY_WATCH_EXIT_AFTER_READY: &str = "CONARY_TEST_TRY_WATCH_EXIT_AFTER_READY";
    pub(crate) const TRY_WATCH_EXIT_AFTER_REFRESHES: &str =
        "CONARY_TEST_TRY_WATCH_EXIT_AFTER_REFRESHES";
    pub(crate) const TRY_WATCH_FAILURE_FILE: &str = "CONARY_TEST_TRY_WATCH_FAILURE_FILE";
    pub(crate) const TRY_WATCH_MARKER_FAIL: &str = "CONARY_TEST_TRY_WATCH_MARKER_FAIL";
    pub(crate) const TRY_WATCH_PAUSE_DURING_COOK: &str = "CONARY_TEST_TRY_WATCH_PAUSE_DURING_COOK";
    pub(crate) const TRY_WATCH_READY_FILE: &str = "CONARY_TEST_TRY_WATCH_READY_FILE";
}

#[cfg(test)]
const TEST_HOOK_NAMES: &[&str] = &[
    names::BOOT_ID,
    names::DB,
    names::FAIL_GENERATION_REBUILD,
    names::FAIL_NATIVE_LIFECYCLE,
    names::HOLD_DURING_REMOVE_MS,
    names::HOLD_DURING_ROLLBACK_MS,
    names::KEY,
    names::KEYLESS,
    names::NATIVE_HANDOFF_FAIL_AFTER,
    names::PACKAGE,
    names::PROC_CMDLINE_PATH,
    names::SKIP_GENERATION_MOUNT,
    names::TRY_KEEP_FAIL_AFTER_BACKUP,
    names::TRY_LAUNCHER,
    names::TRY_MOUNTINFO_PATH,
    names::TRY_REFRESH_FAIL_NAMESPACE_COMMIT_CLEANUP,
    names::TRY_REFRESH_FAIL_NAMESPACE_SWITCH,
    names::TRY_REMOVE_DIR_FAIL,
    names::TRY_RESTORE_DB_FAIL,
    names::TRY_SYNC_PARENT_LOG,
    names::TRY_UMOUNT_FAIL,
    names::TRY_UMOUNT_LOG,
    names::TRY_WATCH_COOK_STARTED_FILE,
    names::TRY_WATCH_EXIT_AFTER_READY,
    names::TRY_WATCH_EXIT_AFTER_REFRESHES,
    names::TRY_WATCH_FAILURE_FILE,
    names::TRY_WATCH_MARKER_FAIL,
    names::TRY_WATCH_PAUSE_DURING_COOK,
    names::TRY_WATCH_READY_FILE,
];

#[derive(Debug, Error)]
#[error("test-hook environment variables are disabled in this Conary build; unset: {variables}")]
pub(crate) struct TestHooksError {
    variables: String,
}

#[cfg(feature = "test-hooks")]
#[derive(Clone, Debug, Default)]
pub(crate) struct TestHooks {
    boot_id: Option<String>,
    db: Option<String>,
    fail_generation_rebuild: Option<OsString>,
    fail_native_lifecycle: Option<OsString>,
    hold_during_remove_ms: Option<u64>,
    hold_during_rollback_ms: Option<u64>,
    key: Option<String>,
    keyless: bool,
    native_handoff_fail_after: Option<String>,
    package: Option<String>,
    proc_cmdline_path: Option<PathBuf>,
    skip_generation_mount: bool,
    try_keep_fail_after_backup: Option<String>,
    try_launcher: Option<OsString>,
    try_mountinfo_path: Option<PathBuf>,
    try_refresh_fail_namespace_commit_cleanup: Option<OsString>,
    try_refresh_fail_namespace_switch: bool,
    try_remove_dir_fail: Option<PathBuf>,
    try_restore_db_fail: bool,
    try_sync_parent_log: Option<PathBuf>,
    try_umount_fail: Option<PathBuf>,
    try_umount_log: Option<PathBuf>,
    try_watch_cook_started_file: Option<PathBuf>,
    try_watch_exit_after_ready: bool,
    try_watch_exit_after_refreshes: Option<usize>,
    try_watch_failure_file: Option<PathBuf>,
    try_watch_marker_fail: bool,
    try_watch_pause_during_cook: bool,
    try_watch_ready_file: Option<PathBuf>,
}

#[cfg(not(feature = "test-hooks"))]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TestHooks;

#[cfg(all(feature = "test-hooks", not(test)))]
static TEST_HOOKS: std::sync::OnceLock<TestHooks> = std::sync::OnceLock::new();

pub(crate) fn initialize() -> Result<(), TestHooksError> {
    #[cfg(feature = "test-hooks")]
    {
        let hooks = TestHooks::capture();
        let _active_hook_count = hooks.active_count();
        #[cfg(not(test))]
        let _ = TEST_HOOKS.set(hooks);
        Ok(())
    }

    #[cfg(not(feature = "test-hooks"))]
    {
        debug_assert_eq!(TestHooks.active_count(), 0);
        let mut variables = std::env::vars_os()
            .map(|(name, _)| name)
            .filter(|name| key_name_is_test_hook(name))
            .map(|name| name.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        variables.sort();
        variables.dedup();
        if variables.is_empty() {
            Ok(())
        } else {
            Err(TestHooksError {
                variables: variables.join(", "),
            })
        }
    }
}

pub(crate) fn get() -> TestHooks {
    #[cfg(all(feature = "test-hooks", not(test)))]
    {
        TEST_HOOKS.get().cloned().unwrap_or_else(TestHooks::capture)
    }

    #[cfg(all(feature = "test-hooks", test))]
    {
        TestHooks::capture()
    }

    #[cfg(not(feature = "test-hooks"))]
    {
        TestHooks
    }
}

#[cfg(feature = "test-hooks")]
impl TestHooks {
    fn capture() -> Self {
        Self {
            boot_id: string(names::BOOT_ID),
            db: string(names::DB),
            fail_generation_rebuild: os_string(names::FAIL_GENERATION_REBUILD),
            fail_native_lifecycle: os_string(names::FAIL_NATIVE_LIFECYCLE),
            hold_during_remove_ms: unsigned(names::HOLD_DURING_REMOVE_MS),
            hold_during_rollback_ms: unsigned(names::HOLD_DURING_ROLLBACK_MS),
            key: string(names::KEY),
            keyless: present(names::KEYLESS),
            native_handoff_fail_after: string(names::NATIVE_HANDOFF_FAIL_AFTER),
            package: string(names::PACKAGE),
            proc_cmdline_path: path(names::PROC_CMDLINE_PATH),
            skip_generation_mount: present(names::SKIP_GENERATION_MOUNT),
            try_keep_fail_after_backup: string(names::TRY_KEEP_FAIL_AFTER_BACKUP),
            try_launcher: os_string(names::TRY_LAUNCHER),
            try_mountinfo_path: path(names::TRY_MOUNTINFO_PATH),
            try_refresh_fail_namespace_commit_cleanup: os_string(
                names::TRY_REFRESH_FAIL_NAMESPACE_COMMIT_CLEANUP,
            ),
            try_refresh_fail_namespace_switch: present(names::TRY_REFRESH_FAIL_NAMESPACE_SWITCH),
            try_remove_dir_fail: path(names::TRY_REMOVE_DIR_FAIL),
            try_restore_db_fail: equals_one(names::TRY_RESTORE_DB_FAIL),
            try_sync_parent_log: path(names::TRY_SYNC_PARENT_LOG),
            try_umount_fail: path(names::TRY_UMOUNT_FAIL),
            try_umount_log: path(names::TRY_UMOUNT_LOG),
            try_watch_cook_started_file: path(names::TRY_WATCH_COOK_STARTED_FILE),
            try_watch_exit_after_ready: present(names::TRY_WATCH_EXIT_AFTER_READY),
            try_watch_exit_after_refreshes: usize_value(names::TRY_WATCH_EXIT_AFTER_REFRESHES),
            try_watch_failure_file: path(names::TRY_WATCH_FAILURE_FILE),
            try_watch_marker_fail: present(names::TRY_WATCH_MARKER_FAIL),
            try_watch_pause_during_cook: present(names::TRY_WATCH_PAUSE_DURING_COOK),
            try_watch_ready_file: path(names::TRY_WATCH_READY_FILE),
        }
    }
}

#[cfg(feature = "test-hooks")]
fn string(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

#[cfg(feature = "test-hooks")]
fn os_string(name: &str) -> Option<OsString> {
    std::env::var_os(name)
}

#[cfg(feature = "test-hooks")]
fn path(name: &str) -> Option<PathBuf> {
    os_string(name).map(PathBuf::from)
}

#[cfg(feature = "test-hooks")]
fn present(name: &str) -> bool {
    os_string(name).is_some()
}

#[cfg(feature = "test-hooks")]
fn equals_one(name: &str) -> bool {
    std::env::var(name).as_deref() == Ok("1")
}

#[cfg(feature = "test-hooks")]
fn unsigned(name: &str) -> Option<u64> {
    string(name).and_then(|value| value.parse().ok())
}

#[cfg(feature = "test-hooks")]
fn usize_value(name: &str) -> Option<usize> {
    string(name).and_then(|value| value.parse().ok())
}

macro_rules! accessors {
    ($(($method:ident, $field:ident, $type:ty, $default:expr)),+ $(,)?) => {
        impl TestHooks {
            $(
                pub(crate) fn $method(&self) -> $type {
                    #[cfg(feature = "test-hooks")]
                    {
                        self.$field.clone()
                    }
                    #[cfg(not(feature = "test-hooks"))]
                    {
                        $default
                    }
                }
            )+
        }
    };
}

accessors!(
    (boot_id, boot_id, Option<String>, None),
    (db, db, Option<String>, None),
    (
        fail_generation_rebuild,
        fail_generation_rebuild,
        Option<OsString>,
        None
    ),
    (
        fail_native_lifecycle,
        fail_native_lifecycle,
        Option<OsString>,
        None
    ),
    (
        hold_during_remove_ms,
        hold_during_remove_ms,
        Option<u64>,
        None
    ),
    (
        hold_during_rollback_ms,
        hold_during_rollback_ms,
        Option<u64>,
        None
    ),
    (key, key, Option<String>, None),
    (keyless, keyless, bool, false),
    (
        native_handoff_fail_after,
        native_handoff_fail_after,
        Option<String>,
        None
    ),
    (package, package, Option<String>, None),
    (proc_cmdline_path, proc_cmdline_path, Option<PathBuf>, None),
    (skip_generation_mount, skip_generation_mount, bool, false),
    (
        try_keep_fail_after_backup,
        try_keep_fail_after_backup,
        Option<String>,
        None
    ),
    (try_launcher, try_launcher, Option<OsString>, None),
    (
        try_mountinfo_path,
        try_mountinfo_path,
        Option<PathBuf>,
        None
    ),
    (
        try_refresh_fail_namespace_commit_cleanup,
        try_refresh_fail_namespace_commit_cleanup,
        Option<OsString>,
        None
    ),
    (
        try_refresh_fail_namespace_switch,
        try_refresh_fail_namespace_switch,
        bool,
        false
    ),
    (
        try_remove_dir_fail,
        try_remove_dir_fail,
        Option<PathBuf>,
        None
    ),
    (try_restore_db_fail, try_restore_db_fail, bool, false),
    (
        try_sync_parent_log,
        try_sync_parent_log,
        Option<PathBuf>,
        None
    ),
    (try_umount_fail, try_umount_fail, Option<PathBuf>, None),
    (try_umount_log, try_umount_log, Option<PathBuf>, None),
    (
        try_watch_cook_started_file,
        try_watch_cook_started_file,
        Option<PathBuf>,
        None
    ),
    (
        try_watch_exit_after_ready,
        try_watch_exit_after_ready,
        bool,
        false
    ),
    (
        try_watch_exit_after_refreshes,
        try_watch_exit_after_refreshes,
        Option<usize>,
        None
    ),
    (
        try_watch_failure_file,
        try_watch_failure_file,
        Option<PathBuf>,
        None
    ),
    (try_watch_marker_fail, try_watch_marker_fail, bool, false),
    (
        try_watch_pause_during_cook,
        try_watch_pause_during_cook,
        bool,
        false
    ),
    (
        try_watch_ready_file,
        try_watch_ready_file,
        Option<PathBuf>,
        None
    ),
);

impl TestHooks {
    fn active_count(&self) -> usize {
        [
            self.boot_id().is_some(),
            self.db().is_some(),
            self.fail_generation_rebuild().is_some(),
            self.fail_native_lifecycle().is_some(),
            self.hold_during_remove_ms().is_some(),
            self.hold_during_rollback_ms().is_some(),
            self.key().is_some(),
            self.keyless(),
            self.native_handoff_fail_after().is_some(),
            self.package().is_some(),
            self.proc_cmdline_path().is_some(),
            self.skip_generation_mount(),
            self.try_keep_fail_after_backup().is_some(),
            self.try_launcher().is_some(),
            self.try_mountinfo_path().is_some(),
            self.try_refresh_fail_namespace_commit_cleanup().is_some(),
            self.try_refresh_fail_namespace_switch(),
            self.try_remove_dir_fail().is_some(),
            self.try_restore_db_fail(),
            self.try_sync_parent_log().is_some(),
            self.try_umount_fail().is_some(),
            self.try_umount_log().is_some(),
            self.try_watch_cook_started_file().is_some(),
            self.try_watch_exit_after_ready(),
            self.try_watch_exit_after_refreshes().is_some(),
            self.try_watch_failure_file().is_some(),
            self.try_watch_marker_fail(),
            self.try_watch_pause_during_cook(),
            self.try_watch_ready_file().is_some(),
        ]
        .into_iter()
        .filter(|active| *active)
        .count()
    }
}

#[cfg(any(not(feature = "test-hooks"), test))]
pub(crate) fn key_name_is_test_hook(name: &OsStr) -> bool {
    name.to_string_lossy().starts_with(PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_prefixed_environment_names() {
        for name in TEST_HOOK_NAMES {
            assert!(key_name_is_test_hook(OsStr::new(name)), "{name}");
        }
        assert!(!key_name_is_test_hook(OsStr::new("CONARY_DB")));
    }
}
