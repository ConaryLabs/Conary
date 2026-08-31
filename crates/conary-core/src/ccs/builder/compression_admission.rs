// crates/conary-core/src/ccs/builder/compression_admission.rs
//! Work-conserving CPU admission for final CCS archive compression.

use super::package_writer::CcsArchiveCompression;
use anyhow::{Context, Result, ensure};
use std::sync::{Arc, Condvar, Mutex};

#[derive(Debug)]
struct AdmissionState {
    available_workers: usize,
}

#[derive(Debug)]
struct AdmissionInner {
    capacity: usize,
    state: Mutex<AdmissionState>,
    available: Condvar,
}

/// Shared authority for the aggregate number of CCS compression workers.
///
/// An archive leases all workers that are idle when it reaches final emission.
/// That keeps the authority work-conserving for a lone conversion and queues a
/// later archive rather than oversubscribing the process. The lease is released
/// as soon as the archive writer returns.
#[derive(Clone, Debug)]
pub struct CcsArchiveCompressionAdmission {
    inner: Arc<AdmissionInner>,
}

impl CcsArchiveCompressionAdmission {
    /// Construct an exact checked aggregate worker capacity.
    pub fn with_capacity(capacity: usize) -> Result<Self> {
        CcsArchiveCompression::with_workers(capacity)?;
        Ok(Self {
            inner: Arc::new(AdmissionInner {
                capacity,
                state: Mutex::new(AdmissionState {
                    available_workers: capacity,
                }),
                available: Condvar::new(),
            }),
        })
    }

    /// Construct the authority from the host or cgroup logical CPU allowance.
    pub fn for_host_parallelism(logical_parallelism: usize) -> Result<Self> {
        ensure!(
            logical_parallelism > 0,
            "logical parallelism must be greater than zero"
        );
        Self::with_capacity(
            logical_parallelism.min(crate::ccs::CCS_BUDGET.max_archive_compression_workers),
        )
    }

    /// Aggregate compression-worker capacity shared by every clone.
    pub fn capacity(&self) -> usize {
        self.inner.capacity
    }

    /// Lease all currently idle workers, waiting when another archive owns the
    /// complete capacity.
    pub fn acquire(&self) -> Result<CcsArchiveCompressionLease> {
        let mut state =
            self.inner.state.lock().map_err(|_| {
                anyhow::anyhow!("CCS archive compression admission lock is poisoned")
            })?;
        while state.available_workers == 0 {
            state = self.inner.available.wait(state).map_err(|_| {
                anyhow::anyhow!("CCS archive compression admission lock is poisoned")
            })?;
        }

        let workers = state.available_workers;
        let compression = CcsArchiveCompression::with_workers(workers)
            .context("admit live CCS archive compression worker lease")?;
        state.available_workers = 0;
        drop(state);

        Ok(CcsArchiveCompressionLease {
            compression,
            workers,
            inner: Arc::clone(&self.inner),
        })
    }
}

impl Default for CcsArchiveCompressionAdmission {
    fn default() -> Self {
        Self::with_capacity(1).expect("one CCS compression worker is always admissible")
    }
}

/// RAII lease retaining one archive's exact checked compression geometry.
#[derive(Debug)]
pub struct CcsArchiveCompressionLease {
    compression: CcsArchiveCompression,
    workers: usize,
    inner: Arc<AdmissionInner>,
}

impl CcsArchiveCompressionLease {
    /// Exact compression geometry authorized for this archive emission.
    pub fn compression(&self) -> CcsArchiveCompression {
        self.compression
    }
}

impl Drop for CcsArchiveCompressionLease {
    fn drop(&mut self) {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.available_workers = state
            .available_workers
            .checked_add(self.workers)
            .expect("CCS compression worker lease release cannot overflow");
        assert!(
            state.available_workers <= self.inner.capacity,
            "CCS compression worker lease released more than the authority capacity"
        );
        self.inner.available.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn lone_archive_saturates_live_capacity_and_release_restores_it() {
        let admission = CcsArchiveCompressionAdmission::with_capacity(4).unwrap();
        let first = admission.acquire().unwrap();
        assert_eq!(first.compression().workers(), 4);
        drop(first);

        let second = admission.acquire().unwrap();
        assert_eq!(second.compression().workers(), 4);
    }

    #[test]
    fn concurrent_archive_waits_instead_of_oversubscribing_capacity() {
        let admission = CcsArchiveCompressionAdmission::with_capacity(4).unwrap();
        let first = admission.acquire().unwrap();
        let waiting_admission = admission.clone();
        let (ready_sender, ready_receiver) = mpsc::channel();
        let (done_sender, done_receiver) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            ready_sender.send(()).unwrap();
            let lease = waiting_admission.acquire().unwrap();
            done_sender.send(lease.compression().workers()).unwrap();
        });

        ready_receiver.recv().unwrap();
        assert!(done_receiver.try_recv().is_err());
        drop(first);
        assert_eq!(
            done_receiver.recv_timeout(Duration::from_secs(5)).unwrap(),
            4
        );
        waiter.join().unwrap();
    }

    #[test]
    fn exact_capacity_rejects_zero_and_over_budget_values() {
        assert!(CcsArchiveCompressionAdmission::with_capacity(0).is_err());
        assert!(
            CcsArchiveCompressionAdmission::with_capacity(
                crate::ccs::CCS_BUDGET.max_archive_compression_workers + 1,
            )
            .is_err()
        );
    }

    #[test]
    fn host_parallelism_is_capped_by_the_canonical_budget() {
        let admission = CcsArchiveCompressionAdmission::for_host_parallelism(usize::MAX).unwrap();
        assert_eq!(
            admission.capacity(),
            crate::ccs::CCS_BUDGET.max_archive_compression_workers
        );
    }
}
