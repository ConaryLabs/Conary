// conary-core/src/packages/arch/payload/mod.rs

//! Exact ALPM tar payload parsing.

mod sparse;
mod xattrs;

use crate::ccs::ArchiveDecodeBounds;
use crate::error::{Error, Result};
use crate::hash::{HashAlgorithm, Hasher};
use crate::packages::archive_utils::normalize_path;
use crate::packages::payload::{
    PAYLOAD_IO_BUFFER_SIZE, PackagePayloadFile, PayloadSpool, ReopenablePayload,
};
use crate::payload::{
    PayloadContentAuthority, PayloadIdentity, PayloadNode, PayloadNodeKind, PayloadTimestamp,
};
use sparse::{GnuSparsePaxRecords, GnuSparseV1};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Write};
use tar::{Entry, EntryType};
use xattrs::{PaxXattrFamily, PaxXattrRecords, decode_libarchive_base64};

pub(crate) struct ArchivePayloadEntry {
    path: String,
    node: PayloadNode,
    content_authority: Option<PayloadContentAuthority>,
    source: Option<ReopenablePayload>,
    hardlink_authority: Option<PayloadContentAuthority>,
    _hardlink_source: Option<ReopenablePayload>,
}

impl ArchivePayloadEntry {
    pub(super) fn path(&self) -> &str {
        &self.path
    }

    pub(super) fn regular_content_size(&self) -> Option<u64> {
        self.content_authority
            .as_ref()
            .map(|authority| authority.size)
    }

    pub(super) fn read_regular_content_bounded(&self, limit: u64) -> Result<Option<Vec<u8>>> {
        if !self.node.kind.is_regular() {
            return Ok(None);
        }
        let authority = self
            .content_authority
            .as_ref()
            .expect("regular parsed entry has content authority");
        if authority.size > limit {
            return Err(Error::ParseError(format!(
                "ALPM control payload {} is {} bytes; maximum is {limit}",
                self.path, authority.size
            )));
        }
        let mut content = Vec::with_capacity(usize::try_from(authority.size).map_err(|_| {
            Error::ParseError(format!(
                "ALPM control payload {} cannot be represented in memory",
                self.path
            ))
        })?);
        self.source
            .as_ref()
            .expect("regular parsed entry has content source")
            .open()?
            .read_to_end(&mut content)?;
        Ok(Some(content))
    }
}

pub(crate) fn declared_spool_bytes<R: Read>(
    entry: &mut Entry<'_, R>,
    bounds: &ArchiveDecodeBounds,
) -> Result<u64> {
    let raw_path = exact_utf8(entry.path_bytes().as_ref(), "payload path", "archive entry")?;
    let pax = read_pax(entry, &raw_path, bounds)?;
    Ok(match entry.header().entry_type() {
        EntryType::Regular | EntryType::Continuous | EntryType::GNUSparse => pax
            .gnu_sparse
            .as_ref()
            .map_or_else(|| entry.size(), |sparse| sparse.logical_size),
        EntryType::Link if entry.size() != 0 => entry.size(),
        _ => 0,
    })
}

