// conary-core/src/packages/deb/payload.rs

//! Exact parser for the `data.tar` subset accepted by dpkg.
//!
//! dpkg does not accept arbitrary tar extensions. Its current grammar accepts
//! v7, ustar, and the GNU long-name/long-link records, while rejecting PAX,
//! GNU sparse, and unknown entry types. Keeping this parser explicit prevents
//! the generic tar library from silently applying unsupported extensions.

use crate::ccs::{ArchiveDecodeBounds, CCS_BUDGET};
use crate::compression;
use crate::error::{Error, Result};
use crate::hash::{HashAlgorithm, Hasher};
use crate::packages::archive_utils::normalize_path;
use crate::packages::payload::{
    PAYLOAD_IO_BUFFER_SIZE, PackagePayload, PackagePayloadFile, PayloadSpool, ReopenablePayload,
};
#[cfg(test)]
use crate::packages::traits::{ExtractedFile, PackageFile};
use crate::payload::{
    PayloadContentAuthority, PayloadIdentity, PayloadNode, PayloadNodeKind, PayloadTimestamp,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Component, Path};
#[cfg(test)]
use std::sync::Arc;

const TAR_BLOCK_SIZE: usize = 512;
const TAR_NAME: std::ops::Range<usize> = 0..100;
const TAR_MODE: std::ops::Range<usize> = 100..108;
const TAR_UID: std::ops::Range<usize> = 108..116;
const TAR_GID: std::ops::Range<usize> = 116..124;
const TAR_SIZE: std::ops::Range<usize> = 124..136;
const TAR_MTIME: std::ops::Range<usize> = 136..148;
const TAR_CHECKSUM: std::ops::Range<usize> = 148..156;
const TAR_TYPEFLAG: usize = 156;
const TAR_LINK_NAME: std::ops::Range<usize> = 157..257;
const TAR_MAGIC: std::ops::Range<usize> = 257..263;
const TAR_DEVICE_MAJOR: std::ops::Range<usize> = 329..337;
const TAR_DEVICE_MINOR: std::ops::Range<usize> = 337..345;
const TAR_PREFIX: std::ops::Range<usize> = 345..500;

const GNU_MAGIC: &[u8; 6] = b"ustar ";
const USTAR_MAGIC: &[u8; 6] = b"ustar\0";

const TYPE_REGULAR_OLD: u8 = 0;
const TYPE_REGULAR: u8 = b'0';
const TYPE_HARDLINK: u8 = b'1';
const TYPE_SYMLINK: u8 = b'2';
const TYPE_CHARACTER_DEVICE: u8 = b'3';
const TYPE_BLOCK_DEVICE: u8 = b'4';
const TYPE_DIRECTORY: u8 = b'5';
const TYPE_FIFO: u8 = b'6';
const TYPE_GNU_LONG_LINK: u8 = b'K';
const TYPE_GNU_LONG_NAME: u8 = b'L';

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TarFormat {
    Old,
    Ustar,
    Gnu,
}

#[derive(Debug)]
struct Header {
    path: Vec<u8>,
    link_name: Vec<u8>,
    entry_type: u8,
    mode: u32,
    uid: u64,
    gid: u64,
    size: u64,
    mtime: i64,
    device_major: Option<u64>,
    device_minor: Option<u64>,
}

#[derive(Debug)]
struct ParsedEntry {
    path: String,
    node: PayloadNode,
    content_authority: Option<PayloadContentAuthority>,
    source: Option<ReopenablePayload>,
}

#[cfg(test)]
pub(super) fn parse_package_files(data_tar_data: &[u8]) -> Result<Vec<PackageFile>> {
    let bounds = CCS_BUDGET.archive_decode_bounds()?;
    let mut metrics = crate::packages::NativePackageParseMetrics::default();
    parse_reader(
        open_bytes_decoder(data_tar_data, &bounds)?,
        None,
        &bounds,
        &mut metrics,
    )
    .map(|entries| {
        entries
            .into_iter()
            .map(|entry| PackageFile {
                path: entry.path,
                node: entry.node,
                content: entry.content_authority,
            })
            .collect()
    })
}

