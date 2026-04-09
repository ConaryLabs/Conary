// src/commands/derivation.rs

//! Derivation engine command handlers

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use conary_core::derivation::executor::{DerivationExecutor, ExecutionResult, ExecutorConfig};
use conary_core::derivation::id::{DerivationId, DerivationInputs};
use conary_core::derivation::recipe_hash::{build_script_hash, source_hash};
use conary_core::generation::mount::{MountOptions, mount_generation, unmount_generation};
use conary_core::recipe::parse_recipe_file;
use rusqlite::Connection;
use tempfile::TempDir;

const DEFAULT_CAS_DIR: &str = "/var/lib/conary/objects";

struct CasRuntime {
    path: PathBuf,
    _tempdir: Option<TempDir>,
}

fn open_derivation_db(db_path: Option<&Path>) -> Result<Connection> {
    match db_path {
        Some(path) => conary_core::db::open(path)
            .with_context(|| format!("Failed to open derivation DB: {}", path.display())),
        None => {
            let conn =
                Connection::open_in_memory().context("Failed to open in-memory derivation DB")?;
            conary_core::db::schema::migrate(&conn)
                .context("Failed to initialize in-memory derivation schema")?;
            Ok(conn)
        }
    }
}

fn prepare_cas_runtime(cas_dir: &Path, db_path: Option<&Path>) -> Result<CasRuntime> {
    if db_path.is_none() {
        let tempdir = tempfile::tempdir().context("Failed to create standalone CAS tempdir")?;
        let path = tempdir.path().join("objects");
        std::fs::create_dir_all(&path)
            .with_context(|| format!("Failed to create standalone CAS dir: {}", path.display()))?;
        return Ok(CasRuntime {
            path,
            _tempdir: Some(tempdir),
        });
    }

    let resolved = match db_path {
        Some(db_path) if cas_dir == Path::new(DEFAULT_CAS_DIR) => {
            conary_core::db::paths::objects_dir(&db_path.to_string_lossy())
        }
        _ => cas_dir.to_path_buf(),
    };

    std::fs::create_dir_all(&resolved)
        .with_context(|| format!("Failed to create CAS dir: {}", resolved.display()))?;

    Ok(CasRuntime {
        path: resolved,
        _tempdir: None,
    })
}

fn render_execution_summary(result: &ExecutionResult) -> Vec<String> {
    match result {
        ExecutionResult::CacheHit {
            derivation_id,
            record,
        } => vec![
            format!("Status: cache hit"),
            format!("Derivation ID: {derivation_id}"),
            format!("Output hash: {}", record.output_hash),
        ],
        ExecutionResult::Built {
            derivation_id,
            output,
        } => vec![
            format!("Status: built"),
            format!("Derivation ID: {derivation_id}"),
            format!("Output hash: {}", output.manifest.output_hash),
        ],
    }
}

fn with_mounted_env_sysroot<T, F>(env: &Path, cas_dir: &Path, f: F) -> Result<T>
where
    F: FnOnce(&Path) -> Result<T>,
{
    let mount_root = tempfile::tempdir().context("Failed to create env mount tempdir")?;
    let mount_point = mount_root.path().join("sysroot");
    std::fs::create_dir_all(&mount_point)
        .with_context(|| format!("Failed to create mount point: {}", mount_point.display()))?;

    let opts = MountOptions {
        image_path: env.to_path_buf(),
        basedir: cas_dir.to_path_buf(),
        mount_point: mount_point.clone(),
        verity: false,
        digest: None,
        upperdir: None,
        workdir: None,
    };

    mount_generation(&opts)
        .with_context(|| format!("Failed to mount build environment: {}", env.display()))?;

    let result = f(&mount_point);
    let unmount_result = unmount_generation(&mount_point).with_context(|| {
        format!(
            "Failed to unmount build environment: {}",
            mount_point.display()
        )
    });

    match (result, unmount_result) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(err), Ok(())) => Err(err),
        (Ok(_), Err(err)) => Err(err),
        (Err(err), Err(_)) => Err(err),
    }
}

/// Build a recipe into CAS via the derivation engine.
///
/// Loads the recipe, opens the derivation index DB, mounts the supplied
/// environment image read-only as a sysroot, and executes the real
/// `DerivationExecutor` pipeline.
pub async fn cmd_derivation_build(
    recipe: &Path,
    env: &Path,
    cas_dir: &Path,
    db_path: Option<&Path>,
) -> Result<()> {
    let parsed = parse_recipe_file(recipe)
        .with_context(|| format!("Failed to parse recipe: {}", recipe.display()))?;

    println!(
        "Recipe: {} v{}",
        parsed.package.name, parsed.package.version
    );

    let env_hash = sha256_of_path(env)
        .with_context(|| format!("Failed to hash environment image: {}", env.display()))?;
    let target_triple = current_target_triple();
    let conn = open_derivation_db(db_path)?;
    let cas_runtime = prepare_cas_runtime(cas_dir, db_path)?;
    let cas = conary_core::filesystem::CasStore::new(&cas_runtime.path)
        .with_context(|| format!("Failed to open CAS store: {}", cas_runtime.path.display()))?;
    let executor =
        DerivationExecutor::new(cas, cas_runtime.path.clone(), ExecutorConfig::default());

    let result = with_mounted_env_sysroot(env, &cas_runtime.path, |sysroot| {
        executor
            .execute(
                &parsed,
                &env_hash,
                &BTreeMap::new(),
                &target_triple,
                sysroot,
                &conn,
            )
            .map_err(|e| anyhow::anyhow!("Derivation build failed: {e}"))
    })?;

    for line in render_execution_summary(&result) {
        println!("{line}");
    }
    println!("CAS directory: {}", cas_runtime.path.display());
    println!("Dependency IDs: transitive resolution is handled by profile/pipeline planning.");

    Ok(())
}

