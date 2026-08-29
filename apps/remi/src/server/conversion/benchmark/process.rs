// apps/remi/src/server/conversion/benchmark/process.rs
//! Low-overhead whole-process resource evidence.

use super::ConversionBenchmarkProcessUsage;
use anyhow::{Context, Result, anyhow, ensure};
use std::fs;
use std::time::{Duration, Instant};

pub(super) struct ProcessUsageProbe {
    before: RawProcessUsage,
    started: Instant,
}

impl ProcessUsageProbe {
    pub(super) fn start() -> Result<Self> {
        Ok(Self {
            before: RawProcessUsage::capture()?,
            started: Instant::now(),
        })
    }

    /// Finish one explicitly bounded phase. A successor probe starts only
    /// after the caller has extracted and assembled the preceding phase's
    /// evidence, so that bookkeeping cannot bleed into the next phase.
    pub(super) fn finish(self) -> Result<ConversionBenchmarkProcessUsage> {
        let after = RawProcessUsage::capture()?;
        let elapsed = self.started.elapsed();
        after.delta(self.before, elapsed)
    }
}

#[derive(Clone, Copy)]
#[cfg_attr(test, derive(Default))]
struct RawProcessUsage {
    user_cpu_us: u64,
    system_cpu_us: u64,
    current_rss_bytes: u64,
    lifetime_peak_rss_bytes: u64,
    minor_faults: u64,
    major_faults: u64,
    block_input_operations: u64,
    block_output_operations: u64,
    voluntary_context_switches: u64,
    involuntary_context_switches: u64,
    thread_count: u64,
    runnable_threads: u64,
    io: ProcessIo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ProcessRss {
    current_bytes: u64,
    lifetime_peak_bytes: u64,
}

impl RawProcessUsage {
    fn capture() -> Result<Self> {
        let rss = ProcessRss::capture()?;
        let (thread_count, runnable_threads) = process_thread_counts()?;
        let io = ProcessIo::capture()?;
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
            current_rss_bytes: rss.current_bytes,
            lifetime_peak_rss_bytes: rss.lifetime_peak_bytes,
            minor_faults: nonnegative(usage.ru_minflt, "minor faults")?,
            major_faults: nonnegative(usage.ru_majflt, "major faults")?,
            block_input_operations: nonnegative(usage.ru_inblock, "block input operations")?,
            block_output_operations: nonnegative(usage.ru_oublock, "block output operations")?,
            voluntary_context_switches: nonnegative(usage.ru_nvcsw, "voluntary context switches")?,
            involuntary_context_switches: nonnegative(
                usage.ru_nivcsw,
                "involuntary context switches",
            )?,
            thread_count,
            runnable_threads,
            io,
        })
    }

    fn delta(self, before: Self, wall: Duration) -> Result<ConversionBenchmarkProcessUsage> {
        Ok(ConversionBenchmarkProcessUsage {
            wall_time_us: u64::try_from(wall.as_micros())
                .context("benchmark wall time exceeds u64 microseconds")?,
            user_cpu_us: checked_delta(self.user_cpu_us, before.user_cpu_us, "user CPU")?,
            system_cpu_us: checked_delta(self.system_cpu_us, before.system_cpu_us, "system CPU")?,
            rss_start_bytes: before.current_rss_bytes,
            rss_end_bytes: self.current_rss_bytes,
            // Each coherent status sample guarantees HWM >= RSS. Preserve the
            // greater HWM because Linux does not promise monotonicity between
            // separate status snapshots.
            process_lifetime_peak_rss_bytes: self
                .lifetime_peak_rss_bytes
                .max(before.lifetime_peak_rss_bytes),
            minor_faults: checked_delta(self.minor_faults, before.minor_faults, "minor faults")?,
            major_faults: checked_delta(self.major_faults, before.major_faults, "major faults")?,
            block_input_operations: checked_delta(
                self.block_input_operations,
                before.block_input_operations,
                "block input operations",
            )?,
            block_output_operations: checked_delta(
                self.block_output_operations,
                before.block_output_operations,
                "block output operations",
            )?,
            logical_read_bytes: checked_delta(
                self.io.logical_read_bytes,
                before.io.logical_read_bytes,
                "logical read bytes",
            )?,
            logical_write_bytes: checked_delta(
                self.io.logical_write_bytes,
                before.io.logical_write_bytes,
                "logical write bytes",
            )?,
            read_syscalls: checked_delta(
                self.io.read_syscalls,
                before.io.read_syscalls,
                "read syscalls",
            )?,
            write_syscalls: checked_delta(
                self.io.write_syscalls,
                before.io.write_syscalls,
                "write syscalls",
            )?,
            storage_read_bytes: checked_delta(
                self.io.storage_read_bytes,
                before.io.storage_read_bytes,
                "storage read bytes",
            )?,
            storage_write_bytes: checked_delta(
                self.io.storage_write_bytes,
                before.io.storage_write_bytes,
                "storage write bytes",
            )?,
            cancelled_write_bytes: checked_nonmonotonic_delta(
                self.io.cancelled_write_bytes,
                before.io.cancelled_write_bytes,
                "cancelled write bytes",
            )?,
            voluntary_context_switches: checked_delta(
                self.voluntary_context_switches,
                before.voluntary_context_switches,
                "voluntary context switches",
            )?,
            involuntary_context_switches: checked_delta(
                self.involuntary_context_switches,
                before.involuntary_context_switches,
                "involuntary context switches",
            )?,
            thread_count_start: before.thread_count,
            thread_count_end: self.thread_count,
            runnable_threads_start: before.runnable_threads,
            runnable_threads_end: self.runnable_threads,
        })
    }
}