#[cfg(test)]
pub(super) fn extract_files(data_tar_data: &[u8]) -> Result<Vec<ExtractedFile>> {
    let source = ReopenablePayload::from_in_memory_bytes(Arc::<[u8]>::from(data_tar_data.to_vec()));
    parse_package_payload(&source)?.to_extracted_in_memory()
}

#[cfg(test)]
pub(super) fn parse_package_payload(data_tar: &ReopenablePayload) -> Result<PackagePayload> {
    let mut metrics = crate::packages::NativePackageParseMetrics::default();
    parse_package_payload_with_metrics(data_tar, &mut metrics)
}

pub(super) fn parse_package_payload_with_metrics(
    data_tar: &ReopenablePayload,
    metrics: &mut crate::packages::NativePackageParseMetrics,
) -> Result<PackagePayload> {
    let bounds = CCS_BUDGET.archive_decode_bounds()?;
    let compressed_bytes = crate::packages::parse_metrics::ReadCounter::default();
    let decompressed_bytes = crate::packages::parse_metrics::ReadCounter::default();
    let metadata = parse_reader(
        open_source_decoder(data_tar, &bounds, &compressed_bytes, &decompressed_bytes)?,
        None,
        &bounds,
        metrics,
    )?;
    let required_bytes = required_spool_bytes(&metadata, &bounds)?;
    let spool = PayloadSpool::new(required_bytes)?;
    let parsed = parse_reader(
        open_source_decoder(data_tar, &bounds, &compressed_bytes, &decompressed_bytes)?,
        Some(&spool),
        &bounds,
        metrics,
    )?;
    if metadata.len() != parsed.len()
        || metadata.iter().zip(&parsed).any(|(first, second)| {
            first.path != second.path
                || first.node != second.node
                || first.content_authority != second.content_authority
        })
    {
        return Err(data_tar_error(
            "DEB data tar changed between metadata and payload passes",
        ));
    }
    let payload_files_spooled =
        u64::try_from(parsed.iter().filter(|entry| entry.source.is_some()).count())
            .map_err(|_| data_tar_error("DEB payload spool file count exceeds u64"))?;
    let payload_bytes_hashed = required_bytes
        .checked_mul(2)
        .ok_or_else(|| data_tar_error("DEB payload hash byte count overflow"))?;
    metrics.checked_add(crate::packages::NativePackageParseMetrics {
        decompressed_archive_bytes_read: decompressed_bytes.bytes(),
        intermediate_archive_bytes_read: compressed_bytes.bytes(),
        payload_files_spooled,
        payload_bytes_spooled: required_bytes,
        payload_spool_file_syncs: payload_files_spooled,
        payload_bytes_hashed,
        ..Default::default()
    })?;
    parsed
        .into_iter()
        .map(|entry| {
            PackagePayloadFile::new(
                entry.path,
                entry.node,
                entry.content_authority,
                entry.source,
            )
        })
        .collect::<Result<Vec<_>>>()
        .map(PackagePayload::new)
}

fn required_spool_bytes(entries: &[ParsedEntry], bounds: &ArchiveDecodeBounds) -> Result<u64> {
    entries.iter().try_fold(0_u64, |total, entry| {
        bounds
            .add_payload_bytes(
                "DEB data.tar declared payload bytes",
                total,
                entry
                    .content_authority
                    .as_ref()
                    .map_or(0, |authority| authority.size),
            )
            .map_err(Into::into)
    })
}

#[cfg(test)]
fn open_bytes_decoder<'a>(
    data: &'a [u8],
    bounds: &ArchiveDecodeBounds,
) -> Result<Box<dyn Read + 'a>> {
    let format = crate::compression::CompressionFormat::from_magic_bytes(data);
    compression::create_decoder_limited(data, format, bounds.max_decompressed_stream_bytes)
        .map_err(|error| data_tar_error(format!("failed to create decoder: {error}")))
}

