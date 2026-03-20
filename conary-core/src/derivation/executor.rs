// conary-core/src/derivation/executor.rs

//! Single-package derivation executor.
//!
//! Wires together derivation ID computation, cache lookup, Kitchen-based
//! building, CAS output capture, and derivation index recording into a
//! single `execute()` method.

use std::collections::BTreeMap;
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
}

impl DerivationExecutor {
    /// Create a new executor backed by the given CAS store.
    #[must_use]
    pub fn new(cas: CasStore, cas_dir: PathBuf) -> Self {
        Self { cas, cas_dir }
    }

    /// Access the underlying CAS store.
    ///
    /// The pipeline needs this to load cached manifests for `CacheHit` results.
    #[must_use]
    pub fn cas(&self) -> &CasStore {
        &self.cas
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
        let destdir = self.cas_dir.join(format!("build-{}", &derivation_id.as_str()[..16]));
        std::fs::create_dir_all(&destdir).map_err(|e| ExecutorError::Io(e.to_string()))?;

        let start = Instant::now();

        let mut cook = kitchen
            .new_cook_with_dest(recipe, &destdir)
            .map_err(|e| ExecutorError::Build(e.to_string()))?;

        cook.prep()
            .map_err(|e| ExecutorError::Build(format!("prep: {e}")))?;
        cook.unpack()
            .map_err(|e| ExecutorError::Build(format!("unpack: {e}")))?;
        cook.patch()
            .map_err(|e| ExecutorError::Build(format!("patch: {e}")))?;
        cook.simmer()
            .map_err(|e| ExecutorError::Build(format!("simmer: {e}")))?;

        let build_duration = start.elapsed().as_secs();

        // Step 4: Capture output from DESTDIR into CAS.
        let manifest = capture_output(
            &destdir,
            &self.cas,
            derivation_id.as_str(),
            build_duration,
        )?;

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
        };

        index
            .insert(&record)
            .map_err(|e| ExecutorError::Index(e.to_string()))?;

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
        };
        let index = DerivationIndex::new(&conn);
        index.insert(&record).unwrap();

        // Execute -- should get a cache hit without building.
        let executor = DerivationExecutor::new(cas, tmp.path().join("cas"));
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
        let executor = DerivationExecutor::new(cas, tmp.path().join("cas"));
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

        let executor = DerivationExecutor::new(cas, tmp.path().join("cas"));
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
}