impl ProcessRss {
    fn capture() -> Result<Self> {
        // Keep the endpoint and high-water mark in one status snapshot. Linux
        // exposes `getrusage().ru_maxrss` through different approximate RSS
        // accounting, so it can be lower than a separately sampled `VmRSS`.
        let contents = fs::read_to_string("/proc/self/status").context("read process RSS")?;
        Self::parse(&contents)
    }

    fn parse(contents: &str) -> Result<Self> {
        let current_bytes = parse_status_kib(contents, "VmRSS")?;
        let lifetime_peak_bytes = parse_status_kib(contents, "VmHWM")?;
        ensure!(
            lifetime_peak_bytes >= current_bytes,
            "/proc/self/status VmHWM is below VmRSS in one sample"
        );
        Ok(Self {
            current_bytes,
            lifetime_peak_bytes,
        })
    }
}

#[derive(Clone, Copy, Default)]
struct ProcessIo {
    logical_read_bytes: u64,
    logical_write_bytes: u64,
    read_syscalls: u64,
    write_syscalls: u64,
    storage_read_bytes: u64,
    storage_write_bytes: u64,
    cancelled_write_bytes: u64,
}

impl ProcessIo {
    fn capture() -> Result<Self> {
        let contents = fs::read_to_string("/proc/self/io").context("read process I/O counters")?;
        Self::parse(&contents)
    }

    fn parse(contents: &str) -> Result<Self> {
        let mut observed = Self::default();
        let mut seen = [false; 7];
        for line in contents.lines() {
            let Some((name, raw)) = line.split_once(':') else {
                return Err(anyhow!("malformed /proc/self/io line: {line}"));
            };
            let slot = match name {
                "rchar" => Some((0, &mut observed.logical_read_bytes)),
                "wchar" => Some((1, &mut observed.logical_write_bytes)),
                "syscr" => Some((2, &mut observed.read_syscalls)),
                "syscw" => Some((3, &mut observed.write_syscalls)),
                "read_bytes" => Some((4, &mut observed.storage_read_bytes)),
                "write_bytes" => Some((5, &mut observed.storage_write_bytes)),
                "cancelled_write_bytes" => Some((6, &mut observed.cancelled_write_bytes)),
                _ => None,
            };
            let Some((index, target)) = slot else {
                continue;
            };
            ensure!(!seen[index], "duplicate /proc/self/io counter '{name}'");
            *target = raw
                .trim()
                .parse::<u64>()
                .with_context(|| format!("parse /proc/self/io counter '{name}'"))?;
            seen[index] = true;
        }
        ensure!(
            seen.into_iter().all(std::convert::identity),
            "/proc/self/io omitted one or more required counters"
        );
        Ok(observed)
    }
}