fn open_source_decoder(
    source: &ReopenablePayload,
    bounds: &ArchiveDecodeBounds,
    compressed_bytes: &crate::packages::parse_metrics::ReadCounter,
    decompressed_bytes: &crate::packages::parse_metrics::ReadCounter,
) -> Result<Box<dyn Read>> {
    let mut probe = compressed_bytes.wrap(source.open()?);
    let mut magic = [0_u8; 8];
    let read = probe.read(&mut magic)?;
    let format = crate::compression::CompressionFormat::from_magic_bytes(&magic[..read]);
    let decoder = compression::create_decoder_limited(
        compressed_bytes.wrap(source.open()?),
        format,
        bounds.max_decompressed_stream_bytes,
    )
    .map_err(|error| data_tar_error(format!("failed to create decoder: {error}")))?;
    Ok(Box::new(decompressed_bytes.wrap(decoder)))
}

fn parse_reader(
    mut reader: Box<dyn Read + '_>,
    spool: Option<&PayloadSpool>,
    bounds: &ArchiveDecodeBounds,
    metrics: &mut crate::packages::NativePackageParseMetrics,
) -> Result<Vec<ParsedEntry>> {
    let mut entries = Vec::<ParsedEntry>::new();
    let mut paths = BTreeSet::<String>::new();
    let mut pending_long_name: Option<Vec<u8>> = None;
    let mut pending_long_link: Option<Vec<u8>> = None;
    let mut entries_seen = 0_u64;
    let mut declared_payload_bytes = 0_u64;
    let mut metadata_bytes = 0_u64;
    let mut archive_root_seen = false;

    while let Some(block) = read_header_block(&mut reader)? {
        if block.iter().all(|byte| *byte == 0) {
            break;
        }
        entries_seen += 1;
        bounds.admit_archive_entry("DEB data.tar entries", entries_seen)?;

        let mut header = decode_header(&block)?;
        match header.entry_type {
            TYPE_GNU_LONG_NAME | TYPE_GNU_LONG_LINK => {
                let value = read_long_value(
                    &mut reader,
                    header.size,
                    header.entry_type,
                    bounds,
                    &mut metadata_bytes,
                )?;
                if header.entry_type == TYPE_GNU_LONG_NAME {
                    pending_long_name = Some(value);
                } else {
                    pending_long_link = Some(value);
                }
                continue;
            }
            _ => {
                if let Some(long_name) = pending_long_name.take() {
                    header.path = long_name;
                }
                if let Some(long_link) = pending_long_link.take() {
                    header.link_name = long_link;
                }
            }
        }

        let raw_path = utf8_field(&header.path, "entry path")?;
        let directory_from_trailing_slash =
            matches!(header.entry_type, TYPE_REGULAR_OLD | TYPE_REGULAR) && raw_path.ends_with('/');
        if is_archive_root_path(raw_path) {
            if header.entry_type != TYPE_DIRECTORY && !directory_from_trailing_slash {
                return Err(data_tar_error(format!(
                    "archive root entry {raw_path:?} is not a directory"
                )));
            }
            require_zero_size("/", header.size, "archive root directory")?;
            if archive_root_seen {
                return Err(data_tar_error(
                    "duplicate archive root directory is not representable",
                ));
            }
            archive_root_seen = true;
            continue;
        }
        let raw_path = if directory_from_trailing_slash {
            raw_path.trim_end_matches('/')
        } else {
            raw_path
        };
        let path = normalize_path(raw_path)
            .map_err(|error| data_tar_error(format!("invalid path {raw_path:?}: {error}")))?;
        if !paths.insert(path.clone()) {
            return Err(data_tar_error(format!(
                "duplicate payload path {path:?} is not representable"
            )));
        }

        let permissions = header.mode & 0o7777;
        let mut content_authority = None;
        let mut source = None;
        let kind = match header.entry_type {
            TYPE_REGULAR_OLD | TYPE_REGULAR if !directory_from_trailing_slash => {
                declared_payload_bytes = bounds.add_payload_bytes(
                    "DEB data.tar declared payload bytes",
                    declared_payload_bytes,
                    header.size,
                )?;
                let (authority, payload_source) =
                    read_regular_content(&mut reader, &path, header.size, spool, entries.len())?;
                content_authority = Some(authority);
                source = payload_source;
                PayloadNodeKind::Regular {
                    hardlink_identity: None,
                }
            }
            TYPE_REGULAR | TYPE_DIRECTORY => {
                require_zero_size(&path, header.size, "directory")?;
                PayloadNodeKind::Directory
            }
            TYPE_SYMLINK => {
                require_zero_size(&path, header.size, "symlink")?;
                PayloadNodeKind::Symlink {
                    target: required_link_target(&header.link_name, &path, "symlink")?,
                }
            }
            TYPE_HARDLINK => {
                require_zero_size(&path, header.size, "hardlink")?;
                let raw_target = required_link_target(&header.link_name, &path, "hardlink")?;
                let target = normalize_path(&raw_target).map_err(|error| {
                    data_tar_error(format!(
                        "invalid hardlink target {raw_target:?} for {path:?}: {error}"
                    ))
                })?;
                PayloadNodeKind::Hardlink {
                    identity: format!("path:{target}"),
                    target,
                }
            }
            TYPE_BLOCK_DEVICE => {
                require_zero_size(&path, header.size, "block device")?;
                PayloadNodeKind::BlockDevice {
                    major: header.device_major.expect("decoded block-device major"),
                    minor: header.device_minor.expect("decoded block-device minor"),
                }
            }
            TYPE_CHARACTER_DEVICE => {
                require_zero_size(&path, header.size, "character device")?;
                PayloadNodeKind::CharacterDevice {
                    major: header.device_major.expect("decoded character-device major"),
                    minor: header.device_minor.expect("decoded character-device minor"),
                }
            }
            TYPE_FIFO => {
                require_zero_size(&path, header.size, "FIFO")?;
                PayloadNodeKind::Fifo
            }
            unsupported => {
                return Err(data_tar_error(format!(
                    "unsupported tar header type {}",
                    display_typeflag(unsupported)
                )));
            }
        };

        let mode_type = mode_type(&kind);
        let node = PayloadNode {
            kind,
            mode: mode_type | permissions,
            user: PayloadIdentity::Numeric { id: header.uid },
            group: PayloadIdentity::Numeric { id: header.gid },
            mtime: PayloadTimestamp {
                seconds: header.mtime,
                nanoseconds: 0,
            },
            xattrs: BTreeMap::new(),
        };
        node.validate()
            .map_err(|error| data_tar_error(format!("invalid node {path:?}: {error}")))?;
        node.validate_content(content_authority.as_ref())
            .map_err(|error| data_tar_error(format!("invalid content {path:?}: {error}")))?;

        entries.push(ParsedEntry {
            path,
            node,
            content_authority,
            source,
        });
    }

    resolve_hardlinks(&mut entries)?;
    metrics.archive_passes = metrics
        .archive_passes
        .checked_add(1)
        .ok_or_else(|| data_tar_error("DEB data archive pass count overflow"))?;
    metrics.archive_entries_traversed = metrics
        .archive_entries_traversed
        .checked_add(entries_seen)
        .ok_or_else(|| data_tar_error("DEB data archive entry count overflow"))?;
    Ok(entries)
}

