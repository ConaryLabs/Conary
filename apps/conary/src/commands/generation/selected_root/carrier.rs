// apps/conary/src/commands/generation/selected_root/carrier.rs

//! Current-generation carrier selection for ordinary package transactions.

use conary_core::generation::artifact::GenerationArtifact;
use conary_core::generation::composefs::ComposefsRuntimeUnavailable;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CurrentGenerationLowerMode {
    /// Explicit try/test materialization never probes or depends on host mount
    /// capabilities.
    MaterializedExplicit,
    /// The host can mount the verified generation image directly.
    DirectComposefs,
    /// The ordinary package loop remains available by materializing the same
    /// verified artifact authority when the composefs carrier is unavailable.
    MaterializedUnavailable(ComposefsRuntimeUnavailable),
}

impl CurrentGenerationLowerMode {
    pub(super) fn load_artifact(
        &self,
        generation_path: &Path,
    ) -> conary_core::Result<GenerationArtifact> {
        match self {
            Self::MaterializedExplicit => {
                conary_core::generation::artifact::load_generation_artifact(generation_path)
            }
            Self::DirectComposefs | Self::MaterializedUnavailable(_) => {
                conary_core::generation::artifact::load_generation_artifact_with_verified_cas(
                    generation_path,
                )
            }
        }
    }

    pub(super) const fn requires_materialization(&self) -> bool {
        !matches!(self, Self::DirectComposefs)
    }

    pub(super) fn record_materialized_fallback(&self, generation: i64) {
        if let Self::MaterializedUnavailable(unavailable) = self {
            tracing::info!(
                generation,
                reason = unavailable.code(),
                recovery = unavailable.recovery(),
                "materializing verified current generation for the basic package loop because the direct composefs carrier is unavailable"
            );
        }
    }
}

