// apps/remi/src/server/conversion/benchmark/process.rs
//! Whole-process resource and worker-occupancy evidence.

use super::ConversionBenchmarkProcessUsage;
use anyhow::{Context, Result, anyhow, ensure};
use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

pub(super) struct ProcessUsageProbe {
    before: RawProcessUsage,
    sampler: ThreadSampler,
    started: Instant,
}

impl ProcessUsageProbe {
    pub(super) fn start() -> Result<Self> {
        Ok(Self {
            before: RawProcessUsage::capture()?,
            sampler: ThreadSampler::start()?,
            started: Instant::now(),
        })
    }

    pub(super) fn finish(self) -> Result<ConversionBenchmarkProcessUsage> {
        let occupancy = self.sampler.finish()?;
        RawProcessUsage::capture()?.delta(self.before, self.started.elapsed(), occupancy)
    }
}

#[derive(Clone, Copy)]
struct RawProcessUsage {
    user_cpu_us: u64,
    system_cpu_us: u64,
    peak_rss_bytes: u64,
    minor_faults: u64,
    major_faults: u64,
    block_input_operations: u64,
    block_output_operations: u64,
    voluntary_context_switches: u64,
    involuntary_context_switches: u64,
}

impl RawProcessUsage {
    fn capture() -> Result<Self> {
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
        // SAFETY: `usage` points to writable storage for `getrusage`.
        let status = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
        if status != 0 {
            return Err(std::io::Error::last_os_error()).context("capture process resource usage");
        }
        // SAFETY: successful `getrusage` initialized the structure.
        let usage = unsafe { usage.assume_init() };
        Ok(Self {
            user_cpu_us: timeval_us(usage.ru_utime)?,
            system_cpu_us: timeval_us(usage.ru_stime)?,
            peak_rss_bytes: nonnegative(usage.ru_maxrss, "peak RSS")?.saturating_mul(1024),
            minor_faults: nonnegative(usage.ru_minflt, "minor faults")?,
            major_faults: nonnegative(usage.ru_majflt, "major faults")?,
            block_input_operations: nonnegative(usage.ru_inblock, "block input operations")?,
            block_output_operations: nonnegative(usage.ru_oublock, "block output operations")?,
            voluntary_context_switches: nonnegative(usage.ru_nvcsw, "voluntary context switches")?,
            involuntary_context_switches: nonnegative(
                usage.ru_nivcsw,
                "involuntary context switches",
            )?,
        })
    }

    fn delta(
        self,
        before: Self,
        wall: Duration,
        occupancy: ThreadOccupancy,
    ) -> Result<ConversionBenchmarkProcessUsage> {
        Ok(ConversionBenchmarkProcessUsage {
            wall_time_us: u64::try_from(wall.as_micros())
                .context("benchmark wall time exceeds u64 microseconds")?,
            user_cpu_us: self.user_cpu_us.saturating_sub(before.user_cpu_us),
            system_cpu_us: self.system_cpu_us.saturating_sub(before.system_cpu_us),
            peak_rss_bytes: self.peak_rss_bytes,
            minor_faults: self.minor_faults.saturating_sub(before.minor_faults),
            major_faults: self.major_faults.saturating_sub(before.major_faults),
            block_input_operations: self
                .block_input_operations
                .saturating_sub(before.block_input_operations),
            block_output_operations: self
                .block_output_operations
                .saturating_sub(before.block_output_operations),
            voluntary_context_switches: self
                .voluntary_context_switches
                .saturating_sub(before.voluntary_context_switches),
            involuntary_context_switches: self
                .involuntary_context_switches
                .saturating_sub(before.involuntary_context_switches),
            maximum_thread_count: occupancy.maximum_thread_count,
            maximum_runnable_threads: occupancy.maximum_runnable_threads,
        })
    }
}