/// dpkg normalizes leading slash/current-directory components onto its
/// extraction root. That root is container metadata, not one package-owned
/// deployable path in Conary's selected-root payload model.
fn is_archive_root_path(path: &str) -> bool {
    let mut components = Path::new(path).components().peekable();
    components.peek().is_some()
        && components.all(|component| matches!(component, Component::RootDir | Component::CurDir))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HardlinkVisit {
    Unvisited,
    Visiting,
    Resolved { anchor_index: usize },
}

/// Resolve the tar hardlink graph after every header has been parsed.
///
/// A hardlink header may precede its target and may point through another
/// hardlink. Resolution is therefore a graph operation, not an archive-order
/// lookup. Every chain must terminate at one regular payload node.
fn resolve_hardlinks(entries: &mut [ParsedEntry]) -> Result<()> {
    let path_indices = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| (entry.path.clone(), index))
        .collect::<BTreeMap<_, _>>();
    let mut visits = vec![HardlinkVisit::Unvisited; entries.len()];
    let mut stack = Vec::new();
    let mut resolved = Vec::new();

    for index in 0..entries.len() {
        if !matches!(entries[index].node.kind, PayloadNodeKind::Hardlink { .. }) {
            continue;
        }
        let anchor_index =
            resolve_hardlink_anchor(index, entries, &path_indices, &mut visits, &mut stack)?;
        resolved.push((index, anchor_index));
    }

    for &(link_index, anchor_index) in &resolved {
        let anchor_path = entries[anchor_index].path.clone();
        let identity = format!("path:{anchor_path}");
        let PayloadNodeKind::Regular { hardlink_identity } = &mut entries[anchor_index].node.kind
        else {
            unreachable!("hardlink resolver returns only regular anchors");
        };
        *hardlink_identity = Some(identity);
        let mut node = entries[anchor_index].node.clone();
        node.kind = PayloadNodeKind::Hardlink {
            target: anchor_path.clone(),
            identity: format!("path:{anchor_path}"),
        };
        entries[link_index].node = node;
    }

    for entry in entries {
        entry
            .node
            .validate()
            .map_err(|error| data_tar_error(format!("invalid node {:?}: {error}", entry.path)))?;
        entry
            .node
            .validate_content(entry.content_authority.as_ref())
            .map_err(|error| {
                data_tar_error(format!("invalid content {:?}: {error}", entry.path))
            })?;
    }
    Ok(())
}