pub(crate) fn parse_entry<R: Read>(
    entry: &mut Entry<'_, R>,
    spool: &PayloadSpool,
    index: usize,
    bounds: &ArchiveDecodeBounds,
) -> Result<ArchivePayloadEntry> {
    let path_bytes = entry.path_bytes().into_owned();
    let raw_path = std::str::from_utf8(&path_bytes)
        .map_err(|error| Error::ParseError(format!("ALPM payload path is not UTF-8: {error}")))?;
    let mut path = normalize_path(raw_path)
        .map_err(|error| Error::ParseError(format!("invalid ALPM payload path: {error}")))?;
    let header = entry.header();
    let entry_type = header.entry_type();
    let permissions = header
        .mode()
        .map_err(|error| Error::ParseError(format!("invalid mode for {path}: {error}")))?
        & 0o7777;
    let uid = header
        .uid()
        .map_err(|error| Error::ParseError(format!("invalid uid for {path}: {error}")))?;
    let gid = header
        .gid()
        .map_err(|error| Error::ParseError(format!("invalid gid for {path}: {error}")))?;
    let header_mtime = header
        .mtime()
        .map_err(|error| Error::ParseError(format!("invalid mtime for {path}: {error}")))?;
    let header_device = if matches!(entry_type, EntryType::Block | EntryType::Char) {
        (
            header
                .device_major()
                .map_err(|error| {
                    Error::ParseError(format!("invalid device major for {path}: {error}"))
                })?
                .map(u64::from),
            header
                .device_minor()
                .map_err(|error| {
                    Error::ParseError(format!("invalid device minor for {path}: {error}"))
                })?
                .map(u64::from),
        )
    } else {
        (None, None)
    };
    let link_target = entry
        .link_name_bytes()
        .map(|target| exact_utf8(target.as_ref(), "link target", &path))
        .transpose()?;
    let pax = read_pax(entry, raw_path, bounds)?;
    let source_path = pax
        .gnu_sparse
        .as_ref()
        .map_or(raw_path, |sparse| sparse.name.as_str());
    path = normalize_path(source_path)
        .map_err(|error| Error::ParseError(format!("invalid ALPM payload path: {error}")))?;
    let mtime = pax.mtime.unwrap_or(PayloadTimestamp {
        seconds: i64::try_from(header_mtime).map_err(|_| {
            Error::ParseError(format!("mtime for {path} exceeds signed timestamp range"))
        })?,
        nanoseconds: 0,
    });
    let device = (
        pax.device_major.or(header_device.0),
        pax.device_minor.or(header_device.1),
    );
    let mut content_authority = None;
    let mut source = None;
    let mut hardlink_authority = None;
    let mut hardlink_source = None;
    let kind = match entry_type {
        EntryType::Regular | EntryType::Continuous | EntryType::GNUSparse => {
            let (authority, payload_source) =
                spool_content(entry, &path, spool, index, pax.gnu_sparse.as_ref(), bounds)?;
            content_authority = Some(authority);
            source = Some(payload_source);
            PayloadNodeKind::Regular {
                hardlink_identity: None,
            }
        }
        EntryType::Directory => {
            require_empty_entry(entry, &path)?;
            PayloadNodeKind::Directory
        }
        EntryType::Symlink => {
            require_empty_entry(entry, &path)?;
            let target = link_target
                .ok_or_else(|| Error::ParseError(format!("ALPM symlink {path} has no target")))?;
            PayloadNodeKind::Symlink { target }
        }
        EntryType::Link => {
            let target = link_target
                .ok_or_else(|| Error::ParseError(format!("ALPM hardlink {path} has no target")))?;
            let target = normalize_path(&target).map_err(|error| {
                Error::ParseError(format!("invalid ALPM hardlink target for {path}: {error}"))
            })?;
            if entry.size() != 0 {
                let (authority, payload_source) =
                    spool_content(entry, &path, spool, index, None, bounds)?;
                hardlink_authority = Some(authority);
                hardlink_source = Some(payload_source);
            }
            PayloadNodeKind::Hardlink {
                identity: format!("path:{target}"),
                target,
            }
        }
        EntryType::Block => {
            require_empty_entry(entry, &path)?;
            let (Some(major), Some(minor)) = device else {
                return Err(Error::ParseError(format!(
                    "ALPM block device {path} has no complete device identity"
                )));
            };
            PayloadNodeKind::BlockDevice { major, minor }
        }
        EntryType::Char => {
            require_empty_entry(entry, &path)?;
            let (Some(major), Some(minor)) = device else {
                return Err(Error::ParseError(format!(
                    "ALPM character device {path} has no complete device identity"
                )));
            };
            PayloadNodeKind::CharacterDevice { major, minor }
        }
        EntryType::Fifo => {
            require_empty_entry(entry, &path)?;
            PayloadNodeKind::Fifo
        }
        other if other.as_byte() == b's' => {
            require_empty_entry(entry, &path)?;
            PayloadNodeKind::Socket
        }
        other => {
            return Err(Error::ParseError(format!(
                "unsupported ALPM tar entry type {:?} for {path}",
                other.as_byte()
            )));
        }
    };
    let mode = mode_type(&kind) | permissions;
    let node = PayloadNode {
        kind,
        mode,
        user: PayloadIdentity::Numeric { id: uid },
        group: PayloadIdentity::Numeric { id: gid },
        mtime,
        xattrs: pax.xattrs,
    };
    node.validate()
        .map_err(|error| Error::ParseError(format!("invalid ALPM node {path}: {error}")))?;
    Ok(ArchivePayloadEntry {
        path,
        node,
        content_authority,
        source,
        hardlink_authority,
        _hardlink_source: hardlink_source,
    })
}