pub(super) fn current_generation_lower_mode(
    require_materialized: bool,
    probe: impl FnOnce() -> std::result::Result<PathBuf, ComposefsRuntimeUnavailable>,
) -> CurrentGenerationLowerMode {
    if require_materialized {
        return CurrentGenerationLowerMode::MaterializedExplicit;
    }

    match probe() {
        Ok(_) => CurrentGenerationLowerMode::DirectComposefs,
        Err(unavailable) => CurrentGenerationLowerMode::MaterializedUnavailable(unavailable),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::generation::artifact::{
        ArtifactWriteInputs, BootAssetsManifest, CasObjectVerification, write_generation_artifact,
    };
    use conary_core::generation::metadata::GenerationMetadata;
    use conary_core::generation::root_manifest::{
        GenerationRootEntry, GenerationRootManifest, build_erofs_image_from_root_manifest,
    };
    use conary_core::payload::{
        PayloadContentAuthority, PayloadIdentity, PayloadNode, PayloadNodeKind, ResolvedPayloadNode,
    };
    use conary_core::runtime_root::ConaryRuntimeRoot;
    use std::path::Path;

    #[test]
    fn missing_composefs_helper_selects_the_verified_materialized_lower() {
        let mode =
            current_generation_lower_mode(false, || Err(ComposefsRuntimeUnavailable::MountHelper));

        assert_eq!(
            mode,
            CurrentGenerationLowerMode::MaterializedUnavailable(
                ComposefsRuntimeUnavailable::MountHelper
            )
        );
    }

    #[test]
    fn available_composefs_runtime_selects_the_direct_generation_lower() {
        let mode =
            current_generation_lower_mode(false, || Ok(PathBuf::from("/usr/bin/mount.composefs")));

        assert_eq!(mode, CurrentGenerationLowerMode::DirectComposefs);
    }

    #[test]
    fn explicit_materialization_does_not_probe_the_host_carrier() {
        let mode = current_generation_lower_mode(true, || {
            panic!("explicit materialization must not probe the host carrier")
        });

        assert_eq!(mode, CurrentGenerationLowerMode::MaterializedExplicit);
    }

    fn prepare_current_generation_fixture(runtime_root: &Path) {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/selected-root-current-generation/prepare.py");
        let status = std::process::Command::new("python3")
            .arg(fixture)
            .arg(runtime_root)
            .status()
            .unwrap();
        assert!(status.success());
    }

    fn rewrite_fixture_ownership_for_current_user(runtime_root: &Path) {
        let generation_dir = runtime_root.join("generations/1");
        let mut metadata = GenerationMetadata::read_from(&generation_dir).unwrap();
        let mut manifest = GenerationRootManifest::read_from(&generation_dir).unwrap();
        let uid = u64::from(unsafe { libc::geteuid() });
        let gid = u64::from(unsafe { libc::getegid() });
        let resolved = |kind: PayloadNodeKind, mode: u32| {
            let mut source = PayloadNode::regular(mode);
            source.kind = kind;
            source.mode = match &source.kind {
                PayloadNodeKind::Directory => libc::S_IFDIR | mode,
                _ => libc::S_IFREG | mode,
            };
            source.user = PayloadIdentity::Numeric { id: uid };
            source.group = PayloadIdentity::Numeric { id: gid };
            ResolvedPayloadNode::from_numeric_source(source).unwrap()
        };
        manifest.root.source.user = PayloadIdentity::Numeric { id: uid };
        manifest.root.source.group = PayloadIdentity::Numeric { id: gid };
        manifest.root.uid = uid;
        manifest.root.gid = gid;
        let content = b"verified carrier fallback authority";
        let cas = conary_core::filesystem::CasStore::new(runtime_root.join("objects")).unwrap();
        let sha256 = cas.store(content).unwrap();
        manifest.entries = vec![
            GenerationRootEntry {
                path: "/usr".to_string(),
                node: resolved(PayloadNodeKind::Directory, 0o755),
                content: None,
            },
            GenerationRootEntry {
                path: "/usr/bin".to_string(),
                node: resolved(PayloadNodeKind::Directory, 0o755),
                content: None,
            },
            GenerationRootEntry {
                path: "/usr/bin/carrier-fallback-proof".to_string(),
                node: resolved(
                    PayloadNodeKind::Regular {
                        hardlink_identity: None,
                    },
                    0o644,
                ),
                content: Some(PayloadContentAuthority {
                    sha256,
                    size: content.len() as u64,
                }),
            },
        ];
        let build = build_erofs_image_from_root_manifest(&manifest, &generation_dir).unwrap();
        metadata.erofs_size = Some(i64::try_from(build.image_size).unwrap());
        metadata.cas_objects_referenced =
            Some(i64::try_from(build.cas_objects_referenced).unwrap());

        let boot_assets: BootAssetsManifest = serde_json::from_slice(
            &std::fs::read(generation_dir.join("boot-assets/manifest.json")).unwrap(),
        )
        .unwrap();
        let artifact_digest = write_generation_artifact(ArtifactWriteInputs {
            generation_dir: &generation_dir,
            generation: 1,
            architecture: "x86_64",
            erofs_path: &generation_dir.join("root.erofs"),
            cas_base_rel: "../../objects",
            cas_verification: CasObjectVerification::Deep,
            boot_assets,
            carrier_capabilities: Default::default(),
        })
        .unwrap();
        metadata.artifact_manifest_sha256 = Some(artifact_digest);
        metadata.write_to(&generation_dir).unwrap();
    }

    #[test]
    fn unavailable_composefs_runtime_materializes_the_verified_current_generation() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("conary.db");
        conary_core::db::init(&db_path).unwrap();
        prepare_current_generation_fixture(temp.path());
        rewrite_fixture_ownership_for_current_user(temp.path());
        let conn = conary_core::db::open(&db_path).unwrap();
        let runtime_root = ConaryRuntimeRoot::from_db_path(&db_path);
        let session_dir = temp.path().join("selected-root-sessions/fallback");

        let prepared = super::super::prepare_current_root_with_probe(
            &conn,
            &runtime_root,
            &session_dir,
            false,
            || Err(ComposefsRuntimeUnavailable::MountHelper),
        )
        .unwrap();

        assert!(matches!(
            prepared,
            super::super::PreparedSelectedRoot::Materialized { .. }
        ));
        assert!(session_dir.join("lower").is_dir());
        assert!(!session_dir.join("root").exists());
        assert_eq!(
            std::fs::read_to_string(session_dir.join("lower/usr/bin/carrier-fallback-proof"))
                .unwrap(),
            "verified carrier fallback authority"
        );
    }

    #[test]
    fn unavailable_composefs_runtime_does_not_bypass_artifact_verification() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("conary.db");
        conary_core::db::init(&db_path).unwrap();
        prepare_current_generation_fixture(temp.path());
        std::fs::write(temp.path().join("generations/1/root.erofs"), b"tampered").unwrap();
        let conn = conary_core::db::open(&db_path).unwrap();
        let runtime_root = ConaryRuntimeRoot::from_db_path(&db_path);
        let session_dir = temp.path().join("selected-root-sessions/tampered");

        let error = match super::super::prepare_current_root_with_probe(
            &conn,
            &runtime_root,
            &session_dir,
            false,
            || Err(ComposefsRuntimeUnavailable::MountHelper),
        ) {
            Ok(_) => panic!("tampered generation artifact must fail before materialization"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("root.erofs"));
        assert!(!session_dir.exists());
    }
}