fn resolve_hardlink_anchor(
    index: usize,
    entries: &[ParsedEntry],
    path_indices: &BTreeMap<String, usize>,
    visits: &mut [HardlinkVisit],
    stack: &mut Vec<usize>,
) -> Result<usize> {
    match visits[index] {
        HardlinkVisit::Resolved { anchor_index } => return Ok(anchor_index),
        HardlinkVisit::Visiting => {
            let cycle_start = stack
                .iter()
                .position(|candidate| *candidate == index)
                .unwrap_or(0);
            let mut cycle = stack[cycle_start..]
                .iter()
                .map(|candidate| format!("{:?}", entries[*candidate].path))
                .collect::<Vec<_>>();
            cycle.push(format!("{:?}", entries[index].path));
            return Err(data_tar_error(format!(
                "hardlink cycle detected: {}",
                cycle.join(" -> ")
            )));
        }
        HardlinkVisit::Unvisited => {}
    }

    let PayloadNodeKind::Hardlink { target, .. } = &entries[index].node.kind else {
        return Ok(index);
    };
    visits[index] = HardlinkVisit::Visiting;
    stack.push(index);

    let target_index = path_indices.get(target).copied().ok_or_else(|| {
        data_tar_error(format!(
            "hardlink {:?} targets missing payload path {target:?}",
            entries[index].path
        ))
    })?;
    let anchor_index = match &entries[target_index].node.kind {
        PayloadNodeKind::Regular { .. } => target_index,
        PayloadNodeKind::Hardlink { .. } => {
            resolve_hardlink_anchor(target_index, entries, path_indices, visits, stack)?
        }
        kind => {
            return Err(data_tar_error(format!(
                "hardlink {:?} resolves to non-regular {} payload {:?}",
                entries[index].path,
                payload_kind_name(kind),
                entries[target_index].path
            )));
        }
    };

    let popped = stack.pop();
    debug_assert_eq!(popped, Some(index));
    visits[index] = HardlinkVisit::Resolved { anchor_index };
    Ok(anchor_index)
}

fn payload_kind_name(kind: &PayloadNodeKind) -> &'static str {
    match kind {
        PayloadNodeKind::Regular { .. } => "regular",
        PayloadNodeKind::Directory => "directory",
        PayloadNodeKind::Symlink { .. } => "symlink",
        PayloadNodeKind::Hardlink { .. } => "hardlink",
        PayloadNodeKind::BlockDevice { .. } => "block-device",
        PayloadNodeKind::CharacterDevice { .. } => "character-device",
        PayloadNodeKind::Fifo => "FIFO",
        PayloadNodeKind::Socket => "socket",
    }
}

