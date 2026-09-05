// crates/conary-core/src/transaction/recovery/tests/scan.rs

use super::*;
use crate::generation::metadata::GenerationMetadata;
use crate::generation::mount::{GenerationMountOutcome, MountOptions};
use std::cell::RefCell;

#[derive(Default)]
struct ScanRuntime {
    mounts: RefCell<Vec<MountOptions>>,
    fail_mount: bool,
}

impl RecoveryRuntime for ScanRuntime {
    fn mount(&self, options: &MountOptions) -> Result<GenerationMountOutcome> {
        self.mounts.borrow_mut().push(options.clone());
        if self.fail_mount {
            return Err(crate::Error::IoError("fixture mount failure".into()));
        }
        Ok(if options.verity {
            GenerationMountOutcome::ComposefsVerity
        } else {
            GenerationMountOutcome::ComposefsPlain
        })
    }
}

fn fixture() -> (TempDir, Connection, TransactionEngine) {
    let (tmp, conn, engine, _) = recovery_fixture();
    for generation in [6, 7] {
        crate::transaction::tests::write_valid_generation_artifact(tmp.path(), generation);
    }
    let gen_dir = tmp.path().join("generations/6");
    let mut metadata = GenerationMetadata::read_from(&gen_dir).unwrap();
    metadata.fsverity_enabled = true;
    metadata.erofs_verity_digest = Some("ab".repeat(32));
    metadata.write_to(&gen_dir).unwrap();
    (tmp, conn, engine)
}

#[test]
fn descending_scan_selects_highest_policy_eligible_artifact_with_typed_skips() {
    for cmdline in ["quiet", "conary.verity=on", "conary.verity=off"] {
        let (tmp, conn, engine) = fixture();
        let runtime = ScanRuntime::default();
        let policy = VerityPolicy::from_kernel_cmdline(cmdline);
        let evidence = engine
            .recover_boot_selection_with_runtime(&conn, &policy, &runtime)
            .unwrap();
        let verified = policy.requires_verification().unwrap();
        let selected = if verified { 6 } else { 7 };
        assert_eq!(evidence.selected_generation, Some(selected));
        assert_eq!(
            crate::generation::mount::current_generation(tmp.path()).unwrap(),
            Some(selected)
        );
        assert_eq!(
            evidence.skipped_artifacts,
            if verified {
                vec![RecoverySkippedArtifact {
                    generation: 7,
                    reason: RecoverySkipReason::VerityPolicy(
                        VerityPolicyError::MissingGenerationVerity { generation: 7 },
                    ),
                }]
            } else {
                Vec::new()
            }
        );
        let mounts = runtime.mounts.borrow();
        assert_eq!(mounts.len(), 1);
        assert_eq!(
            mounts[0].image_path,
            tmp.path()
                .join(format!("generations/{selected}/root.erofs"))
        );
        assert_eq!(mounts[0].verity, verified);
        assert_eq!(mounts[0].digest, verified.then(|| "ab".repeat(32)));
    }
}

#[test]
fn skipped_invalid_artifacts_are_also_reported() {
    let (tmp, conn, engine) = fixture();
    std::fs::create_dir_all(tmp.path().join("generations/8")).unwrap();
    let evidence = engine
        .recover_boot_selection_with_runtime(
            &conn,
            &VerityPolicy::ExplicitlyOff,
            &ScanRuntime::default(),
        )
        .unwrap();
    assert_eq!(evidence.selected_generation, Some(7));
    assert!(matches!(
        evidence.skipped_artifacts.as_slice(),
        [RecoverySkippedArtifact {
            generation: 8,
            reason: RecoverySkipReason::InvalidArtifact { .. },
        }]
    ));
}

#[test]
fn invalid_policy_never_becomes_candidate_ineligibility() {
    let (_tmp, _conn, engine) = fixture();
    let error = engine
        .find_latest_intact_generation(&VerityPolicy::from_kernel_cmdline("conary.verity="))
        .unwrap_err();
    assert!(matches!(
        error,
        crate::Error::BootVerity(VerityPolicyError::InvalidArgument { .. })
    ));
}

#[test]
fn mount_failure_is_fatal_and_does_not_select_a_lower_or_plain_artifact() {
    let (tmp, conn, engine) = fixture();
    let runtime = ScanRuntime {
        fail_mount: true,
        ..Default::default()
    };
    let error = engine
        .recover_boot_selection_with_runtime(&conn, &VerityPolicy::Verified, &runtime)
        .unwrap_err();
    assert!(matches!(error, crate::Error::IoError(_)));
    assert_eq!(runtime.mounts.borrow().len(), 1);
    assert!(runtime.mounts.borrow()[0].verity);
    assert_eq!(
        crate::generation::mount::current_generation(tmp.path()).unwrap(),
        None
    );
}
