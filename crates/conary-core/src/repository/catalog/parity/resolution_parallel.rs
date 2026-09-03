// crates/conary-core/src/repository/catalog/parity/resolution_parallel.rs

//! Bounded worker admission and canonical ordered resolution dispatch.

#[cfg(test)]
use std::cell::Cell;
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::num::NonZeroUsize;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError, TrySendError};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use super::contract::NativeParityPackageV1;
use super::io::NativeParityOracleReader;
use crate::error::{Error, Result};

const CGROUP_ROOT: &str = "/sys/fs/cgroup";
const PROC_SELF_CGROUP: &str = "/proc/self/cgroup";
const PROC_MEMINFO: &str = "/proc/meminfo";

#[cfg(test)]
thread_local! {
    static TEST_CAPACITY: Cell<Option<usize>> = const { Cell::new(None) };
}

#[cfg(test)]
pub(crate) struct ResolutionTestCapacityGuard {
    previous: Option<usize>,
}

#[cfg(test)]
impl Drop for ResolutionTestCapacityGuard {
    fn drop(&mut self) {
        TEST_CAPACITY.set(self.previous);
    }
}

/// Override worker admission on the current test thread only.
#[cfg(test)]
pub(crate) fn resolution_test_capacity(capacity: usize) -> ResolutionTestCapacityGuard {
    assert!(capacity > 0, "test resolution capacity must be positive");
    let previous = TEST_CAPACITY.replace(Some(capacity));
    ResolutionTestCapacityGuard { previous }
}

pub(crate) const RESOLUTION_WALK_MEMORY_BUDGET_CEILING_BYTES: u64 = 8 * 1024 * 1024 * 1024;
// Rounded above the retained 1,271,280 KiB single-pool Fedora observation.
pub(crate) const RESOLUTION_WORKER_RSS_BYTES: u64 = 1536 * 1024 * 1024;

/// Validated operator request for resolution workers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolutionWorkerCount(NonZeroUsize);

impl ResolutionWorkerCount {
    pub fn new(value: usize) -> Result<Self> {
        NonZeroUsize::new(value).map(Self).ok_or_else(|| {
            Error::ConfigError("resolution worker count must be positive".to_string())
        })
    }

    #[must_use]
    pub const fn get(self) -> usize {
        self.0.get()
    }
}

impl FromStr for ResolutionWorkerCount {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        let parsed = value
            .parse::<usize>()
            .map_err(|error| format!("invalid resolution worker count '{value}': {error}"))?;
        Self::new(parsed).map_err(|error| error.to_string())
    }
}

/// Explicit operator choice or capacity-derived default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ResolutionWorkerRequest {
    #[default]
    Automatic,
    Explicit(ResolutionWorkerCount),
}

impl ResolutionWorkerRequest {
    #[must_use]
    pub const fn explicit(workers: ResolutionWorkerCount) -> Self {
        Self::Explicit(workers)
    }

    pub(crate) fn resolve(
        self,
        root_count: u64,
        memory_budget_bytes: u64,
        measured_worker_rss_bytes: u64,
    ) -> Result<ResolutionWorkerCount> {
        if measured_worker_rss_bytes == 0 {
            return Err(Error::InternalError(
                "resolution worker RSS authority must be positive".to_string(),
            ));
        }
        let roots = usize::try_from(root_count).unwrap_or(usize::MAX).max(1);
        #[cfg(test)]
        let test_capacity = TEST_CAPACITY.get();
        #[cfg(not(test))]
        let test_capacity: Option<usize> = None;
        let capacity = if let Some(capacity) = test_capacity {
            capacity.min(roots)
        } else {
            let cpu_limit = detected_cpu_limit()?;
            let memory_limit = memory_budget_bytes / measured_worker_rss_bytes;
            cpu_limit
                .min(usize::try_from(memory_limit).unwrap_or(usize::MAX))
                .min(roots)
        };
        let capacity = ResolutionWorkerCount::new(capacity).map_err(|_| {
            Error::ConfigError(format!(
                "resolution worker memory budget {memory_budget_bytes} is smaller than measured per-worker RSS {measured_worker_rss_bytes}"
            ))
        })?;
        match self {
            Self::Automatic => Ok(capacity),
            Self::Explicit(requested) if requested.get() <= capacity.get() => Ok(requested),
            Self::Explicit(requested) => Err(Error::ConfigError(format!(
                "requested {} resolution workers exceeds detected CPU/memory/root capacity {}",
                requested.get(),
                capacity.get()
            ))),
        }
    }
}