fn decode_header(block: &[u8; TAR_BLOCK_SIZE]) -> Result<Header> {
    validate_checksum(block)?;
    let format = if block[TAR_MAGIC] == GNU_MAGIC[..] {
        TarFormat::Gnu
    } else if block[TAR_MAGIC] == USTAR_MAGIC[..] {
        TarFormat::Ustar
    } else {
        TarFormat::Old
    };
    let entry_type = match block[TAR_TYPEFLAG] {
        TYPE_REGULAR_OLD => TYPE_REGULAR,
        entry_type => entry_type,
    };
    reject_unsupported_type(entry_type)?;

    let mut path = nul_terminated(&block[TAR_NAME]).to_vec();
    if format == TarFormat::Ustar && block[TAR_PREFIX.start] != 0 {
        let mut prefixed = nul_terminated(&block[TAR_PREFIX]).to_vec();
        prefixed.push(b'/');
        prefixed.extend_from_slice(&path);
        path = prefixed;
    }
    if path.is_empty() {
        return Err(data_tar_error("tar header has an empty path"));
    }

    let mode = parse_unsigned(&block[TAR_MODE], u64::from(u32::MAX), "mode")? as u32;
    let uid = parse_unsigned(&block[TAR_UID], i64::MAX as u64, "uid")?;
    let gid = parse_unsigned(&block[TAR_GID], i64::MAX as u64, "gid")?;
    let size = parse_unsigned(&block[TAR_SIZE], u64::MAX, "size")?;
    let mtime = parse_signed(&block[TAR_MTIME], i64::MIN, i64::MAX, "mtime")? as i64;
    let (device_major, device_minor) =
        if matches!(entry_type, TYPE_BLOCK_DEVICE | TYPE_CHARACTER_DEVICE) {
            (
                Some(parse_unsigned(
                    &block[TAR_DEVICE_MAJOR],
                    i64::MAX as u64,
                    "device major",
                )?),
                Some(parse_unsigned(
                    &block[TAR_DEVICE_MINOR],
                    i64::MAX as u64,
                    "device minor",
                )?),
            )
        } else {
            (None, None)
        };

    Ok(Header {
        path,
        link_name: nul_terminated(&block[TAR_LINK_NAME]).to_vec(),
        entry_type,
        mode,
        uid,
        gid,
        size,
        mtime,
        device_major,
        device_minor,
    })
}

fn reject_unsupported_type(entry_type: u8) -> Result<()> {
    match entry_type {
        TYPE_REGULAR
        | TYPE_HARDLINK
        | TYPE_SYMLINK
        | TYPE_CHARACTER_DEVICE
        | TYPE_BLOCK_DEVICE
        | TYPE_DIRECTORY
        | TYPE_FIFO
        | TYPE_GNU_LONG_LINK
        | TYPE_GNU_LONG_NAME => Ok(()),
        b'g' | b'x' => Err(data_tar_error(format!(
            "unsupported PAX tar header type {}",
            display_typeflag(entry_type)
        ))),
        b'D' | b'M' | b'S' | b'V' => Err(data_tar_error(format!(
            "unsupported GNU tar header type {}",
            display_typeflag(entry_type)
        ))),
        b'A' | b'X' => Err(data_tar_error(format!(
            "unsupported Solaris tar header type {}",
            display_typeflag(entry_type)
        ))),
        unsupported => Err(data_tar_error(format!(
            "unknown tar header type {}",
            display_typeflag(unsupported)
        ))),
    }
}

fn validate_checksum(block: &[u8; TAR_BLOCK_SIZE]) -> Result<()> {
    let declared = parse_ascii_octal(&block[TAR_CHECKSUM], "checksum")?;
    let actual = block
        .iter()
        .enumerate()
        .map(|(index, byte)| {
            if TAR_CHECKSUM.contains(&index) {
                u64::from(b' ')
            } else {
                u64::from(*byte)
            }
        })
        .sum::<u64>();
    if declared != u128::from(actual) {
        return Err(data_tar_error(format!(
            "invalid tar header checksum: expected {declared}, computed {actual}"
        )));
    }
    Ok(())
}

fn parse_unsigned(bytes: &[u8], max: u64, field: &str) -> Result<u64> {
    let value = parse_tar_number(bytes, field)?;
    if value < 0 || value > i128::from(max) {
        return Err(data_tar_error(format!(
            "tar {field} field is outside supported range 0..={max}"
        )));
    }
    Ok(value as u64)
}