pub(crate) fn resolve_hardlinks(
    mut entries: Vec<ArchivePayloadEntry>,
) -> Result<Vec<PackagePayloadFile>> {
    let mut paths = HashMap::new();
    for (index, entry) in entries.iter().enumerate() {
        if paths.insert(entry.path.clone(), index).is_some() {
            return Err(Error::ParseError(format!(
                "duplicate ALPM payload path {}",
                entry.path
            )));
        }
    }

    for index in 0..entries.len() {
        if !matches!(entries[index].node.kind, PayloadNodeKind::Hardlink { .. }) {
            continue;
        }
        let anchor = hardlink_anchor(index, &entries, &paths)?;
        let anchor_path = entries[anchor].path.clone();
        let identity = format!("path:{anchor_path}");
        if entries[index].hardlink_authority.is_some()
            && entries[index].hardlink_authority != entries[anchor].content_authority
        {
            return Err(Error::ParseError(format!(
                "ALPM hardlink {} carries bytes different from anchor {anchor_path}",
                entries[index].path
            )));
        }
        match &mut entries[anchor].node.kind {
            PayloadNodeKind::Regular { hardlink_identity } => {
                if let Some(existing) = hardlink_identity
                    && existing != &identity
                {
                    return Err(Error::ParseError(format!(
                        "ALPM hardlink anchor {anchor_path} has conflicting identities"
                    )));
                }
                *hardlink_identity = Some(identity.clone());
            }
            _ => unreachable!("hardlink_anchor returns a regular entry"),
        }
        let mut node = entries[anchor].node.clone();
        node.kind = PayloadNodeKind::Hardlink {
            target: anchor_path,
            identity,
        };
        entries[index].node = node;
    }

    entries
        .into_iter()
        .map(|entry| {
            entry
                .node
                .validate_content(entry.content_authority.as_ref())
                .map_err(|error| {
                    Error::ParseError(format!(
                        "invalid ALPM content authority for {}: {error}",
                        entry.path
                    ))
                })?;
            PackagePayloadFile::new(
                entry.path,
                entry.node,
                entry.content_authority,
                entry.source,
            )
        })
        .collect()
}

fn hardlink_anchor(
    start: usize,
    entries: &[ArchivePayloadEntry],
    paths: &HashMap<String, usize>,
) -> Result<usize> {
    let mut current = start;
    let mut seen = HashSet::new();
    loop {
        if !seen.insert(current) {
            return Err(Error::ParseError(format!(
                "ALPM hardlink cycle starts at {}",
                entries[start].path
            )));
        }
        match &entries[current].node.kind {
            PayloadNodeKind::Regular { .. } => return Ok(current),
            PayloadNodeKind::Hardlink { target, .. } => {
                current = *paths.get(target).ok_or_else(|| {
                    Error::ParseError(format!(
                        "ALPM hardlink {} targets missing payload node {target}",
                        entries[current].path
                    ))
                })?;
            }
            _ => {
                return Err(Error::ParseError(format!(
                    "ALPM hardlink {} targets non-regular node {}",
                    entries[start].path, entries[current].path
                )));
            }
        }
    }
}

#[derive(Default)]
struct PaxAuthority {
    mtime: Option<PayloadTimestamp>,
    device_major: Option<u64>,
    device_minor: Option<u64>,
    xattrs: BTreeMap<String, Vec<u8>>,
    gnu_sparse: Option<GnuSparseV1>,
}

