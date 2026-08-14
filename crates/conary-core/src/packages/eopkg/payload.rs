// conary-core/src/packages/eopkg/payload.rs

//! Exact eopkg 1.2 inner-tar payload admission.

use super::xml::FileRecord;
use crate::ccs::ArchiveDecodeBounds;
use crate::compression::{self, CompressionFormat};
use crate::error::{Error, Result};
use crate::packages::arch::payload;
use crate::packages::payload::{PackagePayload, PayloadSpool};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

fn open_archive(path: &Path, bounds: &ArchiveDecodeBounds) -> Result<tar::Archive<Box<dyn Read>>> {
    let reader = compression::create_decoder_limited(
        File::open(path)?,
        CompressionFormat::Xz,
        bounds.max_decompressed_stream_bytes,
    )
    .map_err(|error| Error::ParseError(format!("open eopkg install.tar.xz: {error}")))?;
    Ok(tar::Archive::new(reader))
}

fn required_bytes(path: &Path, bounds: &ArchiveDecodeBounds) -> Result<u64> {
    let mut archive = open_archive(path, bounds)?;
    let mut count = 0_u64;
    let mut total = 0_u64;
    for entry in archive
        .entries()
        .map_err(|error| Error::ParseError(error.to_string()))?
    {
        count += 1;
        bounds.admit_archive_entry("eopkg payload entries", count)?;
        let entry = entry.map_err(|error| Error::ParseError(error.to_string()))?;
        total = bounds.add_payload_bytes(
            "eopkg declared payload bytes",
            total,
            payload::declared_spool_bytes(&entry),
        )?;
    }
    Ok(total)
}

pub(super) fn parse(
    path: &Path,
    records: &[FileRecord],
    bounds: &ArchiveDecodeBounds,
) -> Result<PackagePayload> {
    let required = required_bytes(path, bounds)?;
    let spool = PayloadSpool::new(required)?;
    let mut archive = open_archive(path, bounds)?;
    let mut parsed = Vec::new();
    let mut count = 0_u64;
    let mut total = 0_u64;
    for entry in archive
        .entries()
        .map_err(|error| Error::ParseError(error.to_string()))?
    {
        count += 1;
        bounds.admit_archive_entry("eopkg payload entries", count)?;
        let mut entry = entry.map_err(|error| Error::ParseError(error.to_string()))?;
        total = bounds.add_payload_bytes(
            "eopkg declared payload bytes",
            total,
            payload::declared_spool_bytes(&entry),
        )?;
        parsed.push(payload::parse_entry(
            &mut entry,
            &spool,
            usize::try_from(count)
                .map_err(|_| Error::ParseError("eopkg entry index exceeds usize".to_string()))?,
        )?);
    }
    if total != required {
        return Err(Error::ParseError(format!(
            "eopkg inner archive changed between passes: reserved {required}, observed {total}"
        )));
    }
    let mut files = payload::resolve_hardlinks(parsed)?;
    apply_files_xml_xattrs(&mut files, records)?;
    let package_payload = PackagePayload::new(files);
    verify_files_xml(&package_payload, records)?;
    Ok(package_payload)
}

