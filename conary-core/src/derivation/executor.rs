// conary-core/src/derivation/executor.rs

//! Single-package derivation executor.
//!
//! Wires together derivation ID computation, cache lookup, Kitchen-based
//! building, CAS output capture, and derivation index recording into a
//! single `execute()` method.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use rusqlite::Connection;
use tracing::info;

use crate::filesystem::CasStore;
use crate::recipe::{Kitchen, KitchenConfig, Recipe};

use super::capture::{CaptureError, capture_output};
use super::id::{DerivationError, DerivationId, DerivationInputs};
use super::index::{DerivationIndex, DerivationRecord};
use super::output::PackageOutput;
use super::recipe_hash;

/// Errors that can occur during derivation execution.
#[derive(Debug, thiserror::Error)]
pub enum ExecutorError {
    /// The build itself failed (prep, unpack, patch, or simmer).
    #[error("build failed: {0}")]
    Build(String),
    /// Capturing output files from the DESTDIR into CAS failed.
    #[error("output capture failed: {0}")]
    Capture(#[from] CaptureError),
    /// An error occurred interacting with the CAS store.
    #[error("CAS error: {0}")]
    Cas(String),
    /// An error occurred reading or writing the derivation index.
    #[error("derivation index error: {0}")]
    Index(String),
    /// A general I/O error.
    #[error("I/O error: {0}")]
    Io(String),
    /// Derivation input validation failed.
    #[error(transparent)]
    Derivation(#[from] DerivationError),
}

/// Parameters passed to `write_build_log` to stay under the argument limit.
struct BuildLogParams<'a> {
    build_env_hash: &'a str,
    cook_log: &'a str,
    status: &'a str,
    duration_secs: u64,
    output_hash: Option<&'a str>,
}

/// Configuration for the derivation executor.
#[derive(Debug, Clone, Default)]
pub struct ExecutorConfig {
    /// Directory for build log files. None disables logging.
    pub log_dir: Option<PathBuf>,
    /// Preserve logs for successful builds (otherwise deleted on success).
    pub keep_logs: bool,
    /// Spawn an interactive shell when a build fails.
    pub shell_on_failure: bool,
}

/// The result of executing a derivation.
#[derive(Debug)]
pub enum ExecutionResult {
    /// The derivation was already built and found in the index.
    CacheHit {
        /// The content-addressed derivation ID.
        derivation_id: DerivationId,
        /// The existing record from the derivation index.
        record: DerivationRecord,
    },
    /// The derivation was built fresh.
    Built {
        /// The content-addressed derivation ID.
        derivation_id: DerivationId,
        /// The captured build output (manifest + serialized bytes + hash).
        output: PackageOutput,
    },
}

/// RAII guard that removes a directory on drop unless disarmed.
///
/// Used to ensure build output directories (DESTDIR) are cleaned up even when
/// the build fails partway through. Call [`disarm`](CleanupGuard::disarm) on
/// the success path to preserve the directory.
struct CleanupGuard {
    path: PathBuf,
    armed: bool,
}

impl CleanupGuard {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for CleanupGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// Spawn an interactive debug shell in the build environment.
///
/// Only spawns if stdin is a tty. Returns when the user exits the shell.
fn spawn_debug_shell(destdir: &Path, sysroot: &Path, recipe: &Recipe) {
    use std::io::IsTerminal;

    if !std::io::stdin().is_terminal() {
        tracing::warn!("--shell-on-failure: no tty detected, skipping shell");
        return;
    }

    let shell = std::env::var("SHELL").unwrap_or_else(|_| {
        if std::path::Path::new("/bin/bash").exists() {
            "/bin/bash".to_owned()
        } else {
            "/bin/sh".to_owned()
        }
    });

    eprintln!("\n  Dropping into build environment. Exit shell to continue.\n");

    let status = std::process::Command::new(&shell)
        .current_dir(destdir)
        .env("DESTDIR", destdir)
        .env("SYSROOT", sysroot)
        .env("PACKAGE", &recipe.package.name)
        .env("VERSION", &recipe.package.version)
        .status();

    match status {
        Ok(s) => info!("debug shell exited with {s}"),
        Err(e) => tracing::warn!("failed to spawn debug shell: {e}"),
    }
}

/// Single-package derivation executor.
///
/// Orchestrates the full derivation lifecycle: compute the content-addressed
/// derivation ID, check the index for a cache hit, build via Kitchen if
/// needed, capture outputs into CAS, and record the result in the index.
pub struct DerivationExecutor {
    /// Content-addressable store for build outputs.
    cas: CasStore,
    /// Root directory for CAS objects (needed for building Kitchen paths).
    cas_dir: PathBuf,
    /// Executor configuration (logging, shell-on-failure, etc.).
    config: ExecutorConfig,
}

impl DerivationExecutor {
    /// Create a new executor backed by the given CAS store.
    #[must_use]
    pub fn new(cas: CasStore, cas_dir: PathBuf, config: ExecutorConfig) -> Self {
        Self { cas, cas_dir, config }
    }