fn read_pax<R: Read>(
    entry: &mut Entry<'_, R>,
    path: &str,
    bounds: &ArchiveDecodeBounds,
) -> Result<PaxAuthority> {
    let mut authority = PaxAuthority::default();
    let Some(extensions) = entry
        .pax_extensions()
        .map_err(|error| Error::ParseError(format!("invalid PAX data for {path}: {error}")))?
    else {
        return Ok(authority);
    };
    let mut xattr_records = PaxXattrRecords::default();
    let mut sparse_records = GnuSparsePaxRecords::default();
    for extension in extensions {
        let extension = extension.map_err(|error| {
            Error::ParseError(format!("invalid PAX record for {path}: {error}"))
        })?;
        let key = extension
            .key()
            .map_err(|error| Error::ParseError(format!("non-UTF-8 PAX key for {path}: {error}")))?;
        let value = extension.value_bytes();
        if sparse_records.record(key, value, path)? {
            continue;
        }
        match key {
            "mtime" => authority.mtime = Some(parse_pax_timestamp(value, path)?),
            "SCHILY.devmajor" => authority.device_major = Some(parse_decimal(value, key, path)?),
            "SCHILY.devminor" => authority.device_minor = Some(parse_decimal(value, key, path)?),
            "RHT.security.selinux" => {
                xattr_records.insert(
                    "security.selinux".to_string(),
                    PaxXattrFamily::RhtSelinux,
                    value.to_vec(),
                    path,
                )?;
            }
            _ if key.starts_with("SCHILY.xattr.") => {
                let name = key.strip_prefix("SCHILY.xattr.").expect("prefix checked");
                xattr_records.insert(
                    name.to_string(),
                    PaxXattrFamily::Schily,
                    value.to_vec(),
                    path,
                )?;
            }
            _ if key.starts_with("LIBARCHIVE.xattr.") => {
                let encoded_name = key
                    .strip_prefix("LIBARCHIVE.xattr.")
                    .expect("prefix checked");
                let name_bytes = percent_decode(encoded_name.as_bytes(), path)?;
                let name = String::from_utf8(name_bytes).map_err(|error| {
                    Error::ParseError(format!(
                        "LIBARCHIVE xattr name for {path} is not UTF-8: {error}"
                    ))
                })?;
                let decoded = decode_libarchive_base64(value, path)?;
                xattr_records.insert(name, PaxXattrFamily::Libarchive, decoded, path)?;
            }
            _ => {}
        }
    }
    authority.xattrs = xattr_records.project(path)?;
    authority.gnu_sparse = sparse_records.finish(path)?;
    if let Some(sparse) = &authority.gnu_sparse {
        bounds.admit_payload_object("ALPM GNU sparse logical payload", sparse.logical_size)?;
    }
    Ok(authority)
}

fn parse_pax_timestamp(value: &[u8], path: &str) -> Result<PayloadTimestamp> {
    let text = std::str::from_utf8(value)
        .map_err(|error| Error::ParseError(format!("invalid PAX mtime for {path}: {error}")))?;
    let negative = text.starts_with('-');
    let unsigned = text.strip_prefix('-').unwrap_or(text);
    let (whole, fractional) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    if whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fractional.bytes().all(|byte| byte.is_ascii_digit())
        || fractional.len() > 9
    {
        return Err(Error::ParseError(format!(
            "PAX mtime for {path} is outside nanosecond decimal grammar"
        )));
    }
    let whole = whole.parse::<u64>().map_err(|error| {
        Error::ParseError(format!("PAX mtime for {path} is out of range: {error}"))
    })?;
    let mut nanos = if fractional.is_empty() {
        0
    } else {
        fractional.parse::<u32>().map_err(|error| {
            Error::ParseError(format!("PAX mtime fraction for {path} is invalid: {error}"))
        })? * 10_u32.pow(u32::try_from(9 - fractional.len()).expect("fraction length bounded"))
    };
    let seconds = if negative {
        if nanos == 0 {
            -i64::try_from(whole).map_err(|_| {
                Error::ParseError(format!("PAX mtime for {path} exceeds signed range"))
            })?
        } else {
            nanos = 1_000_000_000 - nanos;
            -i64::try_from(whole).map_err(|_| {
                Error::ParseError(format!("PAX mtime for {path} exceeds signed range"))
            })? - 1
        }
    } else {
        i64::try_from(whole)
            .map_err(|_| Error::ParseError(format!("PAX mtime for {path} exceeds signed range")))?
    };
    Ok(PayloadTimestamp {
        seconds,
        nanoseconds: nanos,
    })
}