fn parse_signed(bytes: &[u8], min: i64, max: i64, field: &str) -> Result<i128> {
    let value = parse_tar_number(bytes, field)?;
    if value < i128::from(min) || value > i128::from(max) {
        return Err(data_tar_error(format!(
            "tar {field} field is outside supported range {min}..={max}"
        )));
    }
    Ok(value)
}

fn parse_tar_number(bytes: &[u8], field: &str) -> Result<i128> {
    match bytes.first().copied() {
        Some(0x80) => {
            let mut value = 0i128;
            for (index, byte) in bytes.iter().copied().enumerate() {
                let byte = if index == 0 { 0 } else { byte };
                value = value
                    .checked_shl(8)
                    .and_then(|shifted| shifted.checked_add(i128::from(byte)))
                    .ok_or_else(|| data_tar_error(format!("tar {field} field overflows i128")))?;
            }
            Ok(value)
        }
        Some(0xff) => {
            let mut value = -1i128;
            for byte in bytes {
                value = value
                    .checked_shl(8)
                    .map(|shifted| shifted | i128::from(*byte))
                    .ok_or_else(|| data_tar_error(format!("tar {field} field overflows i128")))?;
            }
            Ok(value)
        }
        Some(_) => i128::try_from(parse_ascii_octal(bytes, field)?)
            .map_err(|_| data_tar_error(format!("tar {field} field overflows i128"))),
        None => Err(data_tar_error(format!("tar {field} field is empty"))),
    }
}

fn parse_ascii_octal(bytes: &[u8], field: &str) -> Result<u128> {
    let mut index = 0usize;
    while index < bytes.len() && bytes[index] == b' ' {
        index += 1;
    }
    if index == bytes.len() {
        return Err(data_tar_error(format!("tar {field} field is blank")));
    }

    let mut value = 0u128;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        let digit = bytes[index];
        if digit > b'7' {
            return Err(data_tar_error(format!(
                "tar {field} field contains a non-octal digit"
            )));
        }
        value = value
            .checked_mul(8)
            .and_then(|current| current.checked_add(u128::from(digit - b'0')))
            .ok_or_else(|| data_tar_error(format!("tar {field} field overflows u128")))?;
        index += 1;
    }
    if bytes[index..].iter().any(|byte| !matches!(*byte, 0 | b' ')) {
        return Err(data_tar_error(format!(
            "tar {field} field has invalid trailing bytes"
        )));
    }
    Ok(value)
}

fn read_header_block(reader: &mut dyn Read) -> Result<Option<[u8; TAR_BLOCK_SIZE]>> {
    let mut block = [0u8; TAR_BLOCK_SIZE];
    let mut offset = 0usize;
    while offset < block.len() {
        let read = reader
            .read(&mut block[offset..])
            .map_err(|error| data_tar_error(format!("failed to read tar header: {error}")))?;
        if read == 0 {
            if offset == 0 {
                return Ok(None);
            }
            return Err(data_tar_error("partially read tar header"));
        }
        offset += read;
    }
    Ok(Some(block))
}

fn read_long_value(
    reader: &mut dyn Read,
    size: u64,
    entry_type: u8,
    bounds: &ArchiveDecodeBounds,
    metadata_bytes: &mut u64,
) -> Result<Vec<u8>> {
    let label = if entry_type == TYPE_GNU_LONG_NAME {
        "GNU long-name payload"
    } else {
        "GNU long-link payload"
    };
    *metadata_bytes =
        bounds.add_metadata_bytes("DEB data.tar path metadata", *metadata_bytes, size)?;
    let mut value = vec![0u8; size as usize];
    read_exact(reader, &mut value, label)?;
    discard_padding(reader, size, label)?;
    value.truncate(
        value
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(value.len()),
    );
    Ok(value)
}

