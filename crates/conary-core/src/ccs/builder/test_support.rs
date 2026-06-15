// conary-core/src/ccs/builder/test_support.rs
//! Crate-private CCS builder fixtures for package verification tests.

use super::BuildResult;
use anyhow::{Context, ensure};
use std::path::Path;

pub(crate) fn minimal_build_result(name: &str, version: &str) -> BuildResult {
    use std::collections::HashMap;

    let mut manifest = crate::ccs::manifest::CcsManifest::new_minimal(name, version);
    let provenance = manifest.provenance.get_or_insert_with(Default::default);
    provenance.origin_class = Some("native-built".to_string());
    provenance.hardening_level = Some("hermetic".to_string());
    provenance.merkle_root = Some("sha256:empty".to_string());
    provenance.hermetic_evidence =
        Some(crate::ccs::attestation::test_support::sample_hermetic_evidence_for_tests());

    BuildResult {
        manifest,
        components: HashMap::new(),
        files: Vec::new(),
        blobs: HashMap::new(),
        total_size: 0,
        chunked: false,
        chunk_stats: None,
    }
}

pub(crate) fn rewrite_manifest_toml_for_tests<F>(
    from: &Path,
    to: &Path,
    mutate: F,
) -> anyhow::Result<()>
where
    F: FnOnce(String) -> String,
{
    use flate2::Compression;
    use flate2::read::GzDecoder;
    use flate2::write::GzEncoder;
    use std::fs::File;
    use std::io::Read;
    use tar::{Archive, Builder};

    let input = File::open(from)
        .with_context(|| format!("failed to open source package {}", from.display()))?;
    let decoder = GzDecoder::new(input);
    let mut archive = Archive::new(decoder);
    let output = File::create(to)
        .with_context(|| format!("failed to create rewritten package {}", to.display()))?;
    let encoder = GzEncoder::new(output, Compression::default());
    let mut builder = Builder::new(encoder);
    let mut mutate = Some(mutate);

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_path_buf();
        let mut header = entry.header().clone();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;

        let normalized_path = path.strip_prefix("./").unwrap_or(&path);
        if normalized_path == Path::new("MANIFEST.toml") {
            let rewrite = mutate.take().context("MANIFEST.toml was rewritten twice")?;
            bytes = rewrite(String::from_utf8(bytes)?).into_bytes();
            header.set_size(bytes.len() as u64);
            header.set_cksum();
        }

        builder.append_data(&mut header, &path, bytes.as_slice())?;
    }
    builder.finish()?;
    ensure!(
        mutate.is_none(),
        "test package did not contain MANIFEST.toml"
    );
    Ok(())
}