fn parse_decimal(value: &[u8], key: &str, path: &str) -> Result<u64> {
    let text = std::str::from_utf8(value)
        .map_err(|error| Error::ParseError(format!("invalid {key} value for {path}: {error}")))?;
    text.parse::<u64>()
        .map_err(|error| Error::ParseError(format!("invalid {key} value for {path}: {error}")))
}

fn percent_decode(value: &[u8], path: &str) -> Result<Vec<u8>> {
    let mut decoded = Vec::with_capacity(value.len());
    let mut index = 0;
    while index < value.len() {
        if value[index] != b'%' {
            decoded.push(value[index]);
            index += 1;
            continue;
        }
        let digits = value.get(index + 1..index + 3).ok_or_else(|| {
            Error::ParseError(format!("truncated LIBARCHIVE xattr name for {path}"))
        })?;
        let digits = std::str::from_utf8(digits).expect("hex digits are ASCII bytes");
        decoded.push(u8::from_str_radix(digits, 16).map_err(|error| {
            Error::ParseError(format!("invalid LIBARCHIVE xattr name for {path}: {error}"))
        })?);
        index += 3;
    }
    Ok(decoded)
}

fn spool_content<R: Read>(
    entry: &mut Entry<'_, R>,
    path: &str,
    spool: &PayloadSpool,
    index: usize,
    sparse: Option<&GnuSparseV1>,
    bounds: &ArchiveDecodeBounds,
) -> Result<(PayloadContentAuthority, ReopenablePayload)> {
    let size = sparse.map_or_else(|| entry.size(), |sparse| sparse.logical_size);
    crate::packages::payload::ensure_available_space(spool.root(), size)?;
    let output_path = spool.indexed_path(index);
    let mut output = File::create(&output_path)?;
    let sha256 = if let Some(sparse) = sparse {
        let encoded_size = entry.size();
        sparse::copy_payload(entry, &mut output, encoded_size, sparse, path, bounds)?
    } else {
        copy_declared_payload(entry, &mut output, size, path)?
    };
    output.sync_all()?;
    Ok((
        PayloadContentAuthority { sha256, size },
        spool.source(output_path),
    ))
}

fn copy_declared_payload(
    reader: &mut dyn Read,
    writer: &mut dyn Write,
    size: u64,
    path: &str,
) -> Result<String> {
    let mut hasher = Hasher::new(HashAlgorithm::Sha256);
    let mut copied = 0_u64;
    let mut buffer = [0_u8; PAYLOAD_IO_BUFFER_SIZE];
    let mut limited = reader.take(size.saturating_add(1));
    loop {
        let read = limited.read(&mut buffer).map_err(|error| {
            Error::ParseError(format!("read ALPM payload node {path}: {error}"))
        })?;
        if read == 0 {
            break;
        }
        copied = copied.checked_add(read as u64).ok_or_else(|| {
            Error::ParseError(format!("ALPM payload node {path} size overflows u64"))
        })?;
        if copied > size {
            return Err(Error::ParseError(format!(
                "ALPM payload node {path} declares {size} bytes but yields at least {copied}"
            )));
        }
        hasher.update(&buffer[..read]);
        writer.write_all(&buffer[..read])?;
    }
    if copied != size {
        return Err(Error::ParseError(format!(
            "ALPM payload node {path} declares {size} bytes but yields {}",
            copied
        )));
    }
    Ok(hasher.finalize().value)
}

fn require_empty_entry<R: Read>(entry: &Entry<'_, R>, path: &str) -> Result<()> {
    if entry.size() != 0 {
        return Err(Error::ParseError(format!(
            "non-regular ALPM payload node {path} carries {} bytes",
            entry.size()
        )));
    }
    Ok(())
}