fn apply_files_xml_xattrs(
    files: &mut [crate::packages::payload::PackagePayloadFile],
    records: &[FileRecord],
) -> Result<()> {
    let records = records
        .iter()
        .map(|record| {
            Ok((
                crate::packages::archive_utils::normalize_path(&record.path)?,
                record,
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    for file in files {
        let record = records.get(&file.path).ok_or_else(|| {
            Error::ParseError(format!(
                "eopkg payload path {} is absent from files.xml",
                file.path
            ))
        })?;
        let declared = record.xattrs.iter().cloned().collect::<BTreeMap<_, _>>();
        if !file.node.xattrs.is_empty() && file.node.xattrs != declared {
            return Err(Error::ParseError(format!(
                "eopkg tar and files.xml xattrs disagree for {}",
                file.path
            )));
        }
        file.node.xattrs = declared;
    }
    Ok(())
}

fn verify_files_xml(payload: &PackagePayload, records: &[FileRecord]) -> Result<()> {
    let records = records
        .iter()
        .map(|record| {
            let path =
                crate::packages::archive_utils::normalize_path(&record.path).map_err(|error| {
                    Error::ParseError(format!("unsafe eopkg files.xml path: {error}"))
                })?;
            Ok((path, record))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    if records.len() != payload.files().len() {
        return Err(Error::ParseError(format!(
            "eopkg files.xml records ({}) disagree with payload nodes ({})",
            records.len(),
            payload.files().len()
        )));
    }
    for file in payload.files() {
        let record = records.get(&file.path).ok_or_else(|| {
            Error::ParseError(format!(
                "eopkg payload path {} is absent from files.xml",
                file.path
            ))
        })?;
        let numeric = |identity: &crate::payload::PayloadIdentity| match identity {
            crate::payload::PayloadIdentity::Numeric { id } => Some(*id),
            crate::payload::PayloadIdentity::Named { .. } => None,
        };
        if record
            .mode
            .is_some_and(|mode| file.node.mode & 0o7777 != mode)
            || record
                .uid
                .is_some_and(|uid| numeric(&file.node.user) != Some(uid))
            || record
                .gid
                .is_some_and(|gid| numeric(&file.node.group) != Some(gid))
        {
            return Err(Error::ParseError(format!(
                "eopkg files.xml ownership or mode disagrees with payload for {}",
                file.path
            )));
        }
        let expected_xattrs = record.xattrs.iter().cloned().collect::<BTreeMap<_, _>>();
        if file.node.xattrs != expected_xattrs {
            return Err(Error::ParseError(format!(
                "eopkg files.xml xattrs disagree with payload for {}",
                file.path
            )));
        }
        let content_size = native_content_size(file, payload)?;
        if let Some(expected_size) = record.size
            && content_size != Some(expected_size)
        {
            return Err(Error::ParseError(format!(
                "eopkg files.xml size disagrees with payload for {}",
                file.path
            )));
        }
        if let Some(expected_sha1) = record.sha1.as_deref() {
            if expected_sha1.len() != 40
                || !expected_sha1
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            {
                return Err(Error::ParseError(format!(
                    "eopkg files.xml has invalid SHA1 for {}",
                    file.path
                )));
            }
            if native_content_sha1(file, payload)?.as_deref() != Some(expected_sha1) {
                return Err(Error::ParseError(format!(
                    "eopkg files.xml SHA1 disagrees with payload for {}",
                    file.path
                )));
            }
        }
    }
    Ok(())
}

fn native_content_size(
    file: &crate::packages::payload::PackagePayloadFile,
    payload: &PackagePayload,
) -> Result<Option<u64>> {
    match &file.node.kind {
        crate::payload::PayloadNodeKind::Regular { .. } => {
            Ok(file.content_authority.as_ref().map(|value| value.size))
        }
        crate::payload::PayloadNodeKind::Symlink { target } => Ok(Some(target.len() as u64)),
        crate::payload::PayloadNodeKind::Hardlink { target, .. } => payload
            .files()
            .iter()
            .find(|candidate| candidate.path == *target)
            .ok_or_else(|| {
                Error::ParseError(format!("eopkg hardlink {} lacks {target}", file.path))
            })
            .and_then(|target| native_content_size(target, payload)),
        _ => Ok(None),
    }
}

fn native_content_sha1(
    file: &crate::packages::payload::PackagePayloadFile,
    payload: &PackagePayload,
) -> Result<Option<String>> {
    use sha1::Digest as _;
    let mut hasher = sha1::Sha1::new();
    match &file.node.kind {
        crate::payload::PayloadNodeKind::Regular { .. } => {
            let mut reader = file.open_content()?;
            let mut buffer = [0_u8; crate::packages::payload::PAYLOAD_IO_BUFFER_SIZE];
            loop {
                let read = reader.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
            }
        }
        crate::payload::PayloadNodeKind::Symlink { target } => hasher.update(target.as_bytes()),
        crate::payload::PayloadNodeKind::Hardlink { target, .. } => {
            let target = payload
                .files()
                .iter()
                .find(|candidate| candidate.path == *target)
                .ok_or_else(|| {
                    Error::ParseError(format!("eopkg hardlink {} lacks {target}", file.path))
                })?;
            return native_content_sha1(target, payload);
        }
        _ => return Ok(None),
    }
    Ok(Some(format!("{:x}", hasher.finalize())))
}