/// Show the derivation ID for a recipe without building.
///
/// Computes the content-addressed derivation ID from the recipe inputs and
/// the provided build environment hash, then prints it.
pub async fn cmd_derivation_show(recipe: &Path, env_hash: &str) -> Result<()> {
    let parsed = parse_recipe_file(recipe)
        .with_context(|| format!("Failed to parse recipe: {}", recipe.display()))?;

    println!(
        "Recipe: {} v{}",
        parsed.package.name, parsed.package.version
    );

    let inputs = DerivationInputs {
        source_hash: source_hash(&parsed),
        build_script_hash: build_script_hash(&parsed),
        dependency_ids: BTreeMap::new(),
        build_env_hash: env_hash.to_owned(),
        target_triple: current_target_triple(),
        build_options: BTreeMap::new(),
    };

    let drv_id = DerivationId::compute(&inputs).context("Derivation input validation failed")?;
    println!("Derivation ID: {drv_id}");

    Ok(())
}

/// SHA-256 hash of a file's contents, returned as a 64-char hex string.
fn sha256_of_path(path: &Path) -> Result<String> {
    let mut file =
        std::fs::File::open(path).with_context(|| format!("Cannot open {}", path.display()))?;
    conary_core::hash::sha256_reader_hex(&mut file)
        .with_context(|| format!("Failed to hash {}", path.display()))
}

/// Return the current platform's target triple.
fn current_target_triple() -> String {
    format!("{}-unknown-linux-gnu", std::env::consts::ARCH)
}

#[cfg(test)]
mod tests {
    use super::{
        current_target_triple, open_derivation_db, prepare_cas_runtime, render_execution_summary,
    };
    use conary_core::derivation::executor::ExecutionResult;
    use conary_core::derivation::id::{DerivationId, DerivationInputs};
    use conary_core::derivation::index::DerivationRecord;
    use std::path::Path;

    #[test]
    fn test_open_derivation_db_uses_in_memory_when_db_path_is_none() {
        let conn = open_derivation_db(None).unwrap();
        let backing_file: String = conn
            .query_row("PRAGMA database_list", [], |row| row.get(2))
            .unwrap();

        assert!(backing_file.is_empty());
        let table_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'derivation_index'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(table_count, 1);
    }

    #[test]
    fn test_standalone_derivation_build_uses_temp_cas_dir() {
        let requested = Path::new("/var/lib/conary/objects");
        let runtime = prepare_cas_runtime(requested, None).unwrap();

        assert!(runtime.path.exists());
        assert_ne!(runtime.path, requested);
    }

    #[test]
    fn test_current_target_triple_is_non_empty() {
        let triple = current_target_triple();
        assert!(!triple.is_empty());
        assert!(triple.contains('-'));
    }

    #[test]
    fn test_derivation_build_no_longer_returns_stub_message_on_executor_path() {
        let derivation_id = DerivationId::compute(&DerivationInputs {
            source_hash: "0".repeat(64),
            build_script_hash: "1".repeat(64),
            dependency_ids: Default::default(),
            build_env_hash: "2".repeat(64),
            target_triple: "x86_64-unknown-linux-gnu".to_string(),
            build_options: Default::default(),
        })
        .unwrap();
        let result = ExecutionResult::CacheHit {
            derivation_id,
            record: DerivationRecord {
                derivation_id: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                    .to_string(),
                output_hash: "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210"
                    .to_string(),
                package_name: "hello".to_string(),
                package_version: "1.0.0".to_string(),
                manifest_cas_hash: "a".repeat(64),
                stage: None,
                build_env_hash: Some("b".repeat(64)),
                built_at: "2026-04-08T00:00:00Z".to_string(),
                build_duration_secs: 1,
                trust_level: 2,
                provenance_cas_hash: None,
                reproducible: None,
            },
        };

        let summary = render_execution_summary(&result).join("\n");
        assert!(summary.contains("Status: cache hit"));
        assert!(summary.contains("Derivation ID:"));
        assert!(!summary.contains("[NOT YET IMPLEMENTED]"));
    }
}
