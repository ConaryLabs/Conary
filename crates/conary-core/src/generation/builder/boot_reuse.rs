// crates/conary-core/src/generation/builder/boot_reuse.rs

//! Exact eligibility for reusing an already-published boot artifact set.

use std::path::Path;

use tracing::info;

use super::boot_assets::{RuntimeBootAssetSources, RuntimeBootAssetStaging};
use super::cas::artifact_root_for_generations_root;
use crate::db::models::{ActivationRequest, GenerationPublication};
use crate::generation::artifact::load_verified_generation_boot_assets;

pub(super) fn resolve_reusable_boot_assets(
    conn: &rusqlite::Connection,
    generations_root: &Path,
) -> crate::Result<Option<RuntimeBootAssetSources>> {
    let artifact_root = artifact_root_for_generations_root(generations_root)?;
    let Some(current_generation) = crate::generation::mount::current_generation(&artifact_root)?
    else {
        return Ok(None);
    };
    let Some(publication) =
        GenerationPublication::completed_for_generation(conn, current_generation)?
    else {
        return Ok(None);
    };
    let current_high_water = GenerationPublication::applied_high_water_changeset_id(conn)?;
    if let (Some(previous), Some(current)) = (
        publication.published_through_changeset_id,
        current_high_water,
    ) && current < previous
    {
        return Err(crate::Error::ConflictError(format!(
            "current applied changeset high-water {current} precedes generation {current_generation} publication high-water {previous}"
        )));
    }
    if let Some(through) = current_high_water
        && ActivationRequest::has_applied_boot_runtime_between(
            conn,
            publication.published_through_changeset_id,
            through,
        )?
    {
        return Ok(None);
    }
    if current_high_water.is_none() && publication.published_through_changeset_id.is_some() {
        return Err(crate::Error::ConflictError(format!(
            "generation {current_generation} has a published changeset high-water but the current database has none"
        )));
    }

    let verified = load_verified_generation_boot_assets(
        &generations_root.join(current_generation.to_string()),
    )?;
    let kernel = verified.kernel_path()?;
    let initramfs = verified.initramfs_path()?;
    let efi_bootloader = verified.efi_bootloader_path()?;
    let kernel_version = verified.kernel_version().to_string();
    info!(
        source_generation = verified.generation(),
        publication_high_water = publication.published_through_changeset_id,
        current_high_water,
        "Reusing verified boot assets because no applied boot-runtime mutation request changed their authority"
    );

    Ok(Some(RuntimeBootAssetSources {
        kernel_version,
        kernel,
        initramfs,
        efi_bootloader,
        _sysroot_workspace: None,
        staging: RuntimeBootAssetStaging::Reuse(Box::new(verified)),
    }))
}

#[cfg(all(test, feature = "composefs-rs"))]
mod tests {
    use super::super::test_support::{
        insert_regular_file_with_parents, persist_test_host_capabilities,
    };
    use super::*;
    use crate::activation::{ActivationExecutableIdentity, BootRuntimeActivationInvocation};
    use crate::db::models::{
        ActivationRequestSourceKind, Changeset, ChangesetStatus, GenerationPublicationPhase,
        GenerationPublicationStatus, NewActivationRequest, Trove, TroveType,
    };
    use crate::filesystem::CasStore;
    use std::os::unix::fs::MetadataExt;

    struct PublishedFixture {
        _temp: tempfile::TempDir,
        conn: rusqlite::Connection,
        generations_root: std::path::PathBuf,
        generation: i64,
    }

    fn published_fixture() -> PublishedFixture {
        let temp = tempfile::tempdir().unwrap();
        let generations_root = temp.path().join("generations");
        let objects_dir = temp.path().join("objects");
        let boot_root = temp.path().join("boot");
        std::fs::create_dir_all(&generations_root).unwrap();
        std::fs::create_dir_all(boot_root.join("EFI/BOOT")).unwrap();
        std::fs::write(boot_root.join("vmlinuz-6.20.0-conary"), b"kernel").unwrap();
        std::fs::write(boot_root.join("initramfs-6.20.0-conary.img"), b"initramfs").unwrap();
        std::fs::write(boot_root.join("EFI/BOOT/BOOTX64.EFI"), b"efi").unwrap();

        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::schema::ensure_current(&conn).unwrap();
        persist_test_host_capabilities(&conn);
        let cas = CasStore::new(&objects_dir).unwrap();
        let init = cas.store(b"init").unwrap();
        let hello = cas.store(b"hello").unwrap();
        let mut trove = Trove::new(
            "base-runtime".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            crate::repository::versioning::VersionScheme::Conary,
        );
        trove.architecture = Some("x86_64".to_string());
        let trove_id = trove.insert(&conn).unwrap();
        insert_regular_file_with_parents(&conn, "/sbin/init", init, b"init".len(), 0o755, trove_id);
        insert_regular_file_with_parents(
            &conn,
            "/usr/bin/hello",
            hello,
            b"hello".len(),
            0o755,
            trove_id,
        );

        let (generation, _) = super::super::create::build_generation_from_db_with_boot_root(
            &conn,
            &generations_root,
            "initial exact generation",
            &boot_root,
        )
        .unwrap();
        let publication = GenerationPublication::create_pending(
            &conn,
            None,
            None,
            "/tmp/conary.db",
            temp.path().to_str().unwrap(),
            "initial exact generation",
            &Default::default(),
        )
        .unwrap();
        publication
            .set_phase(
                &conn,
                GenerationPublicationPhase::DatabaseBackedUp,
                GenerationPublicationStatus::Running,
                Some(generation),
                Some(generation),
            )
            .unwrap();
        publication
            .mark_complete_through(&conn, None, generation, generation)
            .unwrap();
        crate::generation::mount::update_current_symlink(temp.path(), generation).unwrap();

        PublishedFixture {
            _temp: temp,
            conn,
            generations_root,
            generation,
        }
    }