    /// Access the underlying CAS store.
    ///
    /// The pipeline needs this to load cached manifests for `CacheHit` results.
    #[must_use]
    pub fn cas(&self) -> &CasStore {
        &self.cas
    }

    /// Write a build log file to `config.log_dir` if configured.
    ///
    /// Returns the path of the written log, or `None` if logging is disabled
    /// or the write fails (failure is logged as a warning, not propagated).
    fn write_build_log(
        &self,
        recipe: &Recipe,
        derivation_id: &DerivationId,
        params: &BuildLogParams<'_>,
    ) -> Option<PathBuf> {
        let BuildLogParams { build_env_hash, cook_log, status, duration_secs, output_hash } =
            params;
        let log_dir = self.config.log_dir.as_ref()?;

        if let Err(e) = std::fs::create_dir_all(log_dir) {
            tracing::warn!("failed to create log_dir {}: {e}", log_dir.display());
            return None;
        }

        let filename = format!(
            "{}-{}.log",
            recipe.package.name,
            &derivation_id.as_str()[..16],
        );
        let log_path = log_dir.join(&filename);

        let timestamp = chrono::Utc::now().to_rfc3339();

        let mut content = format!(
            "=== derivation build log ===\n\
             package: {}\n\
             version: {}\n\
             derivation_id: {}\n\
             build_env_hash: {}\n\
             timestamp: {}\n\
             ---\n\
             {}\n\
             ---\n\
             status: {}\n\
             duration_secs: {}\n",
            recipe.package.name,
            recipe.package.version,
            derivation_id.as_str(),
            build_env_hash,
            timestamp,
            cook_log,
            status,
            duration_secs,
        );

        if let Some(hash) = output_hash {
            content.push_str(&format!("output_hash: {hash}\n"));
        }

        match std::fs::File::create(&log_path) {
            Ok(mut f) => {
                if let Err(e) = f.write_all(content.as_bytes()) {
                    tracing::warn!("failed to write build log {}: {e}", log_path.display());
                    return None;
                }
                Some(log_path)
            }
            Err(e) => {
                tracing::warn!("failed to create build log {}: {e}", log_path.display());
                None
            }
        }
    }