fn checked_delta(after: u64, before: u64, label: &str) -> Result<u64> {
    after
        .checked_sub(before)
        .with_context(|| format!("process {label} counter regressed"))
}

fn checked_nonmonotonic_delta(after: u64, before: u64, label: &str) -> Result<i64> {
    i64::try_from(i128::from(after) - i128::from(before))
        .with_context(|| format!("process {label} delta exceeds i64"))
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

fn parse_status_kib(contents: &str, label: &str) -> Result<u64> {
    let mut values = contents
        .lines()
        .filter_map(|line| line.split_once(':'))
        .filter_map(|(name, value)| (name == label).then_some(value));
    let raw = values
        .next()
        .with_context(|| format!("/proc/self/status omitted {label}"))?;
    ensure!(
        values.next().is_none(),
        "/proc/self/status repeated {label}"
    );
    let mut fields = raw.split_whitespace();
    let kib = fields
        .next()
        .with_context(|| format!("{label} omitted its numeric value"))?
        .parse::<u64>()
        .with_context(|| format!("parse {label} numeric value"))?;
    ensure!(
        fields.next() == Some("kB"),
        "{label} has an unexpected unit"
    );
    ensure!(
        fields.next().is_none(),
        "{label} has unexpected trailing fields"
    );
    kib.checked_mul(1024)
        .with_context(|| format!("{label} exceeds u64 bytes"))
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
            Err(error) if task_record_vanished(&error) => continue,
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

fn task_record_vanished(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::NotFound || error.raw_os_error() == Some(libc::ESRCH)
}

#[cfg(test)]
mod tests {
    use super::*;

    const IO_SAMPLE: &str = "rchar: 11\n\
wchar: 12\n\
syscr: 13\n\
syscw: 14\n\
read_bytes: 15\n\
write_bytes: 16\n\
cancelled_write_bytes: 17\n";

    #[test]
    fn process_io_parses_cancelled_write_counter() {
        let io = ProcessIo::parse(IO_SAMPLE).expect("parse process I/O sample");

        assert_eq!(io.cancelled_write_bytes, 17);
    }

    #[test]
    fn cancelled_write_delta_may_regress() {
        assert_eq!(
            checked_nonmonotonic_delta(4096, 12_288, "cancelled write bytes")
                .expect("cancelled writes are a signed non-monotonic counter"),
            -8192
        );
    }

    #[test]
    fn signed_cancelled_write_delta_round_trips_through_report_json() {
        let expected = ConversionBenchmarkProcessUsage {
            cancelled_write_bytes: -8192,
            ..ConversionBenchmarkProcessUsage::default()
        };

        let encoded = serde_json::to_vec(&expected).expect("encode signed process evidence");
        let decoded: ConversionBenchmarkProcessUsage =
            serde_json::from_slice(&encoded).expect("decode signed process evidence");

        assert_eq!(decoded, expected);
    }

    #[test]
    fn vanished_task_errors_are_ignored_without_hiding_other_proc_failures() {
        assert!(task_record_vanished(&std::io::Error::from_raw_os_error(
            libc::ENOENT
        )));
        assert!(task_record_vanished(&std::io::Error::from_raw_os_error(
            libc::ESRCH
        )));
        assert!(!task_record_vanished(&std::io::Error::from_raw_os_error(
            libc::EACCES
        )));
    }

    #[test]
    fn process_rss_parses_one_coherent_status_sample() {
        let rss = ProcessRss::parse("Name:\tremi\nVmHWM:\t24 kB\nVmRSS:\t16 kB\n")
            .expect("parse process RSS sample");

        assert_eq!(
            rss,
            ProcessRss {
                current_bytes: 16 * 1024,
                lifetime_peak_bytes: 24 * 1024,
            }
        );
    }

    #[test]
    fn process_rss_rejects_incomplete_or_incoherent_status_samples() {
        for (sample, expected) in [
            ("VmHWM:\t24 kB\n", "omitted VmRSS"),
            ("VmRSS:\t16 kB\n", "omitted VmHWM"),
            (
                "VmHWM:\t24 kB\nVmRSS:\t16 kB\nVmRSS:\t16 kB\n",
                "repeated VmRSS",
            ),
            (
                "VmHWM:\t24 kB\nVmHWM:\t24 kB\nVmRSS:\t16 kB\n",
                "repeated VmHWM",
            ),
            (
                "VmHWM:\t24 kB\nVmRSS:\tnot-a-number kB\n",
                "parse VmRSS numeric value",
            ),
            (
                "VmHWM:\t24 KiB\nVmRSS:\t16 kB\n",
                "VmHWM has an unexpected unit",
            ),
            (
                "VmHWM:\t24 kB trailing\nVmRSS:\t16 kB\n",
                "VmHWM has unexpected trailing fields",
            ),
            ("VmHWM:\t8 kB\nVmRSS:\t16 kB\n", "VmHWM is below VmRSS"),
        ] {
            let error = ProcessRss::parse(sample).expect_err("reject invalid process RSS sample");
            assert!(
                error.to_string().contains(expected),
                "unexpected error for {sample:?}: {error:#}"
            );
        }
    }

    #[test]
    fn process_rss_rejects_byte_overflow() {
        let sample = format!("VmHWM:\t{} kB\nVmRSS:\t16 kB\n", u64::MAX);
        let error = ProcessRss::parse(&sample).expect_err("reject overflowing process RSS sample");

        assert!(error.to_string().contains("VmHWM exceeds u64 bytes"));
    }

    #[test]
    fn process_usage_delta_preserves_the_greatest_lifetime_peak_sample() {
        for (before_rss, before_peak, after_rss, after_peak, expected) in [
            (28 * 1024, 32 * 1024, 20 * 1024, 24 * 1024, 32 * 1024),
            (16 * 1024, 24 * 1024, 20 * 1024, 48 * 1024, 48 * 1024),
        ] {
            let before = RawProcessUsage {
                current_rss_bytes: before_rss,
                lifetime_peak_rss_bytes: before_peak,
                ..RawProcessUsage::default()
            };
            let after = RawProcessUsage {
                current_rss_bytes: after_rss,
                lifetime_peak_rss_bytes: after_peak,
                ..RawProcessUsage::default()
            };

            let usage = after
                .delta(before, Duration::ZERO)
                .expect("compute process usage delta");

            assert_eq!(usage.process_lifetime_peak_rss_bytes, expected);
            assert!(usage.process_lifetime_peak_rss_bytes >= usage.rss_start_bytes);
            assert!(usage.process_lifetime_peak_rss_bytes >= usage.rss_end_bytes);
        }
    }

    #[test]
    fn process_usage_probe_reports_strict_endpoint_evidence() {
        let probe = ProcessUsageProbe::start().unwrap();
        std::hint::black_box(fs::read("/proc/self/status").unwrap());
        let usage = probe.finish().unwrap();
        assert!(usage.rss_start_bytes > 0);
        assert!(usage.rss_end_bytes > 0);
        assert!(usage.process_lifetime_peak_rss_bytes >= usage.rss_start_bytes);
        assert!(usage.process_lifetime_peak_rss_bytes >= usage.rss_end_bytes);
        assert!(usage.thread_count_start >= 1);
        assert!(usage.thread_count_end >= 1);
        assert!(usage.logical_read_bytes > 0);
        assert!(usage.read_syscalls > 0);
    }
}