/// Non-authoritative execution evidence emitted separately from canonical
/// resolution bundles and surveys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolutionWalkImplementationEvidenceV1 {
    pub schema_version: u32,
    pub workers: u64,
    pub worker_load_milliseconds: Vec<u64>,
    pub memory_budget_bytes: u64,
    pub measured_worker_rss_bytes: u64,
}

impl ResolutionWalkImplementationEvidenceV1 {
    pub(crate) fn new(
        workers: ResolutionWorkerCount,
        worker_load_milliseconds: Vec<u64>,
        memory_budget_bytes: u64,
        measured_worker_rss_bytes: u64,
    ) -> Result<Self> {
        if worker_load_milliseconds.len() != workers.get() {
            return Err(Error::InternalError(
                "resolution worker load evidence count drifted".to_string(),
            ));
        }
        Ok(Self {
            schema_version: 1,
            workers: workers.get() as u64,
            worker_load_milliseconds,
            memory_budget_bytes,
            measured_worker_rss_bytes,
        })
    }
}

/// Write non-authoritative worker evidence without altering canonical artifacts.
pub fn write_resolution_walk_implementation_evidence(
    path: &Path,
    evidence: &ResolutionWalkImplementationEvidenceV1,
) -> Result<()> {
    let bytes = crate::json::canonical_json(evidence).map_err(|error| {
        Error::ParseError(format!("serialize resolution worker evidence: {error}"))
    })?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}

pub(crate) struct OrderedResolutionMetrics {
    pub(crate) worker_load_milliseconds: Vec<u64>,
}

enum WorkerMessage<R> {
    Ready {
        worker: usize,
        load_milliseconds: u64,
    },
    InitializationFailed {
        worker: usize,
        error: Error,
    },
    Root {
        sequence: u64,
        root: Box<NativeParityPackageV1>,
        result: Result<R>,
    },
}

