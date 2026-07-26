// conary-core/src/derivation/pipeline/tests.rs

use super::*;
use crate::derivation::compose::{ComposeError, erofs_image_hash};
use crate::derivation::id::DerivationInputs;
use crate::derivation::seed::{SeedMetadata, SeedSource};
use crate::derivation::test_helpers::helpers::make_recipe;
use std::collections::HashSet;
use std::path::Path;

/// Build a minimal test seed without a real EROFS image.
fn test_seed(dir: &Path) -> Seed {
    let image_content = b"test seed image bytes for pipeline";
    let image_path = dir.join("seed.erofs");
    std::fs::write(&image_path, image_content).unwrap();

    let actual_hash = erofs_image_hash(&image_path).unwrap();

    Seed {
        metadata: SeedMetadata {
            seed_id: actual_hash,
            source: SeedSource::SelfBuilt,
            origin_url: None,
            builder: Some("test".to_owned()),
            packages: vec!["gcc".to_owned()],
            target_triple: "x86_64-unknown-linux-gnu".to_owned(),
            verified_by: vec![],
            origin_distro: None,
            origin_version: None,
        },
        image_path,
        cas_dir: dir.join("cas"),
    }
}

#[test]
fn generate_profile_produces_correct_structure() {
    let dir = tempfile::tempdir().unwrap();
    let seed = test_seed(dir.path());

    let mut recipes = HashMap::new();
    recipes.insert("gcc-pass1".to_owned(), make_recipe("gcc-pass1", &[], &[]));
    recipes.insert(
        "gcc-pass2".to_owned(),
        make_recipe("gcc-pass2", &["gcc-pass1"], &[]),
    );
    recipes.insert("make".to_owned(), make_recipe("make", &[], &[]));
    recipes.insert("nginx".to_owned(), make_recipe("nginx", &[], &[]));

    let custom = HashSet::new();
    let build_steps =
        crate::derivation::build_order::compute_build_order(&recipes, &custom).unwrap();

    let profile = Pipeline::generate_profile(
        &seed.metadata.seed_id,
        &seed.metadata.source.to_string(),
        &seed.metadata.target_triple,
        &recipes,
        &build_steps,
        "test-manifest",
    )
    .unwrap();

    assert_eq!(profile.profile.manifest, "test-manifest");
    assert_eq!(profile.profile.target, "x86_64-unknown-linux-gnu");
    assert!(!profile.profile.profile_hash.is_empty());
    assert_eq!(profile.seed.id, seed.metadata.seed_id);
    assert!(
        !profile.stages.is_empty(),
        "profile should have at least one stage"
    );

    for stage in &profile.stages {
        assert_eq!(stage.build_env, seed.build_env_hash());
        for drv in &stage.derivations {
            assert_ne!(drv.derivation_id, "pending");
            assert!(!drv.derivation_id.is_empty());
        }
    }

    let total_drvs: usize = profile
        .stages
        .iter()
        .map(|stage| stage.derivations.len())
        .sum();
    assert_eq!(total_drvs, 4, "all 4 recipes should appear in the profile");
}

#[test]
fn generate_profile_hash_is_deterministic() {
    let dir = tempfile::tempdir().unwrap();
    let seed = test_seed(dir.path());

    let mut recipes = HashMap::new();
    recipes.insert("a".to_owned(), make_recipe("a", &[], &[]));

    let custom = HashSet::new();
    let build_steps =
        crate::derivation::build_order::compute_build_order(&recipes, &custom).unwrap();

    let p1 = Pipeline::generate_profile(
        &seed.metadata.seed_id,
        &seed.metadata.source.to_string(),
        &seed.metadata.target_triple,
        &recipes,
        &build_steps,
        "m",
    )
    .unwrap();
    let p2 = Pipeline::generate_profile(
        &seed.metadata.seed_id,
        &seed.metadata.source.to_string(),
        &seed.metadata.target_triple,
        &recipes,
        &build_steps,
        "m",
    )
    .unwrap();

    assert_eq!(p1.profile.profile_hash, p2.profile.profile_hash);
}