    fn apply_changeset(conn: &rusqlite::Connection, boot_runtime: bool) -> i64 {
        let mut changeset = Changeset::new("ordinary package mutation".to_string());
        let changeset_id = changeset.insert(conn).unwrap();
        if boot_runtime {
            let invocation = BootRuntimeActivationInvocation::new(
                "depmod",
                vec!["-a".to_string(), "6.20.0-conary".to_string()],
                ActivationExecutableIdentity {
                    invoked_path: "/usr/sbin/depmod".to_string(),
                    canonical_path: "/usr/bin/kmod".to_string(),
                    sha256: format!("sha256:{}", "a".repeat(64)),
                },
            )
            .unwrap();
            crate::db::models::ActivationRequest::append_batch(
                conn,
                changeset_id,
                &[NewActivationRequest {
                    source_kind: ActivationRequestSourceKind::CapturedBootRuntime,
                    source_package: "kernel-core".to_string(),
                    source_version: "6.20.0-conary".to_string(),
                    source_entry: "rpm:%posttrans".to_string(),
                    invocation: invocation.into(),
                }],
            )
            .unwrap();
        }
        changeset
            .update_status(conn, ChangesetStatus::Applied)
            .unwrap();
        changeset_id
    }

    #[test]
    fn ordinary_changeset_reuses_verified_boot_assets_without_a_sysroot() {
        let fixture = published_fixture();
        apply_changeset(&fixture.conn, false);

        let (next, _) = super::super::create::build_generation_from_db_with_boot_root(
            &fixture.conn,
            &fixture.generations_root,
            "ordinary package mutation",
            Path::new("/boot"),
        )
        .unwrap();

        let prior_kernel = fixture
            .generations_root
            .join(fixture.generation.to_string())
            .join("boot-assets/vmlinuz");
        let next_kernel = fixture
            .generations_root
            .join(next.to_string())
            .join("boot-assets/vmlinuz");
        assert_eq!(
            std::fs::metadata(prior_kernel).unwrap().ino(),
            std::fs::metadata(next_kernel).unwrap().ino(),
            "unchanged verified boot assets should be linked, not copied"
        );
    }

    #[test]
    fn applied_boot_runtime_request_forces_exact_rebuild_path() {
        let fixture = published_fixture();
        apply_changeset(&fixture.conn, true);

        let error = super::super::create::build_generation_from_db_with_boot_root(
            &fixture.conn,
            &fixture.generations_root,
            "kernel package mutation",
            Path::new("/boot"),
        )
        .unwrap_err()
        .to_string();

        assert!(
            error.contains("generation-sysroot-workspace")
                || error.contains("generation boot root")
                || error.contains("boot"),
            "the deliberately unprivileged fixture should reach the sysroot rebuild path: {error}"
        );
    }

    #[test]
    fn corrupted_prior_boot_asset_fails_closed_before_reuse() {
        let fixture = published_fixture();
        apply_changeset(&fixture.conn, false);
        std::fs::write(
            fixture
                .generations_root
                .join(fixture.generation.to_string())
                .join("boot-assets/initramfs.img"),
            b"tampered",
        )
        .unwrap();

        let error = super::super::create::build_generation_from_db_with_boot_root(
            &fixture.conn,
            &fixture.generations_root,
            "ordinary package mutation",
            Path::new("/boot"),
        )
        .unwrap_err();

        assert!(matches!(error, crate::Error::ChecksumMismatch { .. }));
        assert_eq!(
            std::fs::read(
                fixture
                    .generations_root
                    .join(fixture.generation.to_string())
                    .join("boot-assets/initramfs.img")
            )
            .unwrap(),
            b"tampered"
        );
    }
}
