# Remi Conversion Benchmark And Corpus Scan Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add non-mutating Remi conversion timing and scriptlet corpus-scan evidence so cold-path latency and adapter bootstrap needs are measured before schema or replay work.

**Architecture:** Add small, focused Remi modules for timing reports and scriptlet corpus summaries. Instrument the existing conversion service without changing conversion behavior, then add a CLI command that runs bounded benchmarks and emits JSON evidence for later adapter planning.

**Tech Stack:** Rust, Tokio, Serde, SQLite repository metadata, existing Remi conversion service, existing Cargo test gates.

---

## `/goal` Objective

Use this exact objective when starting execution:

```text
/goal Implement Remi conversion benchmark and corpus scan from docs/superpowers/plans/2026-05-27-remi-conversion-benchmark-corpus-plan.md. Stop when the benchmark command emits JSON evidence, targeted Remi tests pass, and docs record how to run the baseline workflow.
```

## File Structure

- Create `apps/remi/src/server/conversion_timing.rs`
  - Defines conversion phase names, per-phase durations, and JSON/log output.
- Create `apps/remi/src/server/scriptlet_corpus.rs`
  - Summarizes scriptlet counts, helper command frequencies, blocked-class hints, and package-level decision estimates for evidence only.
- Modify `apps/remi/src/server/mod.rs`
  - Exposes the new modules where the CLI and tests need them.
- Modify `apps/remi/src/server/conversion.rs`
  - Records timing for the existing conversion path without changing behavior.
- Modify `apps/remi/src/bin/remi.rs`
  - Adds `remi conversion-benchmark` with named-package and bounded-sample modes.
- Modify `docs/modules/remi.md`
  - Documents how to run the benchmark and what the JSON evidence means.

## Task 1: Add Conversion Timing Types

**Files:**

- Create: `apps/remi/src/server/conversion_timing.rs`
- Modify: `apps/remi/src/server/mod.rs`

- [ ] **Step 1: Write the failing timing serialization test**