fn exact_utf8(value: &[u8], field: &str, path: &str) -> Result<String> {
    std::str::from_utf8(value)
        .map(str::to_string)
        .map_err(|error| {
            Error::ParseError(format!("ALPM {field} for {path} is not UTF-8: {error}"))
        })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packages::{ExtractedFile, PackagePayload};
    use std::io::Cursor;
    use tar::{Archive, Builder, Header};

    fn header(
        path: &str,
        entry_type: EntryType,
        mode: u32,
        uid: u64,
        gid: u64,
        mtime: u64,
        size: u64,
    ) -> Header {
        let mut header = Header::new_gnu();
        header.set_path(path).unwrap();
        header.set_entry_type(entry_type);
        header.set_mode(mode);
        header.set_uid(uid);
        header.set_gid(gid);
        header.set_mtime(mtime);
        header.set_size(size);
        header
    }

    fn append(builder: &mut Builder<Vec<u8>>, mut header: Header, content: &[u8]) {
        header.set_cksum();
        builder.append(&header, content).unwrap();
    }

    fn pax_sparse_v1_archive(
        logical_path: &'static str,
        logical_size: &'static [u8],
        map: &[u8],
        extent_bytes: &[u8],
    ) -> Vec<u8> {
        let mut builder = Builder::new(Vec::new());
        builder
            .append_pax_extensions([
                ("GNU.sparse.major", b"1".as_slice()),
                ("GNU.sparse.minor", b"0".as_slice()),
                ("GNU.sparse.name", logical_path.as_bytes()),
                ("GNU.sparse.realsize", logical_size),
            ])
            .unwrap();
        let mut encoded = map.to_vec();
        encoded.resize(encoded.len().div_ceil(512) * 512, 0);
        encoded.extend_from_slice(extent_bytes);
        append(
            &mut builder,
            header(
                "GNUSparseFile.123/sparse.bin",
                EntryType::Regular,
                0o640,
                12,
                34,
                56,
                encoded.len() as u64,
            ),
            &encoded,
        );
        builder.into_inner().unwrap()
    }

    fn parse_archive(bytes: Vec<u8>) -> Result<Vec<ExtractedFile>> {
        let mut archive = Archive::new(Cursor::new(bytes));
        let mut parsed = Vec::new();
        let spool = PayloadSpool::new(0)?;
        let bounds = crate::ccs::CCS_BUDGET.archive_decode_bounds()?;
        for (index, entry) in archive
            .entries()
            .map_err(|error| Error::ParseError(error.to_string()))?
            .enumerate()
        {
            let mut entry = entry.map_err(|error| Error::ParseError(error.to_string()))?;
            parsed.push(parse_entry(&mut entry, &spool, index, &bounds)?);
        }
        PackagePayload::new(resolve_hardlinks(parsed)?).to_extracted_in_memory()
    }

    #[test]
    fn declared_payload_copy_rejects_shorter_and_longer_bodies() {
        let mut output = Vec::new();
        let error =
            copy_declared_payload(&mut Cursor::new(b"ab"), &mut output, 3, "/short").unwrap_err();
        assert!(matches!(&error, Error::ParseError(_)));
        assert!(error.to_string().contains("declares 3 bytes but yields 2"));

        output.clear();
        let error =
            copy_declared_payload(&mut Cursor::new(b"abcd"), &mut output, 3, "/long").unwrap_err();
        assert!(matches!(&error, Error::ParseError(_)));
        assert!(
            error
                .to_string()
                .contains("declares 3 bytes but yields at least 4")
        );
    }

    #[test]
    fn parses_gnu_pax_sparse_v1_as_the_logical_path_and_content() {
        let bytes = pax_sparse_v1_archive(
            "usr/share/example/sparse.bin",
            b"16",
            b"3\n0\n3\n13\n3\n16\n0\n",
            b"abcxyz",
        );
        let bounds = crate::ccs::CCS_BUDGET.archive_decode_bounds().unwrap();
        let mut archive = Archive::new(Cursor::new(bytes.clone()));
        let mut entry = archive.entries().unwrap().next().unwrap().unwrap();
        assert_eq!(declared_spool_bytes(&mut entry, &bounds).unwrap(), 16);

        let files = parse_archive(bytes).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "/usr/share/example/sparse.bin");
        assert_eq!(files[0].content_authority.as_ref().unwrap().size, 16);
        assert_eq!(files[0].content, b"abc\0\0\0\0\0\0\0\0\0\0xyz");
    }

    #[test]
    fn rejects_overlapping_gnu_pax_sparse_v1_extents() {
        let bytes = pax_sparse_v1_archive(
            "usr/share/example/sparse.bin",
            b"5",
            b"2\n0\n4\n3\n2\n",
            b"abcdef",
        );
        let error = parse_archive(bytes).expect_err("overlapping sparse extents must fail closed");
        assert!(
            error.to_string().contains("overlapping, out of order"),
            "{error}"
        );
    }

    #[test]
    fn rejects_gnu_pax_sparse_v1_extent_count_over_the_structural_budget() {
        let bytes = pax_sparse_v1_archive(
            "usr/share/example/sparse.bin",
            b"16",
            b"3\n0\n3\n13\n3\n16\n0\n",
            b"abcxyz",
        );
        let mut bounds = crate::ccs::CCS_BUDGET.archive_decode_bounds().unwrap();
        bounds.max_payload_references = 2;
        let mut archive = Archive::new(Cursor::new(bytes));
        let mut entry = archive.entries().unwrap().next().unwrap().unwrap();
        let error = match parse_entry(&mut entry, &PayloadSpool::new(0).unwrap(), 0, &bounds) {
            Ok(_) => panic!("sparse extent count over the structural budget must fail closed"),
            Err(error) => error,
        };
        let Error::Budget(error) = error else {
            panic!("expected typed payload-reference budget refusal");
        };
        assert_eq!(
            error.dimension,
            crate::ccs::BudgetDimension::PayloadReferenceCount
        );
        assert_eq!(error.observed, 3);
        assert_eq!(error.limit, 2);
    }

    #[test]
    fn parses_exact_posix_nodes_and_pax_authority() {
        let mut builder = Builder::new(Vec::new());
        builder
            .append_pax_extensions([
                ("mtime", b"-1.25".as_slice()),
                ("SCHILY.xattr.user.conary", b"exact".as_slice()),
                // SCHILY and LIBARCHIVE families for the same xattr with
                // byte-identical values (4 bytes: [1, 2, 3, 4]).
                // LIBARCHIVE uses unpadded base64 per the writer grammar.
                (
                    "SCHILY.xattr.security.capability",
                    b"\x01\x02\x03\x04".as_slice(),
                ),
                (
                    "LIBARCHIVE.xattr.security%2Ecapability",
                    b"AQIDBA".as_slice(),
                ),
            ])
            .unwrap();
        append(
            &mut builder,
            header("usr/bin/tool", EntryType::Regular, 0o4750, 123, 456, 99, 5),
            b"hello",
        );

        let mut symlink = header(
            "usr/bin/tool-link",
            EntryType::Symlink,
            0o777,
            321,
            654,
            43,
            0,
        );
        symlink.set_link_name("tool").unwrap();
        append(&mut builder, symlink, &[]);

        let mut character = header("dev/example", EntryType::Char, 0o600, 0, 0, 44, 0);
        character.set_device_major(10).unwrap();
        character.set_device_minor(200).unwrap();
        append(&mut builder, character, &[]);

        append(
            &mut builder,
            header("run/example", EntryType::Fifo, 0o620, 20, 30, 45, 0),
            &[],
        );

        let files = parse_archive(builder.into_inner().unwrap()).unwrap();
        assert_eq!(files.len(), 4);

        let regular = &files[0];
        assert_eq!(regular.path, "/usr/bin/tool");
        assert_eq!(
            regular.node.kind,
            PayloadNodeKind::Regular {
                hardlink_identity: None
            }
        );
        assert_eq!(regular.node.mode, libc::S_IFREG | 0o4750);
        assert_eq!(regular.node.user, PayloadIdentity::Numeric { id: 123 });
        assert_eq!(regular.node.group, PayloadIdentity::Numeric { id: 456 });
        assert_eq!(
            regular.node.mtime,
            PayloadTimestamp {
                seconds: -2,
                nanoseconds: 750_000_000,
            }
        );
        assert_eq!(
            regular.node.xattrs.get("user.conary"),
            Some(&b"exact".to_vec())
        );
        assert_eq!(
            regular.node.xattrs.get("security.capability"),
            Some(&vec![1, 2, 3, 4])
        );
        assert_eq!(regular.content, b"hello");
        assert_eq!(
            regular.content_authority,
            Some(PayloadContentAuthority {
                sha256: crate::hash::sha256(b"hello"),
                size: 5,
            })
        );

        assert_eq!(
            files[1].node.kind,
            PayloadNodeKind::Symlink {
                target: "tool".to_string()
            }
        );
        assert_eq!(files[1].node.mode, libc::S_IFLNK | 0o777);
        assert!(files[1].content_authority.is_none());
        assert_eq!(
            files[2].node.kind,
            PayloadNodeKind::CharacterDevice {
                major: 10,
                minor: 200
            }
        );
        assert_eq!(files[2].node.mode, libc::S_IFCHR | 0o600);
        assert_eq!(files[3].node.kind, PayloadNodeKind::Fifo);
        assert_eq!(files[3].node.mode, libc::S_IFIFO | 0o620);
    }

    #[test]
    fn resolves_forward_hardlink_chains_to_one_canonical_anchor() {
        let mut builder = Builder::new(Vec::new());
        let mut second = header("usr/bin/second", EntryType::Link, 0, 0, 0, 0, 0);
        second.set_link_name("usr/bin/first").unwrap();
        append(&mut builder, second, &[]);
        let mut first = header("usr/bin/first", EntryType::Link, 0, 0, 0, 0, 0);
        first.set_link_name("usr/bin/anchor").unwrap();
        append(&mut builder, first, &[]);
        append(
            &mut builder,
            header("usr/bin/anchor", EntryType::Regular, 0o755, 10, 20, 30, 7),
            b"payload",
        );

        let files = parse_archive(builder.into_inner().unwrap()).unwrap();
        let identity = "path:/usr/bin/anchor".to_string();
        for file in &files[..2] {
            assert_eq!(
                file.node.kind,
                PayloadNodeKind::Hardlink {
                    target: "/usr/bin/anchor".to_string(),
                    identity: identity.clone(),
                }
            );
            assert!(file.content.is_empty());
            assert!(file.content_authority.is_none());
        }
        assert_eq!(
            files[2].node.kind,
            PayloadNodeKind::Regular {
                hardlink_identity: Some(identity),
            }
        );
        assert_eq!(files[2].content, b"payload");
    }

    #[test]
    fn rejects_hardlinks_whose_target_is_absent_from_the_archive() {
        let mut builder = Builder::new(Vec::new());
        let mut link = header("usr/bin/link", EntryType::Link, 0, 0, 0, 0, 0);
        link.set_link_name("usr/bin/missing").unwrap();
        append(&mut builder, link, &[]);

        let error = parse_archive(builder.into_inner().unwrap())
            .expect_err("missing hardlink targets must not be guessed");
        assert!(
            error
                .to_string()
                .contains("targets missing payload node /usr/bin/missing"),
            "{error}"
        );
    }

    #[test]
    fn rejects_hardlink_cycles_with_the_exact_cycle() {
        let mut builder = Builder::new(Vec::new());
        let mut first = header("usr/bin/a", EntryType::Link, 0, 0, 0, 0, 0);
        first.set_link_name("usr/bin/b").unwrap();
        append(&mut builder, first, &[]);
        let mut second = header("usr/bin/b", EntryType::Link, 0, 0, 0, 0, 0);
        second.set_link_name("usr/bin/a").unwrap();
        append(&mut builder, second, &[]);

        let error = parse_archive(builder.into_inner().unwrap())
            .expect_err("hardlink cycles must not be flattened");
        assert!(
            error
                .to_string()
                .contains("hardlink cycle starts at /usr/bin/a"),
            "{error}"
        );
    }
}