#[test]
fn generate_profile_different_seeds_produce_different_hashes() {
    let dir1 = tempfile::tempdir().unwrap();
    let seed1 = test_seed(dir1.path());

    let dir2 = tempfile::tempdir().unwrap();
    std::fs::write(dir2.path().join("seed.erofs"), b"different seed content").unwrap();
    let hash2 = erofs_image_hash(&dir2.path().join("seed.erofs")).unwrap();
    let seed2 = Seed {
        metadata: SeedMetadata {
            seed_id: hash2,
            source: SeedSource::SelfBuilt,
            origin_url: None,
            builder: None,
            packages: vec![],
            target_triple: "x86_64-unknown-linux-gnu".to_owned(),
            verified_by: vec![],
            origin_distro: None,
            origin_version: None,
        },
        image_path: dir2.path().join("seed.erofs"),
        cas_dir: dir2.path().join("cas"),
    };

    let mut recipes = HashMap::new();
    recipes.insert("a".to_owned(), make_recipe("a", &[], &[]));

    let custom = HashSet::new();
    let build_steps =
        crate::derivation::build_order::compute_build_order(&recipes, &custom).unwrap();

    let p1 = Pipeline::generate_profile(
        &seed1.metadata.seed_id,
        &seed1.metadata.source.to_string(),
        &seed1.metadata.target_triple,
        &recipes,
        &build_steps,
        "m",
    )
    .unwrap();
    let p2 = Pipeline::generate_profile(
        &seed2.metadata.seed_id,
        &seed2.metadata.source.to_string(),
        &seed2.metadata.target_triple,
        &recipes,
        &build_steps,
        "m",
    )
    .unwrap();

    assert_ne!(p1.profile.profile_hash, p2.profile.profile_hash);
}

#[test]
fn ordered_stages_groups_and_sorts_correctly() {
    use crate::derivation::build_order::BuildStep;

    let steps = vec![
        BuildStep {
            package: "nginx".to_owned(),
            stage: Stage::System,
            order: 3,
        },
        BuildStep {
            package: "gcc-pass1".to_owned(),
            stage: Stage::Toolchain,
            order: 0,
        },
        BuildStep {
            package: "make".to_owned(),
            stage: Stage::Foundation,
            order: 2,
        },
        BuildStep {
            package: "gcc-pass2".to_owned(),
            stage: Stage::Toolchain,
            order: 1,
        },
    ];

    let stages = ordered_stages(&steps);

    assert_eq!(stages.len(), 3);
    assert_eq!(stages[0].0, Stage::Toolchain);
    assert_eq!(stages[0].1, vec!["gcc-pass1", "gcc-pass2"]);
    assert_eq!(stages[1].0, Stage::Foundation);
    assert_eq!(stages[1].1, vec!["make"]);
    assert_eq!(stages[2].0, Stage::System);
    assert_eq!(stages[2].1, vec!["nginx"]);
}

#[test]
fn collect_dep_ids_picks_up_completed_deps() {
    let recipe = make_recipe("bash", &["glibc"], &["make"]);

    let glibc_id = DerivationId::compute(&DerivationInputs {
        source_hash: "src1".to_owned(),
        build_script_hash: "script1".to_owned(),
        dependency_ids: BTreeMap::new(),
        build_env_hash: "env1".to_owned(),
        target_triple: "x86_64-unknown-linux-gnu".to_owned(),
        build_options: BTreeMap::new(),
    })
    .unwrap();

    let mut completed = HashMap::new();
    completed.insert("glibc".to_owned(), glibc_id.clone());

    let dep_ids = collect_dep_ids(&recipe, &completed);

    assert_eq!(dep_ids.len(), 1);
    assert_eq!(dep_ids.get("glibc").unwrap(), &glibc_id);
    assert!(!dep_ids.contains_key("make"));
}

#[test]
fn empty_build_steps_produce_empty_profile() {
    let dir = tempfile::tempdir().unwrap();
    let seed = test_seed(dir.path());
    let recipes = HashMap::new();
    let build_steps: Vec<crate::derivation::build_order::BuildStep> = vec![];

    let profile = Pipeline::generate_profile(
        &seed.metadata.seed_id,
        &seed.metadata.source.to_string(),
        &seed.metadata.target_triple,
        &recipes,
        &build_steps,
        "empty",
    )
    .unwrap();

    assert!(profile.stages.is_empty());
}

#[test]
fn pipeline_error_from_compose_error() {
    let error: PipelineError = ComposeError::EmptyComposition.into();
    assert!(matches!(error, PipelineError::Compose(_)));
}

#[test]
fn pipeline_error_from_executor_error() {
    let error: PipelineError = ExecutorError::Build("test".to_owned()).into();
    assert!(matches!(error, PipelineError::Executor(_)));
}

#[test]
fn pipeline_config_fields() {
    let config = PipelineConfig {
        cas_dir: PathBuf::from("/tmp/cas"),
        work_dir: PathBuf::from("/tmp/work"),
        target_triple: "x86_64-unknown-linux-gnu".to_owned(),
        jobs: 4,
        log_dir: None,
        keep_logs: false,
        shell_on_failure: false,
        only_packages: None,
        cascade: false,
        substituter_sources: vec![],
        publish_endpoint: None,
        publish_token: None,
    };
    assert_eq!(config.jobs, 4);
    assert!(config.only_packages.is_none());
}