Add this test to the bottom of `apps/remi/src/server/conversion_timing.rs` after creating the file:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    #[test]
    fn timing_report_serializes_phase_durations() {
        let mut report = ConversionTimingReport::new("fedora", "nginx", Some("1.28.0"));
        report.record(ConversionPhase::PackageLookup, Duration::from_millis(11));
        report.record(ConversionPhase::Download, Duration::from_millis(22));
        report.finish(true);

        let value = serde_json::to_value(&report).expect("timing report serializes");
        assert_eq!(value["distro"], json!("fedora"));
        assert_eq!(value["package"], json!("nginx"));
        assert_eq!(value["version"], json!("1.28.0"));
        assert_eq!(value["success"], json!(true));
        assert_eq!(value["phases"][0]["phase"], json!("package_lookup"));
        assert_eq!(value["phases"][0]["duration_ms"], json!(11));
        assert_eq!(value["phases"][1]["phase"], json!("download"));
        assert_eq!(value["phases"][1]["duration_ms"], json!(22));
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run:

```bash
cargo test -p remi conversion_timing::tests::timing_report_serializes_phase_durations
```

Expected: compile failure because `ConversionTimingReport` and `ConversionPhase` do not exist yet.

- [ ] **Step 3: Implement timing types**

Create `apps/remi/src/server/conversion_timing.rs` with this implementation above the test module from Step 1:

```rust
// apps/remi/src/server/conversion_timing.rs
//! Timing evidence for Remi package conversion.

use serde::Serialize;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversionPhase {
    PackageLookup,
    Download,
    Checksum,
    CacheLookup,
    Parse,
    LegacyConversion,
    ChunkStorage,
    Persistence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConversionPhaseTiming {
    pub phase: ConversionPhase,
    pub duration_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConversionTimingReport {
    pub distro: String,
    pub package: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub phases: Vec<ConversionPhaseTiming>,
    pub total_ms: u128,
    pub success: bool,
    #[serde(skip)]
    started_at: Instant,
}

impl ConversionTimingReport {
    pub fn new(distro: &str, package: &str, version: Option<&str>) -> Self {
        Self {
            distro: distro.to_string(),
            package: package.to_string(),
            version: version.map(ToString::to_string),
            phases: Vec::new(),
            total_ms: 0,
            success: false,
            started_at: Instant::now(),
        }
    }

    pub fn record(&mut self, phase: ConversionPhase, duration: Duration) {
        self.phases.push(ConversionPhaseTiming {
            phase,
            duration_ms: duration.as_millis(),
        });
    }

    pub fn finish(&mut self, success: bool) {
        self.success = success;
        self.total_ms = self.started_at.elapsed().as_millis();
    }

    pub fn time<T, E>(
        &mut self,
        phase: ConversionPhase,
        f: impl FnOnce() -> Result<T, E>,
    ) -> Result<T, E> {
        let started = Instant::now();
        let result = f();
        self.record(phase, started.elapsed());
        result
    }
}
```

- [ ] **Step 4: Expose the module**

In `apps/remi/src/server/mod.rs`, add:

```rust
pub mod conversion_timing;
```

near the other `pub mod` declarations.

- [ ] **Step 5: Run the test to verify it passes**

Run:

```bash
cargo test -p remi conversion_timing::tests::timing_report_serializes_phase_durations
```

Expected: pass.

- [ ] **Step 6: Commit Task 1**

```bash
git add apps/remi/src/server/conversion_timing.rs apps/remi/src/server/mod.rs
git commit -m "feat(remi): add conversion timing report types"
```

## Task 2: Add Scriptlet Corpus Summary Types

**Files:**

- Create: `apps/remi/src/server/scriptlet_corpus.rs`
- Modify: `apps/remi/src/server/mod.rs`

- [ ] **Step 1: Write failing corpus tests**

Create `apps/remi/src/server/scriptlet_corpus.rs` with the module header and these tests:

```rust
// apps/remi/src/server/scriptlet_corpus.rs
//! Evidence-only scriptlet corpus summaries for adapter planning.

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::packages::traits::{Scriptlet, ScriptletPhase};

    fn scriptlet(content: &str) -> Scriptlet {
        Scriptlet {
            phase: ScriptletPhase::PostInstall,
            interpreter: "/bin/sh".to_string(),
            content: content.to_string(),
            flags: None,
        }
    }

    #[test]
    fn corpus_summary_counts_helper_commands() {
        let summary = ScriptletCorpusSummary::from_scriptlets(
            "fedora",
            "nginx",
            &[scriptlet("systemctl daemon-reload\nldconfig\n")],
        );

        assert_eq!(summary.package, "nginx");
        assert_eq!(summary.scriptlet_count, 1);
        assert_eq!(summary.command_counts.get("systemctl"), Some(&1));
        assert_eq!(summary.command_counts.get("ldconfig"), Some(&1));
        assert!(summary.blocked_class_hints.is_empty());
    }

    #[test]
    fn corpus_summary_marks_package_manager_recursion() {
        let summary = ScriptletCorpusSummary::from_scriptlets(
            "arch",
            "bad-news",
            &[scriptlet("pacman -Syu\ncurl https://example.invalid/script.sh\n")],
        );

        assert!(summary.blocked_class_hints.contains(&"package-manager-recursion".to_string()));
        assert!(summary.blocked_class_hints.contains(&"network".to_string()));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p remi scriptlet_corpus::tests
```

Expected: compile failure because `ScriptletCorpusSummary` does not exist yet.

- [ ] **Step 3: Implement corpus summary**

Replace the initial module body in `apps/remi/src/server/scriptlet_corpus.rs` with this implementation above the test module from Step 1:

```rust
// apps/remi/src/server/scriptlet_corpus.rs
//! Evidence-only scriptlet corpus summaries for adapter planning.

use conary_core::packages::traits::Scriptlet;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize)]
pub struct ScriptletCorpusSummary {
    pub distro: String,
    pub package: String,
    pub scriptlet_count: usize,
    pub command_counts: BTreeMap<String, usize>,
    pub blocked_class_hints: Vec<String>,
}

impl ScriptletCorpusSummary {
    pub fn from_scriptlets(distro: &str, package: &str, scriptlets: &[Scriptlet]) -> Self {
        let mut command_counts = BTreeMap::new();
        let mut blocked = BTreeSet::new();

        for scriptlet in scriptlets {
            for command in commands_from_scriptlet(&scriptlet.content) {
                *command_counts.entry(command.clone()).or_insert(0) += 1;
                match command.as_str() {
                    "dnf" | "yum" | "rpm" | "apt" | "apt-get" | "dpkg" | "pacman" => {
                        blocked.insert("package-manager-recursion".to_string());
                    }
                    "curl" | "wget" | "scp" | "ssh" => {
                        blocked.insert("network".to_string());
                    }
                    "restorecon" | "semanage" | "setsebool" => {
                        blocked.insert("selinux".to_string());
                    }
                    "setcap" | "setpriv" | "chmod" => {}
                    _ => {}
                }
            }
        }

        Self {
            distro: distro.to_string(),
            package: package.to_string(),
            scriptlet_count: scriptlets.len(),
            command_counts,
            blocked_class_hints: blocked.into_iter().collect(),
        }
    }
}

fn commands_from_scriptlet(content: &str) -> Vec<String> {
    content
        .lines()
        .filter_map(first_command_token)
        .collect()
}

fn first_command_token(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }

    let trimmed = trimmed
        .strip_prefix("if ")
        .or_else(|| trimmed.strip_prefix("then "))
        .unwrap_or(trimmed);

    let token = trimmed
        .split(|c: char| c.is_whitespace() || c == ';' || c == '(')
        .next()?;
    let command = token.rsplit('/').next().unwrap_or(token);

    if command.is_empty()
        || matches!(
            command,
            "if" | "then" | "else" | "elif" | "fi" | "case" | "esac" | "for" | "do" | "done"
        )
    {
        return None;
    }

    Some(command.to_string())
}
```

This is evidence-only corpus counting. It must not become conversion authority.

- [ ] **Step 4: Expose the module**

In `apps/remi/src/server/mod.rs`, add:

```rust
pub mod scriptlet_corpus;
```

near the other `pub mod` declarations.

- [ ] **Step 5: Run corpus tests**

Run:

```bash
cargo test -p remi scriptlet_corpus::tests
```

Expected: pass.

- [ ] **Step 6: Commit Task 2**

```bash
git add apps/remi/src/server/scriptlet_corpus.rs apps/remi/src/server/mod.rs
git commit -m "feat(remi): add scriptlet corpus summary"
```

## Task 3: Instrument Conversion Service Timing

**Files:**

- Modify: `apps/remi/src/server/conversion.rs`

- [ ] **Step 1: Write failing unit test for timing report plumbing**

Add this test near the existing conversion service tests in `apps/remi/src/server/conversion.rs`:

```rust
#[test]
fn server_conversion_result_can_carry_timing_report() {
    use crate::server::conversion_timing::{ConversionPhase, ConversionTimingReport};
    use std::time::Duration;

    let mut timing = ConversionTimingReport::new("fedora", "nginx", None);
    timing.record(ConversionPhase::PackageLookup, Duration::from_millis(7));
    timing.finish(true);

    let result = ServerConversionResult {
        name: "nginx".to_string(),
        version: "1.28.0".to_string(),
        distro: "fedora".to_string(),
        chunk_hashes: vec![],
        total_size: 0,
        content_hash: "sha256:test".to_string(),
        ccs_path: std::path::PathBuf::from("/tmp/nginx.ccs"),
        timing: Some(timing),
    };

    assert_eq!(result.timing.unwrap().phases[0].duration_ms, 7);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run:

```bash
cargo test -p remi server_conversion_result_can_carry_timing_report
```

Expected: compile failure because `ServerConversionResult` has no `timing` field.

- [ ] **Step 3: Add timing field**

In `apps/remi/src/server/conversion.rs`, import the timing types:

```rust
use crate::server::conversion_timing::{ConversionPhase, ConversionTimingReport};
```

Add this field to `ServerConversionResult`:

```rust
    /// Phase timing report for this conversion, when collected.
    pub timing: Option<ConversionTimingReport>,
```

Set `timing: None` in existing cached-result builders and persistence result constructors. Set `timing: Some(report)` on the fresh conversion path after `persist_conversion_result` returns.

- [ ] **Step 4: Time the existing phases**

In `convert_package_async`, create the report at the top:

```rust
let mut timing = ConversionTimingReport::new(distro, package_name, version);
```

Wrap the existing calls with `std::time::Instant` or `timing.time(...)` for these phases:

- `ConversionPhase::PackageLookup` around `find_package_for_conversion_async`
- `ConversionPhase::Download` around `download_package_with_refresh_async`
- `ConversionPhase::Checksum` around checksum calculation
- `ConversionPhase::CacheLookup` around `cached_conversion_result_async`
- `ConversionPhase::Parse` and `ConversionPhase::LegacyConversion` inside `parse_and_convert_package`
- `ConversionPhase::ChunkStorage` around `store_chunks`
- `ConversionPhase::Persistence` around `persist_conversion_result`

If splitting `Parse` and `LegacyConversion` inside the blocking function is awkward, add a small internal `ParsedConversionTiming` return field so the outer async function can append the nested phase timings.

- [ ] **Step 5: Emit timing logs**

After a fresh conversion succeeds, call:

```rust
timing.finish(true);
tracing::info!(
    target: "remi::conversion_timing",
    distro = %timing.distro,
    package = %timing.package,
    total_ms = timing.total_ms,
    phases = %serde_json::to_string(&timing.phases).unwrap_or_else(|_| "[]".to_string()),
    "conversion timing report"
);
```

Before returning a conversion error from `convert_package_async`, finish and log the report with `success = false` where the error is still in scope.

- [ ] **Step 6: Run targeted conversion timing test**

Run:

```bash
cargo test -p remi server_conversion_result_can_carry_timing_report
```

Expected: pass.

- [ ] **Step 7: Run Remi conversion tests**

Run:

```bash
cargo test -p remi conversion
```

Expected: pass.

- [ ] **Step 8: Commit Task 3**

```bash
git add apps/remi/src/server/conversion.rs
git commit -m "feat(remi): record conversion phase timing"
```

## Task 4: Add `remi conversion-benchmark` CLI

**Files:**

- Modify: `apps/remi/src/bin/remi.rs`

- [ ] **Step 1: Add CLI testable args shape**

Add a new command variant:

```rust
    /// Benchmark conversion latency and scriptlet corpus evidence.
    ConversionBenchmark(ConversionBenchmarkArgs),
```

Add the args struct:

```rust
#[derive(Args)]
struct ConversionBenchmarkArgs {
    /// Database path
    #[arg(long, default_value = "/var/lib/conary/conary.db")]
    db: String,

    /// Path to chunk storage directory
    #[arg(long, default_value = "/var/lib/conary/data/chunks")]
    chunk_dir: String,

    /// Path to cache/scratch directory
    #[arg(long, default_value = "/var/lib/conary/data/cache")]
    cache_dir: String,

    /// Distribution to benchmark, such as fedora, ubuntu, debian, or arch
    #[arg(long)]
    distro: String,

    /// Package names to benchmark. Repeat the flag for multiple packages.
    #[arg(long = "package")]
    packages: Vec<String>,

    /// Maximum repository packages to scan when no package names are supplied.
    #[arg(long, default_value = "25")]
    max_packages: usize,

    /// Emit JSON lines instead of pretty JSON.
    #[arg(long)]
    jsonl: bool,

    /// Parse package metadata and scriptlets without writing converted CCS packages.
    #[arg(long)]
    scan_only: bool,
}
```

- [ ] **Step 2: Wire command dispatch**

In the top-level command match, add:

```rust
Some(Command::ConversionBenchmark(args)) => run_conversion_benchmark_command(args),
```

Implement `run_conversion_benchmark_command` in `apps/remi/src/bin/remi.rs`:

```rust
fn run_conversion_benchmark_command(args: ConversionBenchmarkArgs) -> Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async move {
        let service = remi::server::ConversionService::new(
            PathBuf::from(args.chunk_dir),
            PathBuf::from(args.cache_dir),
            PathBuf::from(args.db),
            None,
        );

        let packages = if args.packages.is_empty() {
            service
                .benchmark_package_sample(&args.distro, args.max_packages)
                .await?
        } else {
            args.packages
        };

        for package in packages {
            let evidence = if args.scan_only {
                service.scan_package_scriptlets(&args.distro, &package, None, None).await?
            } else {
                service.benchmark_package_conversion(&args.distro, &package, None, None).await?
            };

            if args.jsonl {
                println!("{}", serde_json::to_string(&evidence)?);
            } else {
                println!("{}", serde_json::to_string_pretty(&evidence)?);
            }
        }

        Ok(())
    })
}
```

The helper methods used here are added in Task 5.

- [ ] **Step 3: Run CLI compile check to verify missing helpers**

Run:

```bash
cargo check -p remi
```

Expected: compile failure naming `benchmark_package_sample`, `scan_package_scriptlets`, and `benchmark_package_conversion`.

- [ ] **Step 4: Commit CLI shape after helpers are added**

Do not commit this task until Task 5 provides the helper methods and `cargo check -p remi` passes.

## Task 5: Add Benchmark Helper Methods

**Files:**

- Modify: `apps/remi/src/server/conversion.rs`

- [ ] **Step 1: Define benchmark evidence type**

Add this serializable type near `ServerConversionResult`:

```rust
#[derive(Debug, serde::Serialize)]
pub struct ConversionBenchmarkEvidence {
    pub distro: String,
    pub package: String,
    pub version: Option<String>,
    pub scan_only: bool,
    pub timing: Option<crate::server::conversion_timing::ConversionTimingReport>,
    pub scriptlet_summary: Option<crate::server::scriptlet_corpus::ScriptletCorpusSummary>,
    pub converted: bool,
    pub error: Option<String>,
}
```

- [ ] **Step 2: Add package sample query**

Add an async method on `ConversionService`:

```rust
pub async fn benchmark_package_sample(&self, distro: &str, limit: usize) -> Result<Vec<String>> {
    let service = self.clone();
    let distro = distro.to_string();
    tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&service.db_path)?;
        let repo_pattern = match distro.as_str() {
            "fedora" => "fedora%",
            "ubuntu" | "debian" => "ubuntu%",
            "arch" => "arch%",
            _ => return Err(anyhow!("Unknown distribution: {}", distro)),
        };
        let mut stmt = conn.prepare(
            "SELECT DISTINCT rp.name
             FROM repository_packages rp
             JOIN repositories r ON rp.repository_id = r.id
             WHERE r.name LIKE ?1 AND rp.size > 0
             ORDER BY rp.size DESC
             LIMIT ?2",
        )?;
        let names = stmt
            .query_map(rusqlite::params![repo_pattern, limit as i64], |row| row.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        Ok(names)
    })
    .await
    .map_err(|e| anyhow!("benchmark package sample task panicked: {e}"))?
}
```

- [ ] **Step 3: Add scan-only helper**

Add:

```rust
pub async fn scan_package_scriptlets(
    &self,
    distro: &str,
    package_name: &str,
    version: Option<&str>,
    architecture: Option<&str>,
) -> Result<ConversionBenchmarkEvidence> {
    let repo_pkg = self
        .find_package_for_conversion_async(distro, package_name, version, architecture)
        .await?;
    let cache_dir = self
        .cache_dir
        .canonicalize()
        .unwrap_or_else(|_| self.cache_dir.clone());
    let temp_dir = TempDir::new_in(&cache_dir).context("Failed to create temp directory")?;
    let (repo_pkg, pkg_path) = self
        .download_package_with_refresh_async(PackageDownloadRefresh {
            distro,
            package_name,
            version,
            architecture,
            repo_pkg,
            dest_dir: temp_dir.path(),
        })
        .await?;
    let service = self.clone();
    let distro_owned = distro.to_string();
    let package_owned = package_name.to_string();
    let summary_result = tokio::task::spawn_blocking(move || {
        let (mut metadata, _files, _format) = service.parse_package(&pkg_path, &distro_owned)?;
        Self::apply_repository_identity(&mut metadata, &repo_pkg);
        Ok(crate::server::scriptlet_corpus::ScriptletCorpusSummary::from_scriptlets(
            &distro_owned,
            &package_owned,
            &metadata.scriptlets,
        ))
    })
    .await
    .map_err(|e| anyhow!("scriptlet scan task panicked: {e}"))?;
    let summary = summary_result?;

    Ok(ConversionBenchmarkEvidence {
        distro: distro.to_string(),
        package: package_name.to_string(),
        version: version.map(ToString::to_string),
        scan_only: true,
        timing: None,
        scriptlet_summary: Some(summary),
        converted: false,
        error: None,
    })
}
```

- [ ] **Step 4: Add conversion benchmark helper**

Add:

```rust
pub async fn benchmark_package_conversion(
    &self,
    distro: &str,
    package_name: &str,
    version: Option<&str>,
    architecture: Option<&str>,
) -> Result<ConversionBenchmarkEvidence> {
    match self
        .convert_package_async(distro, package_name, version, architecture)
        .await
    {
        Ok(result) => Ok(ConversionBenchmarkEvidence {
            distro: distro.to_string(),
            package: package_name.to_string(),
            version: Some(result.version),
            scan_only: false,
            timing: result.timing,
            scriptlet_summary: None,
            converted: true,
            error: None,
        }),
        Err(err) => Ok(ConversionBenchmarkEvidence {
            distro: distro.to_string(),
            package: package_name.to_string(),
            version: version.map(ToString::to_string),
            scan_only: false,
            timing: None,
            scriptlet_summary: None,
            converted: false,
            error: Some(err.to_string()),
        }),
    }
}
```

- [ ] **Step 5: Run Remi compile check**

Run:

```bash
cargo check -p remi
```

Expected: pass.

- [ ] **Step 6: Commit Tasks 4 and 5 together**

```bash
git add apps/remi/src/bin/remi.rs apps/remi/src/server/conversion.rs
git commit -m "feat(remi): add conversion benchmark command"
```

## Task 6: Document The Baseline Workflow

**Files:**

- Modify: `docs/modules/remi.md`

- [ ] **Step 1: Add benchmark section**

Add a section named `Conversion Benchmark Evidence` to `docs/modules/remi.md`:

````markdown
## Conversion Benchmark Evidence

Remi includes a local benchmark command for measuring cold-path conversion cost
before making public latency claims:

```bash
cargo run -p remi -- conversion-benchmark \
  --db /var/lib/conary/conary.db \
  --chunk-dir /var/lib/conary/data/chunks \
  --cache-dir /var/lib/conary/data/cache \
  --distro fedora \
  --package nginx \
  --jsonl