fn read_regular_content(
    reader: &mut dyn Read,
    path: &str,
    size: u64,
    spool: Option<&PayloadSpool>,
    index: usize,
) -> Result<(PayloadContentAuthority, Option<ReopenablePayload>)> {
    let mut output = spool
        .map(|spool| {
            let output_path = spool.indexed_path(index);
            File::create(&output_path).map(|file| (file, output_path))
        })
        .transpose()?;
    let mut hasher = Hasher::new(HashAlgorithm::Sha256);
    let mut remaining = size;
    let mut buffer = [0u8; PAYLOAD_IO_BUFFER_SIZE];
    while remaining > 0 {
        let chunk_size = usize::try_from(remaining.min(buffer.len() as u64))
            .expect("bounded regular payload chunk size");
        read_exact(reader, &mut buffer[..chunk_size], "regular payload")?;
        hasher.update(&buffer[..chunk_size]);
        if let Some((file, _)) = &mut output {
            file.write_all(&buffer[..chunk_size])?;
        }
        remaining -= chunk_size as u64;
    }
    discard_padding(reader, size, &format!("regular payload {path:?}"))?;
    if let Some((file, _)) = &mut output {
        file.sync_all()?;
    }
    let source = output.map(|(_, path)| {
        spool
            .expect("output exists only when a spool was supplied")
            .source(path)
    });

    Ok((
        PayloadContentAuthority {
            sha256: hasher.finalize().value,
            size,
        },
        source,
    ))
}

fn discard_padding(reader: &mut dyn Read, size: u64, description: &str) -> Result<()> {
    let padding = (TAR_BLOCK_SIZE as u64 - size % TAR_BLOCK_SIZE as u64) % TAR_BLOCK_SIZE as u64;
    let mut bytes = [0u8; TAR_BLOCK_SIZE];
    read_exact(reader, &mut bytes[..padding as usize], "tar entry padding")?;
    if bytes[..padding as usize].iter().any(|byte| *byte != 0) {
        return Err(data_tar_error(format!(
            "{description} declares {size} bytes but carries nonzero bytes in its padding"
        )));
    }
    Ok(())
}

fn read_exact(reader: &mut dyn Read, bytes: &mut [u8], description: &str) -> Result<()> {
    reader
        .read_exact(bytes)
        .map_err(|error| data_tar_error(format!("failed to read {description}: {error}")))
}

fn require_zero_size(path: &str, size: u64, kind: &str) -> Result<()> {
    if size != 0 {
        return Err(data_tar_error(format!(
            "{kind} payload {path:?} declares unexpected {size} byte content"
        )));
    }
    Ok(())
}

fn required_link_target(bytes: &[u8], path: &str, kind: &str) -> Result<String> {
    if bytes.is_empty() {
        return Err(data_tar_error(format!(
            "{kind} payload {path:?} has no link target"
        )));
    }
    utf8_field(bytes, "link target").map(str::to_string)
}

fn utf8_field<'a>(bytes: &'a [u8], field: &str) -> Result<&'a str> {
    std::str::from_utf8(bytes)
        .map_err(|_| data_tar_error(format!("tar {field} is not valid UTF-8")))
}

fn nul_terminated(bytes: &[u8]) -> &[u8] {
    &bytes[..bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len())]
}

fn mode_type(kind: &PayloadNodeKind) -> u32 {
    match kind {
        PayloadNodeKind::Regular { .. } | PayloadNodeKind::Hardlink { .. } => libc::S_IFREG,
        PayloadNodeKind::Directory => libc::S_IFDIR,
        PayloadNodeKind::Symlink { .. } => libc::S_IFLNK,
        PayloadNodeKind::BlockDevice { .. } => libc::S_IFBLK,
        PayloadNodeKind::CharacterDevice { .. } => libc::S_IFCHR,
        PayloadNodeKind::Fifo => libc::S_IFIFO,
        PayloadNodeKind::Socket => libc::S_IFSOCK,
    }
}

fn display_typeflag(entry_type: u8) -> String {
    if entry_type.is_ascii_graphic() {
        format!("{entry_type:#04x} ('{}')", char::from(entry_type))
    } else {
        format!("{entry_type:#04x}")
    }
}

fn data_tar_error(message: impl Into<String>) -> Error {
    Error::ParseError(format!("Failed to parse DEB data.tar: {}", message.into()))
}

#[cfg(test)]
#[path = "payload/tests.rs"]
mod tests;
