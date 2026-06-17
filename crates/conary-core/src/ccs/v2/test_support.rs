// conary-core/src/ccs/v2/test_support.rs

use super::schema::*;
use anyhow::Context;
use std::collections::BTreeMap;
use std::path::Path;

pub(crate) fn package_authority_with_one_file(name: &str) -> AuthorityDocumentV2 {
    AuthorityDocumentV2::package_for_tests(name)
}

pub(crate) fn one_file_payloads_for_tests() -> BTreeMap<String, Vec<u8>> {
    BTreeMap::from([("/usr/bin/hello".to_string(), b"hello world\n".to_vec())])
}

pub(crate) fn rewrite_v2_archive_for_tests<F>(
    from: &Path,
    to: &Path,
    mutate: F,
) -> anyhow::Result<()>
where
    F: FnOnce(&mut BTreeMap<String, Vec<u8>>),
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
    let mut entries = BTreeMap::new();

    for entry in archive.entries()? {
        let mut entry = entry?;
        if entry.header().entry_type().is_dir() {
            continue;
        }
        let path = entry.path()?.to_path_buf();
        let normalized = path.strip_prefix("./").unwrap_or(&path);
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        entries.insert(normalized.to_string_lossy().into_owned(), bytes);
    }

    mutate(&mut entries);

    let output = File::create(to)
        .with_context(|| format!("failed to create rewritten package {}", to.display()))?;
    let encoder = GzEncoder::new(output, Compression::default());
    let mut builder = Builder::new(encoder);
    for (path, bytes) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, path, bytes.as_slice())?;
    }
    builder.into_inner()?.finish()?;
    Ok(())
}
