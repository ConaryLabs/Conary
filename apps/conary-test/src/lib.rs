// conary-test/src/lib.rs

pub mod bootstrap;
pub mod build_info;
pub mod config;
pub mod container;
pub mod deploy;
pub mod engine;
pub mod error;
pub mod paths;
pub mod remi_client;
pub mod remi_stream;
pub mod report;
pub(crate) mod static_binary;
pub mod suite_inventory;
pub mod wal;

#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::{LazyLock, Mutex, MutexGuard};

    static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    pub fn lock_env() -> MutexGuard<'static, ()> {
        ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
