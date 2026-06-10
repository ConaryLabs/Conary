// apps/conary/src/commands/bootstrap/run_record.rs

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::state::{BootstrapLatestPointer, BootstrapRunRecord};
use super::types::BootstrapRunOptions;
use crate::commands::operation_records::new_operation_id;

/// Load all recipes from subdirectories of `recipe_dir`, returning a `HashMap`
/// keyed by package name. Walks `cross-tools/`, `temp-tools/`, `system/`, `tier2/`.
pub(super) fn start_bootstrap_run_record(
    opts: &BootstrapRunOptions<'_>,
    manifest_path: &Path,
    recipe_dir: &Path,
    seed_id: &str,
) -> Result<BootstrapRunRecord> {
    let work_dir = PathBuf::from(opts.work_dir);
    std::fs::create_dir_all(&work_dir)?;

    let mut record = BootstrapRunRecord::started(
        new_operation_id("bootstrap-run"),
        work_dir,
        manifest_path.to_path_buf(),
        recipe_dir.to_path_buf(),
        seed_id.to_string(),
    );
    record.up_to = opts.up_to.map(str::to_owned);
    record.only = opts.only.map(|only| only.to_vec()).unwrap_or_default();
    record.cascade = opts.cascade;

    std::fs::create_dir_all(record.operation_dir())?;
    record.save()?;

    Ok(record)
}
fn link_bootstrap_run_outputs(record: &BootstrapRunRecord) -> Result<()> {
    std::fs::create_dir_all(&record.output_dir)?;
    let run_current_link = record.output_dir.join("current");
    let _ = std::fs::remove_file(&run_current_link);
    std::os::unix::fs::symlink("generations/1", &run_current_link)?;

    let top_output_dir = record.work_dir.join("output");
    std::fs::create_dir_all(&top_output_dir)?;
    let top_current_link = top_output_dir.join("current");
    let _ = std::fs::remove_file(&top_current_link);
    let relative_target = PathBuf::from("..")
        .join("operations")
        .join(&record.id)
        .join("output")
        .join("current");
    std::os::unix::fs::symlink(relative_target, &top_current_link)?;
    Ok(())
}
pub(super) fn finish_bootstrap_run_success(
    record: &mut BootstrapRunRecord,
    generation_dir: &Path,
    profile_hash: &str,
) -> Result<()> {
    record.generation_dir = Some(generation_dir.to_path_buf());
    record.profile_hash = Some(profile_hash.to_string());
    record.completed_successfully = true;
    record.failure_reason = None;
    record.save()?;
    BootstrapLatestPointer::new(record.id.clone(), record.path())
        .save(&BootstrapLatestPointer::path_for(&record.work_dir))?;
    link_bootstrap_run_outputs(record)?;
    Ok(())
}
pub(super) fn finish_bootstrap_run_failure(
    record: &mut BootstrapRunRecord,
    error: &anyhow::Error,
) -> Result<()> {
    record.completed_successfully = false;
    record.failure_reason = Some(error.to_string());
    record.save()
}
pub(super) fn load_completed_bootstrap_run_record(work_dir: &Path) -> Result<BootstrapRunRecord> {
    let pointer_path = BootstrapLatestPointer::path_for(work_dir);
    let latest = BootstrapLatestPointer::load(&pointer_path).with_context(|| {
        format!(
            "Failed to load bootstrap latest pointer from {}",
            pointer_path.display()
        )
    })?;
    let record = BootstrapRunRecord::load(&latest.record_path).with_context(|| {
        format!(
            "Failed to load bootstrap run record from {}",
            latest.record_path.display()
        )
    })?;
    if record.id != latest.operation_id {
        anyhow::bail!(
            "Bootstrap latest pointer {} does not match record id {}",
            latest.operation_id,
            record.id
        );
    }
    if !record.completed_successfully {
        anyhow::bail!(
            "Bootstrap run {} in {} did not complete successfully",
            record.id,
            work_dir.display()
        );
    }
    Ok(record)
}

#[cfg(test)]
mod tests {
    use super::super::state::{BootstrapLatestPointer, BootstrapRunRecord};
    use super::super::types::BootstrapRunOptions;
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_bootstrap_run_writes_success_record_with_output_paths() {
        let temp = tempfile::tempdir().expect("tempdir");
        let manifest_path = temp.path().join("system.toml");
        let recipe_dir = temp.path().join("recipes");
        std::fs::create_dir_all(&recipe_dir).expect("recipe dir");
        std::fs::write(
            &manifest_path,
            "[system]\nname = 'test'\ntarget = 'x86_64-conary-linux-gnu'\n",
        )
        .expect("manifest");

        let only = vec!["bash".to_string(), "coreutils".to_string()];
        let opts = BootstrapRunOptions {
            manifest: manifest_path.to_str().expect("manifest path"),
            work_dir: temp.path().to_str().expect("work dir"),
            seed: "/tmp/seed",
            recipe_dir: recipe_dir.to_str().expect("recipe dir"),
            up_to: Some("system"),
            only: Some(&only),
            cascade: true,
            keep_logs: false,
            shell_on_failure: false,
            verbose: false,
            no_substituters: false,
            publish: false,
        };

        let mut record = start_bootstrap_run_record(&opts, &manifest_path, &recipe_dir, "seed-abc")
            .expect("start record");
        let generation_dir = record.output_dir.join("generations").join("1");
        std::fs::create_dir_all(&generation_dir).expect("generation dir");

        finish_bootstrap_run_success(&mut record, &generation_dir, "profile-xyz")
            .expect("finish record");

        let loaded = BootstrapRunRecord::load(&record.path()).expect("load record");
        let latest = BootstrapLatestPointer::load(&BootstrapLatestPointer::path_for(temp.path()))
            .expect("load latest");

        assert_eq!(loaded.manifest_path, manifest_path);
        assert_eq!(loaded.recipe_dir, recipe_dir);
        assert_eq!(loaded.seed_id, "seed-abc");
        assert_eq!(loaded.up_to.as_deref(), Some("system"));
        assert_eq!(loaded.only, only);
        assert!(loaded.cascade);
        assert_eq!(
            loaded.derivation_db_path,
            loaded.operation_dir().join("derivations.db")
        );
        assert_eq!(loaded.output_dir, loaded.operation_dir().join("output"));
        assert_eq!(loaded.generation_dir, Some(generation_dir.clone()));
        assert_eq!(loaded.profile_hash.as_deref(), Some("profile-xyz"));
        assert!(loaded.completed_successfully);
        assert_eq!(latest.operation_id, loaded.id);
        assert_eq!(latest.record_path, loaded.path());
        assert_eq!(
            std::fs::read_link(loaded.output_dir.join("current")).expect("run current link"),
            PathBuf::from("generations").join("1")
        );
        assert_eq!(
            std::fs::read_link(temp.path().join("output/current")).expect("top current link"),
            PathBuf::from("..")
                .join("operations")
                .join(&loaded.id)
                .join("output")
                .join("current")
        );
    }
}