fn timeval_us(value: libc::timeval) -> Result<u64> {
    let seconds = nonnegative(value.tv_sec, "resource usage seconds")?;
    let micros = nonnegative(value.tv_usec, "resource usage microseconds")?;
    seconds
        .checked_mul(1_000_000)
        .and_then(|value| value.checked_add(micros))
        .context("resource usage time overflow")
}

fn nonnegative<T>(value: T, label: &str) -> Result<u64>
where
    i128: From<T>,
{
    let value = i128::from(value);
    ensure!(value >= 0, "{label} is negative");
    u64::try_from(value).with_context(|| format!("{label} exceeds u64"))
}

struct ThreadSampler {
    stop: Arc<AtomicBool>,
    maximum_thread_count: Arc<AtomicU64>,
    maximum_runnable_threads: Arc<AtomicU64>,
    handle: Option<thread::JoinHandle<Result<()>>>,
}

#[derive(Clone, Copy)]
struct ThreadOccupancy {
    maximum_thread_count: u64,
    maximum_runnable_threads: u64,
}

impl ThreadSampler {
    fn start() -> Result<Self> {
        let (initial_threads, initial_runnable) = process_thread_counts()?;
        let stop = Arc::new(AtomicBool::new(false));
        let maximum_thread_count = Arc::new(AtomicU64::new(initial_threads));
        let maximum_runnable_threads = Arc::new(AtomicU64::new(initial_runnable));
        let thread_stop = Arc::clone(&stop);
        let thread_count = Arc::clone(&maximum_thread_count);
        let runnable_count = Arc::clone(&maximum_runnable_threads);
        let handle = thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                let (threads, runnable) = process_thread_counts()?;
                // The sampler itself is part of `/proc/self/task` and is
                // removed from both reported occupancy floors.
                thread_count.fetch_max(threads.saturating_sub(1), Ordering::Relaxed);
                runnable_count.fetch_max(runnable.saturating_sub(1), Ordering::Relaxed);
                thread::sleep(Duration::from_millis(5));
            }
            Ok(())
        });
        Ok(Self {
            stop,
            maximum_thread_count,
            maximum_runnable_threads,
            handle: Some(handle),
        })
    }

    fn finish(mut self) -> Result<ThreadOccupancy> {
        self.stop.store(true, Ordering::Relaxed);
        self.handle
            .take()
            .context("thread sampler handle is absent")?
            .join()
            .map_err(|_| anyhow!("benchmark thread sampler panicked"))??;
        Ok(ThreadOccupancy {
            maximum_thread_count: self.maximum_thread_count.load(Ordering::Relaxed),
            maximum_runnable_threads: self.maximum_runnable_threads.load(Ordering::Relaxed),
        })
    }
}

fn process_thread_counts() -> Result<(u64, u64)> {
    let mut threads = 0_u64;
    let mut runnable = 0_u64;
    for entry in fs::read_dir("/proc/self/task")? {
        let entry = entry?;
        // Threads may exit between `read_dir` and reading their task state.
        // A vanished task is not a sampler failure; count only task records
        // that remained readable long enough to yield one coherent sample.
        let stat = match fs::read_to_string(entry.path().join("stat")) {
            Ok(stat) => stat,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        let state = stat
            .rfind(')')
            .and_then(|offset| stat.get(offset + 1..))
            .and_then(|rest| rest.split_whitespace().next())
            .context("parse /proc task state")?;
        threads = threads
            .checked_add(1)
            .context("process thread count overflow")?;
        if state == "R" {
            runnable = runnable
                .checked_add(1)
                .context("runnable process thread count overflow")?;
        }
    }
    Ok((threads, runnable))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_usage_probe_reports_thread_occupancy() {
        let probe = ProcessUsageProbe::start().unwrap();
        for _ in 0..128 {
            thread::spawn(|| std::hint::black_box((0_u64..10_000).sum::<u64>()))
                .join()
                .unwrap();
        }
        let usage = probe.finish().unwrap();
        assert!(usage.maximum_thread_count >= 1);
    }
}
