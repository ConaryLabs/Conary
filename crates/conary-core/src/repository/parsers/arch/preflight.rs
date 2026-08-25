// crates/conary-core/src/repository/parsers/arch/preflight.rs

//! Read-only native source-candidate facts for an authenticated ALPM database.

use std::io::Read;
use std::path::Path;

use tar::Archive;

use super::ArchParser;
use crate::error::{Error, Result};
use crate::repository::parsers::{ArchPackageFragmentKind, RepositorySnapshotSink};

pub(super) fn preflight_database<S: RepositorySnapshotSink>(
    parser: &ArchParser,
    repo_url: &str,
    database_file: &Path,
    sink: &mut S,
) -> Result<()> {
    let decoder = super::super::common::open_metadata_decoder(
        database_file,
        &format!("Arch repository database {}", database_file.display()),
    )?;
    let mut archive = Archive::new(decoder);
    for entry in archive.entries()? {
        let mut entry = entry
            .map_err(|error| Error::ParseError(format!("Failed to read tarball entry: {error}")))?;
        let path = entry
            .path()
            .map_err(|error| Error::ParseError(format!("Invalid path in tarball: {error}")))?;
        let path = path.to_str().ok_or_else(|| {
            Error::ParseError("Arch repository entry path is not valid UTF-8".to_string())
        })?;
        let directory = path
            .split('/')
            .next()
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        if path.ends_with("/desc") {
            let mut content = String::new();
            entry
                .read_to_string(&mut content)
                .map_err(|error| Error::ParseError(format!("Failed to read desc file: {error}")))?;
            if let Some(directory) = directory.as_deref() {
                sink.preflight_arch_package_fragment(
                    directory,
                    ArchPackageFragmentKind::Desc,
                    &content,
                )?;
            }
            let fields = parser.parse_desc_file(&content)?;
            sink.preflight_package(parser.package_from_fields(repo_url, &fields, None)?)?;
        } else if path.ends_with("/depends") {
            let mut content = String::new();
            entry.read_to_string(&mut content).map_err(|error| {
                Error::ParseError(format!("Failed to read depends file: {error}"))
            })?;
            if let Some(directory) = directory.as_deref() {
                sink.preflight_arch_package_fragment(
                    directory,
                    ArchPackageFragmentKind::Depends,
                    &content,
                )?;
            }
            let fields = parser.parse_desc_file(&content)?;
            let mut groups = parser.parse_structured_depends(&content)?;
            groups.extend(parser.parse_relation_fields(&[&fields])?);
            sink.preflight_requirement_groups(groups)?;
        }
    }
    Ok(())
}