```

Use `--scan-only` to parse package metadata and summarize scriptlet helper
commands without writing converted CCS packages:

```bash
cargo run -p remi -- conversion-benchmark \
  --db /var/lib/conary/conary.db \
  --chunk-dir /var/lib/conary/data/chunks \
  --cache-dir /var/lib/conary/data/cache \
  --distro fedora \
  --max-packages 25 \
  --scan-only \
  --jsonl
```

The scriptlet corpus summary is evidence for adapter planning only. It is not
the authority for declaring a scriptlet `replaced`; that authority belongs to
the legacy scriptlet semantics bundle decision model.
````

- [ ] **Step 2: Run docs grep**

Run:

```bash
rg -n "Conversion Benchmark Evidence|scriptlet corpus summary|legacy scriptlet semantics bundle" docs/modules/remi.md
```

Expected: the new section and warning sentence are present.

- [ ] **Step 3: Commit docs**

```bash
git add docs/modules/remi.md
git commit -m "docs(remi): document conversion benchmark evidence"
```

## Task 7: Final Verification

**Files:**

- Verify current workspace only.

- [ ] **Step 1: Run targeted tests**

```bash
cargo test -p remi conversion_timing
cargo test -p remi scriptlet_corpus
cargo test -p remi conversion
```

Expected: all pass.

- [ ] **Step 2: Run formatting and diff checks**

```bash
cargo fmt --check
git diff --check
```

Expected: both pass.

- [ ] **Step 3: Run broader Remi check**

```bash
cargo check -p remi
```

Expected: pass.

- [ ] **Step 4: Summarize evidence**

Record in the final response:

- commands run;
- pass/fail status;
- any benchmark command that could not be run because local repository metadata was absent;
- commit hashes created during the goal;
- whether the worktree is clean.