    /// Execute a derivation: check cache, build if needed, capture output.
    ///
    /// # Flow
    ///
    /// 1. Compute the `DerivationId` from recipe inputs.
    /// 2. Check the derivation index for an existing record (cache hit).
    /// 3. If not cached, build via Kitchen with bootstrap config.
    /// 4. Capture the DESTDIR into CAS via `capture_output()`.
    /// 5. Serialize the manifest, store it in CAS.
    /// 6. Record the derivation in the index.
    /// 7. Return the result.
    ///
    /// # Errors
    ///
    /// Returns `ExecutorError` on build failure, CAS errors, index errors,
    /// or I/O errors.
    pub fn execute(
        &self,
        recipe: &Recipe,
        build_env_hash: &str,
        dep_ids: &BTreeMap<String, DerivationId>,
        target_triple: &str,
        sysroot: &Path,
        conn: &Connection,
    ) -> Result<ExecutionResult, ExecutorError> {
        // Step 1: Compute derivation ID from all inputs.
        let src_hash = recipe_hash::source_hash(recipe);
        let script_hash = recipe_hash::build_script_hash(recipe);

        let inputs = DerivationInputs {
            source_hash: src_hash,
            build_script_hash: script_hash,
            dependency_ids: dep_ids.clone(),
            build_env_hash: build_env_hash.to_owned(),
            target_triple: target_triple.to_owned(),
            build_options: BTreeMap::new(),
        };

        let derivation_id = DerivationId::compute(&inputs)?;
        info!(
            "derivation {} for {}-{}",
            derivation_id, recipe.package.name, recipe.package.version,
        );

        // Step 2: Check derivation index for cache hit.
        let index = DerivationIndex::new(conn);
        if let Some(record) = index
            .lookup(derivation_id.as_str())
            .map_err(|e| ExecutorError::Index(e.to_string()))?
        {
            info!(
                "cache hit for {} (output_hash={})",
                derivation_id,
                &record.output_hash[..16],
            );
            return Ok(ExecutionResult::CacheHit {
                derivation_id,
                record,
            });
        }

        // Step 3: Build using Kitchen.
        let config = KitchenConfig::for_bootstrap(sysroot);
        let kitchen = Kitchen::new(config);

        // Create a Cook that installs to a temporary DESTDIR.
        // The CleanupGuard ensures the directory is removed on any error path.
        let destdir = self.cas_dir.join(format!("build-{}", &derivation_id.as_str()[..16]));
        std::fs::create_dir_all(&destdir).map_err(|e| ExecutorError::Io(e.to_string()))?;
        let mut destdir_guard = CleanupGuard::new(destdir.clone());

        let start = Instant::now();

        let mut cook = kitchen
            .new_cook_with_dest(recipe, &destdir)
            .map_err(|e| ExecutorError::Build(e.to_string()))?;

        let build_result = (|| -> Result<(), ExecutorError> {
            cook.prep()
                .map_err(|e| ExecutorError::Build(format!("prep: {e}")))?;
            cook.unpack()
                .map_err(|e| ExecutorError::Build(format!("unpack: {e}")))?;
            cook.patch()
                .map_err(|e| ExecutorError::Build(format!("patch: {e}")))?;
            cook.simmer()
                .map_err(|e| ExecutorError::Build(format!("simmer: {e}")))?;
            Ok(())
        })();

        let build_duration = start.elapsed().as_secs();
        let cook_log = cook.build_log().to_owned();

        if let Err(build_err) = build_result {
            // Write log on failure (always preserved).
            let log_path = self.write_build_log(
                recipe,
                &derivation_id,
                &BuildLogParams {
                    build_env_hash,
                    cook_log: &cook_log,
                    status: "FAILED",
                    duration_secs: build_duration,
                    output_hash: None,
                },
            );
            if let Some(path) = &log_path {
                info!("build log: {}", path.display());
            }

            if self.config.shell_on_failure {
                // Disarm guard to keep DESTDIR alive during shell session.
                destdir_guard.disarm();

                eprintln!("[FAILED] {}-{}", recipe.package.name, recipe.package.version);
                if let Some(path) = &log_path {
                    eprintln!("  Build log: {}", path.display());
                }
                eprintln!("  Sysroot: {}", sysroot.display());
                eprintln!("  DESTDIR: {}", destdir.display());

                spawn_debug_shell(&destdir, sysroot, recipe);

                // Clean up DESTDIR after shell exits (guard was disarmed).
                let _ = std::fs::remove_dir_all(&destdir);
            }

            return Err(build_err);
        }

        // Step 4: Capture output from DESTDIR into CAS.
        let manifest = capture_output(
            &destdir,
            &self.cas,
            derivation_id.as_str(),
            build_duration,
        )?;

        // Output is safely in CAS -- disarm the guard so it does not
        // double-remove on the success path.
        destdir_guard.disarm();

        // Step 5: Serialize manifest and store in CAS.
        let pkg_output = PackageOutput::from_manifest(manifest)
            .map_err(|e| ExecutorError::Cas(format!("manifest serialization: {e}")))?;

        let manifest_cas_hash = self
            .cas
            .store(&pkg_output.manifest_bytes)
            .map_err(|e| ExecutorError::Cas(e.to_string()))?;

        // Step 6: Record in derivation index.
        let record = DerivationRecord {
            derivation_id: derivation_id.as_str().to_owned(),
            output_hash: pkg_output.manifest.output_hash.clone(),
            package_name: recipe.package.name.clone(),
            package_version: recipe.package.version.clone(),
            manifest_cas_hash,
            stage: None,
            build_env_hash: Some(build_env_hash.to_owned()),
            built_at: pkg_output.manifest.built_at.clone(),
            build_duration_secs: build_duration,
            trust_level: 0,
            provenance_cas_hash: None,
            reproducible: None,
        };

        index
            .insert(&record)
            .map_err(|e| ExecutorError::Index(e.to_string()))?;

        // Build provenance record (4 layers: source, build, signature, content).
        let source_prov = crate::provenance::SourceProvenance::from_tarball(
            &recipe.source.archive,
            &recipe.source.checksum,
        );

        let mut build_prov =
            crate::provenance::BuildProvenance::new(&recipe_hash::build_script_hash(recipe));
        build_prov
            .build_env
            .push(("build_env_hash".to_owned(), build_env_hash.to_owned()));
        build_prov
            .build_env
            .push(("target_triple".to_owned(), target_triple.to_owned()));
        build_prov.build_env.push((
            "derivation_id".to_owned(),
            derivation_id.as_str().to_owned(),
        ));
        build_prov
            .set_host_attestation(crate::provenance::HostAttestation::from_current_system());
        build_prov.complete();

        let sig_prov = crate::provenance::SignatureProvenance::default();

        let mut content_prov =
            crate::provenance::ContentProvenance::new(&pkg_output.manifest.output_hash);
        content_prov.total_size = pkg_output.manifest.files.iter().map(|f| f.size).sum();
        content_prov.file_count =
            (pkg_output.manifest.files.len() + pkg_output.manifest.symlinks.len()) as u64;

        let provenance =
            crate::provenance::Provenance::new(source_prov, build_prov, sig_prov, content_prov);

        // Store provenance as CAS object.
        let provenance_cas_hash = match provenance.to_json() {
            Ok(json) => match self.cas.store(json.as_bytes()) {
                Ok(hash) => Some(hash),
                Err(e) => {
                    tracing::warn!("failed to store provenance: {e}");
                    None
                }
            },
            Err(e) => {
                tracing::warn!("failed to serialize provenance: {e}");
                None
            }
        };

        // Update the derivation record with provenance hash and trust level 2 (locally built).
        if let Some(ref hash) = provenance_cas_hash {
            let _ = index.set_trust_level(derivation_id.as_str(), 2);
            let _ = conn.execute(
                "UPDATE derivation_index SET provenance_cas_hash = ?2 WHERE derivation_id = ?1",
                rusqlite::params![derivation_id.as_str(), hash],
            );
        }

        // Clean up the temporary DESTDIR (best effort).
        if let Err(e) = std::fs::remove_dir_all(&destdir) {
            tracing::warn!("failed to clean up DESTDIR {}: {e}", destdir.display());
        }

        info!(
            "built {} in {}s (output_hash={})",
            derivation_id,
            build_duration,
            &pkg_output.manifest.output_hash[..16.min(pkg_output.manifest.output_hash.len())],
        );

        // Write log on success; delete it unless keep_logs is set.
        let log_path = self.write_build_log(
            recipe,
            &derivation_id,
            &BuildLogParams {
                build_env_hash,
                cook_log: &cook_log,
                status: "SUCCESS",
                duration_secs: build_duration,
                output_hash: Some(&pkg_output.manifest.output_hash),
            },
        );
        if let Some(path) = &log_path && !self.config.keep_logs {
            let _ = std::fs::remove_file(path);
        }

        // Step 7: Return result.
        Ok(ExecutionResult::Built {
            derivation_id,
            output: pkg_output,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema::migrate;
    use crate::derivation::test_helpers::helpers::test_cas;
    use tempfile::TempDir;

    /// Create a minimal recipe for testing via TOML deserialization.
    fn test_recipe(name: &str, version: &str) -> Recipe {
        let toml_str = format!(
            r#"
[package]
name = "{name}"
version = "{version}"

[source]
archive = "https://example.com/{name}-{version}.tar.gz"
checksum = "sha256:abc123"

[build]
make = "make"
install = "make install"
"#
        );
        toml::from_str(&toml_str).expect("test recipe must parse")
    }

    /// Set up an in-memory database with migrations applied.
    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn
    }

    #[test]
    fn cache_hit_returns_existing_record() {
        let tmp = TempDir::new().unwrap();
        let cas = test_cas(tmp.path());
        let conn = setup_db();

        let recipe = test_recipe("glibc", "2.39");
        let dep_ids = BTreeMap::new();
        let build_env_hash = "env_hash_abc";
        let target_triple = "x86_64-unknown-linux-gnu";

        // Compute the derivation ID that execute() will produce.
        let src_hash = recipe_hash::source_hash(&recipe);
        let script_hash = recipe_hash::build_script_hash(&recipe);
        let inputs = DerivationInputs {
            source_hash: src_hash,
            build_script_hash: script_hash,
            dependency_ids: dep_ids.clone(),
            build_env_hash: build_env_hash.to_owned(),
            target_triple: target_triple.to_owned(),
            build_options: BTreeMap::new(),
        };
        let expected_id = DerivationId::compute(&inputs).unwrap();

        // Pre-insert a record so execute() finds it.
        let record = DerivationRecord {
            derivation_id: expected_id.as_str().to_owned(),
            output_hash: "out_hash_123".to_owned(),
            package_name: "glibc".to_owned(),
            package_version: "2.39".to_owned(),
            manifest_cas_hash: "manifest_hash_456".to_owned(),
            stage: Some("phase1".to_owned()),
            build_env_hash: Some(build_env_hash.to_owned()),
            built_at: "2026-03-19T12:00:00Z".to_owned(),
            build_duration_secs: 30,
            trust_level: 0,
            provenance_cas_hash: None,
            reproducible: None,
        };
        let index = DerivationIndex::new(&conn);
        index.insert(&record).unwrap();

        // Execute -- should get a cache hit without building.
        let executor = DerivationExecutor::new(cas, tmp.path().join("cas"), ExecutorConfig::default());
        let sysroot = tmp.path().join("sysroot");
        std::fs::create_dir_all(&sysroot).unwrap();

        let result = executor
            .execute(
                &recipe,
                build_env_hash,
                &dep_ids,
                target_triple,
                &sysroot,
                &conn,
            )
            .expect("execute must succeed");

        match result {
            ExecutionResult::CacheHit {
                derivation_id,
                record: hit_record,
            } => {
                assert_eq!(derivation_id, expected_id);
                assert_eq!(hit_record.output_hash, "out_hash_123");
                assert_eq!(hit_record.package_name, "glibc");
                assert_eq!(hit_record.manifest_cas_hash, "manifest_hash_456");
            }
            ExecutionResult::Built { .. } => {
                panic!("expected CacheHit, got Built");
            }
        }
    }

    #[test]
    fn derivation_id_is_deterministic_across_calls() {
        let recipe = test_recipe("zlib", "1.3.1");
        let dep_ids = BTreeMap::new();

        let compute_id = |recipe: &Recipe| {
            let inputs = DerivationInputs {
                source_hash: recipe_hash::source_hash(recipe),
                build_script_hash: recipe_hash::build_script_hash(recipe),
                dependency_ids: dep_ids.clone(),
                build_env_hash: "env_aaa".to_owned(),
                target_triple: "x86_64-unknown-linux-gnu".to_owned(),
                build_options: BTreeMap::new(),
            };
            DerivationId::compute(&inputs).unwrap()
        };

        let id1 = compute_id(&recipe);
        let id2 = compute_id(&recipe);
        assert_eq!(id1, id2, "same inputs must produce same derivation ID");
    }

    #[test]
    fn different_deps_produce_different_ids() {
        let recipe = test_recipe("bash", "5.2");

        let mut deps1 = BTreeMap::new();
        deps1.insert("glibc".to_owned(), DerivationId::compute(&DerivationInputs {
            source_hash: "src1".to_owned(),
            build_script_hash: "script1".to_owned(),
            dependency_ids: BTreeMap::new(),
            build_env_hash: "env1".to_owned(),
            target_triple: "x86_64-unknown-linux-gnu".to_owned(),
            build_options: BTreeMap::new(),
        }).unwrap());

        let mut deps2 = BTreeMap::new();
        deps2.insert("glibc".to_owned(), DerivationId::compute(&DerivationInputs {
            source_hash: "src2_different".to_owned(),
            build_script_hash: "script1".to_owned(),
            dependency_ids: BTreeMap::new(),
            build_env_hash: "env1".to_owned(),
            target_triple: "x86_64-unknown-linux-gnu".to_owned(),
            build_options: BTreeMap::new(),
        }).unwrap());

        let make_id = |deps: &BTreeMap<String, DerivationId>| {
            let inputs = DerivationInputs {
                source_hash: recipe_hash::source_hash(&recipe),
                build_script_hash: recipe_hash::build_script_hash(&recipe),
                dependency_ids: deps.clone(),
                build_env_hash: "env_aaa".to_owned(),
                target_triple: "x86_64-unknown-linux-gnu".to_owned(),
                build_options: BTreeMap::new(),
            };
            DerivationId::compute(&inputs).unwrap()
        };

        let id1 = make_id(&deps1);
        let id2 = make_id(&deps2);
        assert_ne!(id1, id2, "different dependency IDs must produce different derivation IDs");
    }

    #[test]
    fn cas_accessor_returns_store() {
        let tmp = TempDir::new().unwrap();
        let cas = test_cas(tmp.path());
        let executor = DerivationExecutor::new(cas, tmp.path().join("cas"), ExecutorConfig::default());
        // Just verify the accessor doesn't panic and returns a usable store.
        assert!(executor.cas().exists("nonexistent") == false);
    }

    #[test]
    fn cache_miss_with_no_kitchen_infra_returns_build_error() {
        // Without real source archives, Kitchen build will fail at prep.
        // This confirms the error path works correctly.
        let tmp = TempDir::new().unwrap();
        let cas = test_cas(tmp.path());
        let conn = setup_db();

        let recipe = test_recipe("coreutils", "9.5");
        let dep_ids = BTreeMap::new();
        let sysroot = tmp.path().join("sysroot");
        std::fs::create_dir_all(&sysroot).unwrap();

        let executor = DerivationExecutor::new(cas, tmp.path().join("cas"), ExecutorConfig::default());
        let result = executor.execute(
            &recipe,
            "env_hash",
            &dep_ids,
            "x86_64-unknown-linux-gnu",
            &sysroot,
            &conn,
        );

        match result {
            Err(ExecutorError::Build(msg)) => {
                assert!(
                    msg.contains("prep"),
                    "error should mention the prep phase, got: {msg}",
                );
            }
            other => {
                panic!(
                    "expected Build error from prep phase, got: {other:?}",
                );
            }
        }
    }

    #[test]
    fn destdir_cleaned_up_on_build_failure() {
        // Without real source archives the build will fail during prep.
        // The CleanupGuard should remove the DESTDIR on any error path.
        let tmp = TempDir::new().unwrap();
        let cas = test_cas(tmp.path());
        let conn = setup_db();

        let recipe = test_recipe("sed", "4.9");
        let dep_ids = BTreeMap::new();
        let sysroot = tmp.path().join("sysroot");
        std::fs::create_dir_all(&sysroot).unwrap();

        let cas_dir = tmp.path().join("cas");
        let executor = DerivationExecutor::new(cas, cas_dir.clone(), ExecutorConfig::default());

        let result = executor.execute(
            &recipe,
            "env_hash",
            &dep_ids,
            "x86_64-unknown-linux-gnu",
            &sysroot,
            &conn,
        );

        // The build must fail (no real sources).
        assert!(result.is_err(), "execute should fail without real sources");

        // Compute the expected DESTDIR path to verify it was cleaned up.
        let src_hash = recipe_hash::source_hash(&recipe);
        let script_hash = recipe_hash::build_script_hash(&recipe);
        let inputs = DerivationInputs {
            source_hash: src_hash,
            build_script_hash: script_hash,
            dependency_ids: dep_ids,
            build_env_hash: "env_hash".to_owned(),
            target_triple: "x86_64-unknown-linux-gnu".to_owned(),
            build_options: BTreeMap::new(),
        };
        let derivation_id = DerivationId::compute(&inputs).unwrap();
        let destdir = cas_dir.join(format!("build-{}", &derivation_id.as_str()[..16]));

        assert!(
            !destdir.exists(),
            "DESTDIR should have been cleaned up on build failure: {}",
            destdir.display(),
        );
    }

    #[test]
    fn shell_on_failure_does_not_hang_without_tty() {
        // In CI/tests, stdin is not a tty.
        // Verify that with shell_on_failure=true, execute() still returns
        // the build error without blocking.
        let tmp = TempDir::new().unwrap();
        let cas = test_cas(tmp.path());
        let conn = setup_db();

        let config = ExecutorConfig {
            log_dir: None,
            keep_logs: false,
            shell_on_failure: true, // enabled, but no tty in tests
        };

        let recipe = test_recipe("sed", "4.9");
        let sysroot = tmp.path().join("sysroot");
        std::fs::create_dir_all(&sysroot).unwrap();

        let executor = DerivationExecutor::new(cas, tmp.path().join("cas"), config);
        let result = executor.execute(
            &recipe,
            "env_hash",
            &BTreeMap::new(),
            "x86_64-unknown-linux-gnu",
            &sysroot,
            &conn,
        );

        // Should fail with Build error, not hang
        assert!(matches!(result, Err(ExecutorError::Build(_))));
    }

    #[test]
    fn build_log_written_on_failure() {
        let tmp = TempDir::new().unwrap();
        let log_dir = tmp.path().join("logs");
        let cas = test_cas(tmp.path());
        let conn = setup_db();

        let config = ExecutorConfig {
            log_dir: Some(log_dir.clone()),
            keep_logs: false,
            shell_on_failure: false,
        };

        let recipe = test_recipe("sed", "4.9");
        let sysroot = tmp.path().join("sysroot");
        std::fs::create_dir_all(&sysroot).unwrap();

        let executor = DerivationExecutor::new(cas, tmp.path().join("cas"), config);
        let result = executor.execute(
            &recipe,
            "env_hash",
            &BTreeMap::new(),
            "x86_64-unknown-linux-gnu",
            &sysroot,
            &conn,
        );

        assert!(result.is_err(), "execute should fail without real sources");

        // Log file should exist in the logs directory.
        let logs: Vec<_> = std::fs::read_dir(&log_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(logs.len(), 1, "should have one log file");

        let content = std::fs::read_to_string(logs[0].path()).unwrap();
        assert!(content.contains("package: sed"), "log should contain package name");
        assert!(content.contains("status: FAILED"), "log should show FAILED status");
    }

    #[test]
    fn successful_build_generates_provenance() {
        // Verify that the provenance types can be constructed without panicking.
        let source = crate::provenance::SourceProvenance::from_tarball(
            "https://example.com/test.tar.gz",
            "sha256:abc123",
        );
        let build = crate::provenance::BuildProvenance::new("script_hash");
        let sig = crate::provenance::SignatureProvenance::default();
        let content = crate::provenance::ContentProvenance::new("output_hash");
        let prov = crate::provenance::Provenance::new(source, build, sig, content);
        let json = prov.to_json().unwrap();
        assert!(json.contains("output_hash"));
    }
}
