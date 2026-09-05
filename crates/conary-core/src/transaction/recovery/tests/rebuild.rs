// crates/conary-core/src/transaction/recovery/tests/rebuild.rs

use super::*;
use crate::filesystem::fsverity::FsVerityError;
use crate::generation::builder::{BootRoot, rebuild_generation_image_with_boot_root};
use crate::generation::builder::test_support::{
    insert_regular_file_with_parents, persist_test_host_capabilities,
};
use crate::generation::metadata::GenerationMetadata;
use crate::generation::mount::{GenerationMountOutcome, MountOptions};
use std::cell::{Cell, RefCell};

struct RebuildRuntime {
    boot_root: BootRoot,
    enable_calls: Cell<usize>,
    mounts: RefCell<Vec<MountOptions>>,
    fail_enable: bool,
}

impl RecoveryRuntime for RebuildRuntime {
    fn rebuild(
        &self,
        conn: &Connection,
        generations_root: &Path,
        generation: i64,
        summary: &str,
    ) -> Result<crate::generation::builder::BuildResult> {
        rebuild_generation_image_with_boot_root(
            conn,
            generations_root,
            generation,
            summary,
            &self.boot_root,
        )
    }

    fn enable_verity(&self, image: &Path) -> std::result::Result<bool, FsVerityError> {
        self.enable_calls.set(self.enable_calls.get() + 1);
        assert!(image.exists());
        assert!(
            !GenerationMetadata::read_from(image.parent().unwrap())
                .unwrap()
                .fsverity_enabled
        );
        if self.fail_enable {
            Err(FsVerityError::NotSupported(image.to_path_buf()))
        } else {
            Ok(true)
        }
    }

    fn mount(&self, options: &MountOptions) -> Result<GenerationMountOutcome> {
        // Read the persisted artifact at the actual mount boundary. Merely
        // changing an in-memory flag cannot satisfy this regression.
        let metadata = GenerationMetadata::read_from(options.image_path.parent().unwrap())?;
        assert_eq!(metadata.fsverity_enabled, options.verity);
        assert!(
            metadata
                .erofs_verity_digest
                .as_ref()
                .is_some_and(|d| !d.is_empty())
        );
        if options.verity {
            assert_eq!(options.digest, metadata.erofs_verity_digest);
        } else {
            assert_eq!(options.digest, None);
        }
        self.mounts.borrow_mut().push(options.clone());
        Ok(if options.verity {
            GenerationMountOutcome::ComposefsVerity
        } else {
            GenerationMountOutcome::ComposefsPlain
        })
    }
}

fn fixture(damaged: bool) -> (TempDir, Connection, TransactionEngine, RebuildRuntime) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let boot_root = root.join("boot");
    std::fs::create_dir_all(boot_root.join("EFI/BOOT")).unwrap();
    std::fs::write(boot_root.join("vmlinuz-6.19.8-conary"), b"kernel").unwrap();
    std::fs::write(boot_root.join("initramfs-6.19.8-conary.img"), b"initramfs").unwrap();
    std::fs::write(boot_root.join("EFI/BOOT/BOOTX64.EFI"), b"efi").unwrap();
    let conn = Connection::open_in_memory().unwrap();
    crate::db::schema::ensure_current(&conn).unwrap();
    persist_test_host_capabilities(&conn);
    let cas = crate::filesystem::CasStore::new(root.join("objects")).unwrap();
    let mut trove = crate::db::models::Trove::new(
        "kernel-core".into(),
        "6.19.8-conary".into(),
        crate::db::models::TroveType::Package,
        crate::repository::versioning::VersionScheme::Conary,
    );
    trove.architecture = Some("x86_64".into());
    let trove_id = trove.insert(&conn).unwrap();
    insert_regular_file_with_parents(
        &conn,
        "/sbin/init",
        cas.store(b"init").unwrap(),
        4,
        0o100755,
        trove_id,
    );
    let engine = TransactionEngine::new(crate::transaction::TransactionConfig::new(root)).unwrap();
    std::fs::create_dir_all(root.join("generations/7")).unwrap();
    if damaged {
        std::fs::write(root.join("generations/7/root.erofs"), b"damaged").unwrap();
    }
    crate::generation::mount::update_current_symlink(root, 7).unwrap();
    let runtime = RebuildRuntime {
        boot_root: BootRoot::Staged(boot_root),
        enable_calls: Cell::new(0),
        mounts: RefCell::new(Vec::new()),
        fail_enable: false,
    };
    (tmp, conn, engine, runtime)
}

#[test]
fn rebuild_then_mount_persists_the_active_policy() {
    for cmdline in ["quiet", "conary.verity=on", "conary.verity=off"] {
        for damaged in [false, true] {
            let (tmp, conn, engine, runtime) = fixture(damaged);
            let policy = VerityPolicy::from_kernel_cmdline(cmdline);
            engine
                .recover_boot_selection_with_runtime(&conn, &policy, &runtime)
                .unwrap();
            let verified = policy.requires_verification().unwrap();
            assert_eq!(runtime.enable_calls.get(), usize::from(verified));
            assert_eq!(runtime.mounts.borrow().len(), 1);
            assert_eq!(runtime.mounts.borrow()[0].verity, verified);
            assert_eq!(policy.warning().is_some(), !verified);
            assert_eq!(
                crate::generation::mount::current_generation(tmp.path()).unwrap(),
                Some(7)
            );
            let metadata =
                GenerationMetadata::read_from(&tmp.path().join("generations/7")).unwrap();
            assert_eq!(metadata.fsverity_enabled, verified);
        }
    }
}

#[test]
fn failed_rebuild_enablement_never_mounts_or_advertises_verity() {
    let (tmp, conn, engine, mut runtime) = fixture(false);
    runtime.fail_enable = true;
    let error = engine
        .recover_boot_selection_with_runtime(&conn, &VerityPolicy::Verified, &runtime)
        .unwrap_err();
    assert!(error.to_string().contains("fs-verity support"));
    assert_eq!(runtime.enable_calls.get(), 1);
    assert!(runtime.mounts.borrow().is_empty());
    assert!(
        !GenerationMetadata::read_from(&tmp.path().join("generations/7"))
            .unwrap()
            .fsverity_enabled
    );
}