/// Resolve roots concurrently while making the calling thread the sole,
/// canonical-order sink owner. Workers sample the sink's latest evidence
/// allowance immediately before each solve, so exhaustion stops later
/// explanation construction without making scheduling authoritative. A worker
/// panic becomes an ordered error result and the failed worker remains present
/// until the coordinator closes dispatch, preventing a missing-sequence wait.
pub(crate) fn walk_ordered_parallel<W, R>(
    package_oracle: &NativeParityOracleReader,
    workers: ResolutionWorkerCount,
    explanation_byte_limit: u64,
    initialize: impl Fn(usize) -> Result<W> + Sync,
    resolve: impl Fn(&mut W, &NativeParityPackageV1, u64) -> R + Sync,
    mut emit: impl FnMut(&NativeParityPackageV1, R) -> Result<u64>,
) -> Result<OrderedResolutionMetrics>
where
    R: Send,
{
    std::thread::scope(|scope| {
        let worker_count = workers.get();
        let channel_capacity = worker_count.checked_mul(2).ok_or_else(|| {
            Error::ConfigError("resolution worker channel capacity exceeds usize".to_string())
        })?;
        let max_in_flight = u64::try_from(channel_capacity).map_err(|_| {
            Error::ConfigError("resolution worker channel capacity exceeds u64".to_string())
        })?;
        let explanation_byte_limit = Arc::new(AtomicU64::new(explanation_byte_limit));
        let (result_sender, result_receiver) = mpsc::sync_channel(channel_capacity);
        let mut job_senders = Vec::with_capacity(worker_count);
        let mut handles = Vec::with_capacity(worker_count);
        for worker in 0..worker_count {
            let (job_sender, job_receiver) = mpsc::sync_channel(1);
            job_senders.push(job_sender);
            let result_sender = result_sender.clone();
            let initialize = &initialize;
            let resolve = &resolve;
            let explanation_byte_limit = Arc::clone(&explanation_byte_limit);
            handles.push(scope.spawn(move || {
                let started = Instant::now();
                let mut state = match catch_worker_panic(worker, None, || initialize(worker)) {
                    Ok(Ok(state)) => state,
                    Ok(Err(error)) => {
                        let _ = result_sender
                            .send(WorkerMessage::InitializationFailed { worker, error });
                        return;
                    }
                    Err(error) => {
                        let _ = result_sender
                            .send(WorkerMessage::InitializationFailed { worker, error });
                        return;
                    }
                };
                let load_milliseconds =
                    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
                if result_sender
                    .send(WorkerMessage::Ready {
                        worker,
                        load_milliseconds,
                    })
                    .is_err()
                {
                    return;
                }
                let mut terminal_failure: Option<String> = None;
                while let Ok((sequence, root)) = job_receiver.recv() {
                    let byte_limit = explanation_byte_limit.load(Ordering::Acquire);
                    let result = if let Some(message) = &terminal_failure {
                        Err(Error::InternalError(message.clone()))
                    } else {
                        match catch_worker_panic(worker, Some(sequence), || {
                            resolve(&mut state, &root, byte_limit)
                        }) {
                            Ok(result) => Ok(result),
                            Err(error) => {
                                terminal_failure = Some(error.to_string());
                                Err(error)
                            }
                        }
                    };
                    if result_sender
                        .send(WorkerMessage::Root {
                            sequence,
                            root: Box::new(root),
                            result,
                        })
                        .is_err()
                    {
                        return;
                    }
                }
            }));
        }
        drop(result_sender);

        let mut loads = vec![None; worker_count];
        let mut initialization_errors = BTreeMap::new();
        for _ in 0..worker_count {
            match result_receiver.recv() {
                Ok(WorkerMessage::Ready {
                    worker,
                    load_milliseconds,
                }) => loads[worker] = Some(load_milliseconds),
                Ok(WorkerMessage::InitializationFailed { worker, error }) => {
                    initialization_errors.insert(worker, error);
                }
                Ok(WorkerMessage::Root { .. }) => {
                    return Err(Error::InternalError(
                        "resolution worker produced a root before readiness".to_string(),
                    ));
                }
                Err(_) => {
                    return Err(Error::InternalError(
                        "resolution worker readiness channel closed".to_string(),
                    ));
                }
            }
        }
        if let Some((_, error)) = initialization_errors.into_iter().next() {
            drop(job_senders);
            join_workers(handles)?;
            return Err(error);
        }

        let mut pending = BTreeMap::new();
        let mut next_sequence = 0_u64;
        let mut dispatched = 0_u64;
        let mut next_worker = 0_usize;
        let walk = package_oracle.for_each_package(|root| {
            while dispatched.saturating_sub(next_sequence) >= max_in_flight {
                receive_and_emit(
                    &result_receiver,
                    &mut pending,
                    &mut next_sequence,
                    explanation_byte_limit.as_ref(),
                    &mut emit,
                )?;
            }
            let sequence = dispatched;
            let mut job = (sequence, root);
            loop {
                match job_senders[next_worker].try_send(job) {
                    Ok(()) => break,
                    Err(TrySendError::Full(returned)) => {
                        job = returned;
                        receive_and_emit(
                            &result_receiver,
                            &mut pending,
                            &mut next_sequence,
                            explanation_byte_limit.as_ref(),
                            &mut emit,
                        )?;
                    }
                    Err(TrySendError::Disconnected(_)) => {
                        return Err(Error::InternalError(format!(
                            "resolution worker {next_worker} stopped before accepting root {sequence}"
                        )));
                    }
                }
            }
            dispatched = dispatched.checked_add(1).ok_or_else(|| {
                Error::ConfigError("resolution root sequence exceeds u64".to_string())
            })?;
            next_worker = (next_worker + 1) % worker_count;
            drain_available(
                &result_receiver,
                &mut pending,
                &mut next_sequence,
                explanation_byte_limit.as_ref(),
                &mut emit,
            )
        });
        drop(job_senders);

        let result = match walk {
            Ok(()) => {
                while next_sequence < dispatched {
                    receive_and_emit(
                        &result_receiver,
                        &mut pending,
                        &mut next_sequence,
                        explanation_byte_limit.as_ref(),
                        &mut emit,
                    )?;
                }
                Ok(())
            }
            Err(error) => {
                while result_receiver.recv().is_ok() {}
                Err(error)
            }
        };
        join_workers(handles)?;
        result?;

        let worker_load_milliseconds = loads
            .into_iter()
            .enumerate()
            .map(|(worker, load)| {
                load.ok_or_else(|| {
                    Error::InternalError(format!(
                        "resolution worker {worker} omitted load evidence"
                    ))
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(OrderedResolutionMetrics {
            worker_load_milliseconds,
        })
    })
}

fn receive_and_emit<R>(
    receiver: &Receiver<WorkerMessage<R>>,
    pending: &mut BTreeMap<u64, (NativeParityPackageV1, Result<R>)>,
    next_sequence: &mut u64,
    explanation_byte_limit: &AtomicU64,
    emit: &mut impl FnMut(&NativeParityPackageV1, R) -> Result<u64>,
) -> Result<()> {
    let message = receiver
        .recv()
        .map_err(|_| Error::InternalError("resolution worker result channel closed".to_string()))?;
    retain_result(message, pending)?;
    emit_ready(pending, next_sequence, explanation_byte_limit, emit)
}

fn drain_available<R>(
    receiver: &Receiver<WorkerMessage<R>>,
    pending: &mut BTreeMap<u64, (NativeParityPackageV1, Result<R>)>,
    next_sequence: &mut u64,
    explanation_byte_limit: &AtomicU64,
    emit: &mut impl FnMut(&NativeParityPackageV1, R) -> Result<u64>,
) -> Result<()> {
    loop {
        match receiver.try_recv() {
            Ok(message) => retain_result(message, pending)?,
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => break,
        }
    }
    emit_ready(pending, next_sequence, explanation_byte_limit, emit)
}

fn retain_result<R>(
    message: WorkerMessage<R>,
    pending: &mut BTreeMap<u64, (NativeParityPackageV1, Result<R>)>,
) -> Result<()> {
    let WorkerMessage::Root {
        sequence,
        root,
        result,
    } = message
    else {
        return Err(Error::InternalError(
            "resolution worker repeated readiness".to_string(),
        ));
    };
    if pending.insert(sequence, (*root, result)).is_some() {
        return Err(Error::InternalError(format!(
            "resolution worker repeated root sequence {sequence}"
        )));
    }
    Ok(())
}

fn emit_ready<R>(
    pending: &mut BTreeMap<u64, (NativeParityPackageV1, Result<R>)>,
    next_sequence: &mut u64,
    explanation_byte_limit: &AtomicU64,
    emit: &mut impl FnMut(&NativeParityPackageV1, R) -> Result<u64>,
) -> Result<()> {
    while let Some((root, result)) = pending.remove(next_sequence) {
        explanation_byte_limit.store(emit(&root, result?)?, Ordering::Release);
        *next_sequence = next_sequence.checked_add(1).ok_or_else(|| {
            Error::ConfigError("resolution root sequence exceeds u64".to_string())
        })?;
    }
    Ok(())
}

fn catch_worker_panic<T>(
    worker: usize,
    sequence: Option<u64>,
    operation: impl FnOnce() -> T,
) -> Result<T> {
    catch_unwind(AssertUnwindSafe(operation)).map_err(|_| {
        let phase = sequence.map_or_else(
            || "during initialization".to_string(),
            |sequence| format!("while resolving root sequence {sequence}"),
        );
        Error::InternalError(format!("resolution worker {worker} panicked {phase}"))
    })
}

fn join_workers<T>(handles: Vec<std::thread::ScopedJoinHandle<'_, T>>) -> Result<()> {
    for (worker, handle) in handles.into_iter().enumerate() {
        if handle.join().is_err() {
            return Err(Error::InternalError(format!(
                "resolution worker {worker} panicked"
            )));
        }
    }
    Ok(())
}

fn detected_cpu_limit() -> Result<usize> {
    let available = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
    let Some(path) = unified_cgroup_path(Path::new(PROC_SELF_CGROUP))? else {
        return Ok(available);
    };
    let quota = cgroup_cpu_limit(Path::new(CGROUP_ROOT), &path)?;
    Ok(quota.map_or(available, |quota| available.min(quota.max(1))))
}

pub(crate) fn resolution_walk_memory_budget_bytes() -> Result<u64> {
    let detected = detected_memory_limit()?.unwrap_or(RESOLUTION_WALK_MEMORY_BUDGET_CEILING_BYTES);
    Ok(RESOLUTION_WALK_MEMORY_BUDGET_CEILING_BYTES.min(detected.saturating_mul(3) / 4))
}

fn detected_memory_limit() -> Result<Option<u64>> {
    let cgroup = unified_cgroup_path(Path::new(PROC_SELF_CGROUP))?
        .map(|path| cgroup_memory_limit(Path::new(CGROUP_ROOT), &path))
        .transpose()?
        .flatten();
    let host = host_memory_bytes(Path::new(PROC_MEMINFO))?;
    Ok(match (cgroup, host) {
        (Some(cgroup), Some(host)) => Some(cgroup.min(host)),
        (Some(cgroup), None) => Some(cgroup),
        (None, host) => host,
    })
}

fn unified_cgroup_path(proc_self_cgroup: &Path) -> Result<Option<PathBuf>> {
    let contents = match std::fs::read_to_string(proc_self_cgroup) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    for line in contents.lines() {
        if let Some(path) = line.strip_prefix("0::") {
            let path = path.strip_prefix('/').unwrap_or(path);
            return Ok(Some(PathBuf::from(path)));
        }
    }
    Ok(None)
}

fn cgroup_cpu_limit(root: &Path, relative: &Path) -> Result<Option<usize>> {
    let mut current = root.join(relative);
    let mut limit = None;
    loop {
        let path = current.join("cpu.max");
        match std::fs::read_to_string(&path) {
            Ok(contents) => {
                if let Some(value) = parse_cpu_max(contents.trim())? {
                    limit = Some(limit.map_or(value, |existing: usize| existing.min(value)));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        if current == root || !current.pop() || !current.starts_with(root) {
            break;
        }
    }
    Ok(limit)
}

fn cgroup_memory_limit(root: &Path, relative: &Path) -> Result<Option<u64>> {
    let mut current = root.join(relative);
    let mut limit = None;
    loop {
        let path = current.join("memory.max");
        match std::fs::read_to_string(&path) {
            Ok(contents) if contents.trim() == "max" => {}
            Ok(contents) => {
                let value = contents.trim().parse::<u64>().map_err(|error| {
                    Error::ConfigError(format!("parse cgroup memory.max: {error}"))
                })?;
                if value == 0 {
                    return Err(Error::ConfigError(
                        "cgroup memory.max must be positive or max".to_string(),
                    ));
                }
                limit = Some(limit.map_or(value, |existing: u64| existing.min(value)));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        if current == root || !current.pop() || !current.starts_with(root) {
            break;
        }
    }
    Ok(limit)
}

fn host_memory_bytes(meminfo: &Path) -> Result<Option<u64>> {
    let contents = match std::fs::read_to_string(meminfo) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let Some(line) = contents.lines().find(|line| line.starts_with("MemTotal:")) else {
        return Err(Error::ConfigError(
            "/proc/meminfo omits MemTotal".to_string(),
        ));
    };
    let mut fields = line.split_whitespace();
    let _label = fields.next();
    let kib = fields
        .next()
        .ok_or_else(|| Error::ConfigError("MemTotal omits its value".to_string()))?
        .parse::<u64>()
        .map_err(|error| Error::ConfigError(format!("parse MemTotal: {error}")))?;
    if fields.next() != Some("kB") || fields.next().is_some() {
        return Err(Error::ConfigError("MemTotal must use kB units".to_string()));
    }
    kib.checked_mul(1024)
        .map(Some)
        .ok_or_else(|| Error::ConfigError("MemTotal bytes exceed u64".to_string()))
}

fn parse_cpu_max(value: &str) -> Result<Option<usize>> {
    let mut fields = value.split_whitespace();
    let quota = fields
        .next()
        .ok_or_else(|| Error::ConfigError("cgroup cpu.max is missing its quota".to_string()))?;
    let period = fields
        .next()
        .ok_or_else(|| Error::ConfigError("cgroup cpu.max is missing its period".to_string()))?;
    if fields.next().is_some() {
        return Err(Error::ConfigError(
            "cgroup cpu.max has extra fields".to_string(),
        ));
    }
    if quota == "max" {
        return Ok(None);
    }
    let quota = quota
        .parse::<u64>()
        .map_err(|error| Error::ConfigError(format!("parse cgroup CPU quota: {error}")))?;
    let period = period
        .parse::<u64>()
        .map_err(|error| Error::ConfigError(format!("parse cgroup CPU period: {error}")))?;
    if quota == 0 || period == 0 {
        return Err(Error::ConfigError(
            "cgroup cpu.max quota and period must be positive".to_string(),
        ));
    }
    let whole_cpus = quota / period;
    Ok(Some(
        usize::try_from(whole_cpus.max(1)).unwrap_or(usize::MAX),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> NativeParityPackageV1 {
        serde_json::from_value(serde_json::json!({
            "package_key_sha256": "c".repeat(64),
            "member_ordinal": 0,
            "source_identity": "test-source",
            "repository_identity": "test-repository",
            "source_snapshot_sha256": "d".repeat(64),
            "source_profile": "test-profile",
            "name": "test-package",
            "version": "1",
            "package_release": "1",
            "architecture": "x86_64",
            "debian_multi_arch": null,
            "checksum": format!("sha256:{}", "e".repeat(64)),
            "size": 1,
            "download_url": "https://example.test/test-package.rpm",
            "version_scheme": "rpm",
            "provides": [],
            "requirement_groups": []
        }))
        .unwrap()
    }

    #[test]
    fn worker_count_rejects_zero() {
        assert!(ResolutionWorkerCount::new(0).is_err());
        assert!("0".parse::<ResolutionWorkerCount>().is_err());
        assert_eq!("4".parse::<ResolutionWorkerCount>().unwrap().get(), 4);
    }

    #[test]
    fn automatic_workers_obey_memory_and_root_capacity() {
        let workers = ResolutionWorkerRequest::Automatic
            .resolve(3, 8 * 1024, 1024)
            .unwrap();
        assert!(workers.get() <= 3);
        let error = ResolutionWorkerRequest::Explicit(ResolutionWorkerCount::new(4).unwrap())
            .resolve(3, 8 * 1024, 1024)
            .unwrap_err();
        assert!(error.to_string().contains("exceeds detected"));
    }

    #[test]
    fn parses_cgroup_v2_cpu_quota_conservatively() {
        assert_eq!(parse_cpu_max("max 100000").unwrap(), None);
        assert_eq!(parse_cpu_max("200000 100000").unwrap(), Some(2));
        assert_eq!(parse_cpu_max("150000 100000").unwrap(), Some(1));
        assert!(parse_cpu_max("100000 0").is_err());
        assert!(parse_cpu_max("rubbish").is_err());
    }

    #[test]
    fn reads_nested_unified_cgroup_path() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("cgroup");
        std::fs::write(&file, "0::/user.slice/test.scope\n").unwrap();
        assert_eq!(
            unified_cgroup_path(&file).unwrap(),
            Some(PathBuf::from("user.slice/test.scope"))
        );
    }

    #[test]
    fn takes_tightest_cgroup_cpu_ancestor() {
        let directory = tempfile::tempdir().unwrap();
        let child = directory.path().join("user.slice/test.scope");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::write(directory.path().join("cpu.max"), "8 1\n").unwrap();
        std::fs::write(directory.path().join("user.slice/cpu.max"), "3 1\n").unwrap();
        std::fs::write(child.join("cpu.max"), "max 1\n").unwrap();
        assert_eq!(
            cgroup_cpu_limit(directory.path(), Path::new("user.slice/test.scope")).unwrap(),
            Some(3)
        );
    }

    #[test]
    fn takes_tightest_cgroup_memory_ancestor() {
        let directory = tempfile::tempdir().unwrap();
        let child = directory.path().join("user.slice/test.scope");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::write(directory.path().join("memory.max"), "8589934592\n").unwrap();
        std::fs::write(
            directory.path().join("user.slice/memory.max"),
            "4294967296\n",
        )
        .unwrap();
        std::fs::write(child.join("memory.max"), "max\n").unwrap();
        assert_eq!(
            cgroup_memory_limit(directory.path(), Path::new("user.slice/test.scope")).unwrap(),
            Some(4 * 1024 * 1024 * 1024)
        );
    }

    #[test]
    fn parses_host_memory_kib() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("meminfo");
        std::fs::write(&file, "MemTotal:       16384 kB\nMemFree: 1 kB\n").unwrap();
        assert_eq!(host_memory_bytes(&file).unwrap(), Some(16 * 1024 * 1024));
    }

    #[test]
    fn implementation_evidence_is_canonical_and_create_only() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("implementation.json");
        let evidence = ResolutionWalkImplementationEvidenceV1::new(
            ResolutionWorkerCount::new(2).unwrap(),
            vec![11, 13],
            8 * 1024 * 1024 * 1024,
            1536 * 1024 * 1024,
        )
        .unwrap();
        write_resolution_walk_implementation_evidence(&path, &evidence).unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            crate::json::canonical_json(&evidence).unwrap()
        );
        assert!(write_resolution_walk_implementation_evidence(&path, &evidence).is_err());
    }

    #[test]
    fn ordered_emit_publishes_exhausted_evidence_budget_to_workers() {
        let mut pending = BTreeMap::from([(0, (root(), Ok(())))]);
        let mut next_sequence = 0;
        let explanation_byte_limit = AtomicU64::new(64);

        emit_ready(
            &mut pending,
            &mut next_sequence,
            &explanation_byte_limit,
            &mut |_, ()| Ok(0),
        )
        .unwrap();

        assert_eq!(next_sequence, 1);
        assert_eq!(explanation_byte_limit.load(Ordering::Acquire), 0);
    }

    #[test]
    fn worker_panics_become_sequence_bound_internal_errors() {
        let error = catch_worker_panic(2, Some(17), || -> () {
            panic!("private panic detail must not escape")
        })
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "Internal error: resolution worker 2 panicked while resolving root sequence 17"
        );
    }
}
