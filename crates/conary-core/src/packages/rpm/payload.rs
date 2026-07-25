// conary-core/src/packages/rpm/payload.rs

//! Exact RPM header and payload projection.
//!
//! RPM's header is authoritative for installed metadata. The CPIO stream is
//! only authoritative for entry association and bytes; in particular, its
//! numeric uid/gid and inode fields must not override RPM's named ownership or
//! `FILEDEVICES`/`FILEINODES` hard-link identity.

use crate::compression::{self, CompressionFormat};
use crate::error::{Error, Result};
use crate::packages::archive_utils::normalize_path;
use crate::packages::common::MAX_EXTRACTION_FILE_SIZE;
use crate::packages::traits::ExtractedFile;
use crate::payload::{
    PayloadContentAuthority, PayloadIdentity, PayloadNode, PayloadNodeKind, PayloadTimestamp,
};
use md5::Md5;
use rpm::{DigestAlgorithm, FileFlags, IndexTag, Package};
use sha2::{Digest, Sha224, Sha256, Sha384, Sha512};
use sha3::{Digest as Sha3Digest, Sha3_256, Sha3_512};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{self, Read};

const CPIO_HEADER_SIZE: usize = 110;
const CPIO_NEWC_MAGIC: &[u8; 6] = b"070701";
const CPIO_CRC_MAGIC: &[u8; 6] = b"070702";
const CPIO_STRIPPED_MAGIC: &[u8; 6] = b"07070X";
const CPIO_TRAILER: &str = "TRAILER!!!";
const MAX_CPIO_NAME_SIZE: u64 = 4096;

#[derive(Debug, Clone)]
struct HeaderRecord {
    path: String,
    mode: u32,
    user: String,
    group: String,
    mtime: u32,
    size: u64,
    ghost: bool,
    digest: Option<DeclaredDigest>,
    link_target: Option<String>,
    caps: Option<String>,
    ima_signature: Option<String>,
    device: u32,
    inode: u32,
    rdev: u16,
    nlink: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeclaredDigest {
    algorithm: DigestAlgorithm,
    hex: String,
}

#[derive(Debug)]
struct PayloadMember {
    header_index: usize,
    content: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchiveFlavor {
    Newc,
    Stripped,
}

pub(super) fn parse(package: &Package) -> Result<Vec<ExtractedFile>> {
    require_cpio_payload(package)?;
    let records = header_records(package)?;
    let members = payload_members(package, &records)?;
    let mut payload_by_index = HashMap::with_capacity(members.len());
    for member in members {
        if payload_by_index
            .insert(member.header_index, member.content)
            .is_some()
        {
            return Err(parse_error(format!(
                "RPM payload contains header index {} more than once",
                member.header_index
            )));
        }
    }

    for (index, record) in records.iter().enumerate() {
        if record.ghost {
            if payload_by_index.contains_key(&index) {
                return Err(parse_error(format!(
                    "RPM ghost path {} unexpectedly appears in the payload",
                    record.path
                )));
            }
        } else if !payload_by_index.contains_key(&index) {
            return Err(parse_error(format!(
                "RPM header path {} is missing from the payload",
                record.path
            )));
        }
    }
    if payload_by_index.keys().any(|index| *index >= records.len()) {
        return Err(parse_error(
            "RPM payload references a header index outside the file table",
        ));
    }

    let hardlink_groups = hardlink_groups(&records)?;
    let mut group_by_index = HashMap::new();
    for group in &hardlink_groups {
        for index in group {
            group_by_index.insert(*index, group.as_slice());
        }
    }

    let mut output = Vec::with_capacity(records.len());
    let mut emitted_hardlink_groups = HashSet::new();
    for (index, record) in records.iter().enumerate() {
        if record.ghost {
            continue;
        }
        if let Some(group) = group_by_index.get(&index) {
            let anchor = group[0];
            if !emitted_hardlink_groups.insert(anchor) {
                continue;
            }
            emit_hardlink_group(group, &records, &mut payload_by_index, &mut output)?;
        } else {
            let content = payload_by_index
                .remove(&index)
                .expect("payload completeness checked");
            output.push(project_single(record, content)?);
        }
    }
    if !payload_by_index.is_empty() {
        return Err(parse_error(
            "RPM payload projection left unconsumed header members",
        ));
    }
    Ok(output)
}

fn require_cpio_payload(package: &Package) -> Result<()> {
    let format = package
        .metadata
        .header
        .get_entry_data_as_string(IndexTag::RPMTAG_PAYLOADFORMAT)
        .map_err(|error| parse_error(format!("read RPM payload format: {error}")))?;
    if format != "cpio" {
        return Err(parse_error(format!(
            "RPM payload format {format:?} is not the documented cpio grammar"
        )));
    }
    Ok(())
}

fn header_records(package: &Package) -> Result<Vec<HeaderRecord>> {
    let entries = package
        .metadata
        .get_file_entries()
        .map_err(|error| parse_error(format!("read RPM file header: {error}")))?;
    if entries.is_empty() {
        return Ok(Vec::new());
    }
    let count = entries.len();
    let devices = required_u32_array(package, IndexTag::RPMTAG_FILEDEVICES, count)?;
    let inodes = required_u32_array(package, IndexTag::RPMTAG_FILEINODES, count)?;
    let rdevs = required_u16_array(package, IndexTag::RPMTAG_FILERDEVS, count)?;
    let nlinks = optional_u32_array(package, IndexTag::RPMTAG_FILENLINKS, count)?;
    let mut seen_paths = HashSet::with_capacity(count);
    entries
        .into_iter()
        .enumerate()
        .map(|(index, entry)| {
            let raw_path = entry.path();
            let raw_path = raw_path.to_str().ok_or_else(|| {
                parse_error(format!("RPM header path at index {index} is not UTF-8"))
            })?;
            let path = normalize_path(raw_path)
                .map_err(|error| parse_error(format!("invalid RPM header path: {error}")))?;
            if raw_path != path {
                return Err(parse_error(format!(
                    "RPM header path {raw_path:?} is not canonical absolute form {path:?}"
                )));
            }
            if !seen_paths.insert(path.clone()) {
                return Err(parse_error(format!("duplicate RPM header path {path}")));
            }
            let user = exact_identity(entry.user(), "user", &path)?;
            let group = exact_identity(entry.group(), "group", &path)?;
            let link_target = entry
                .linkto()
                .map(|target| exact_text(target, "symlink target", &path))
                .transpose()?;
            let caps = entry
                .caps()
                .filter(|value| !value.is_empty())
                .map(str::to_string);
            let ima_signature = entry
                .ima_signature()
                .filter(|value| !value.is_empty())
                .map(str::to_string);
            let digest = entry.digest().map(|digest| DeclaredDigest {
                algorithm: digest.algorithm(),
                hex: digest.as_hex().to_string(),
            });
            Ok(HeaderRecord {
                path,
                mode: u32::from(entry.mode().raw_mode()),
                user,
                group,
                mtime: entry.modified_at().0,
                size: entry.size() as u64,
                ghost: entry.flags().contains(FileFlags::GHOST),
                digest,
                link_target,
                caps,
                ima_signature,
                device: devices[index],
                inode: inodes[index],
                rdev: rdevs[index],
                nlink: nlinks.as_ref().map(|values| values[index]),
            })
        })
        .collect()
}

fn required_u32_array(package: &Package, tag: IndexTag, count: usize) -> Result<Vec<u32>> {
    let values = package
        .metadata
        .header
        .get_entry_data_as_u32_array(tag)
        .map_err(|error| parse_error(format!("read RPM {tag}: {error}")))?;
    require_array_len(tag, values, count)
}

fn required_u16_array(package: &Package, tag: IndexTag, count: usize) -> Result<Vec<u16>> {
    let values = package
        .metadata
        .header
        .get_entry_data_as_u16_array(tag)
        .map_err(|error| parse_error(format!("read RPM {tag}: {error}")))?;
    require_array_len(tag, values, count)
}

fn optional_u32_array(package: &Package, tag: IndexTag, count: usize) -> Result<Option<Vec<u32>>> {
    match package.metadata.header.get_entry_data_as_u32_array(tag) {
        Ok(values) => require_array_len(tag, values, count).map(Some),
        Err(rpm::Error::TagNotFound(_)) => Ok(None),
        Err(error) => Err(parse_error(format!("read RPM {tag}: {error}"))),
    }
}

fn require_array_len<T>(tag: IndexTag, values: Vec<T>, count: usize) -> Result<Vec<T>> {
    if values.len() != count {
        return Err(parse_error(format!(
            "RPM {tag} has {} values for {count} file-header entries",
            values.len()
        )));
    }
    Ok(values)
}

fn payload_members(package: &Package, records: &[HeaderRecord]) -> Result<Vec<PayloadMember>> {
    if package.payload.is_empty() {
        return Ok(Vec::new());
    }
    let compressor = package
        .metadata
        .get_payload_compressor()
        .map_err(|error| parse_error(format!("read RPM payload compressor: {error}")))?;
    let reader = payload_decoder(&package.payload, compressor)?;
    RpmPayloadReader::new(reader, records).read_all()
}

fn payload_decoder<'a>(
    payload: &'a [u8],
    compressor: rpm::CompressionType,
) -> Result<Box<dyn Read + 'a>> {
    let cursor = io::Cursor::new(payload);
    match compressor {
        rpm::CompressionType::None => {
            limited_decoder(cursor, CompressionFormat::None, "uncompressed")
        }
        rpm::CompressionType::Gzip => limited_decoder(cursor, CompressionFormat::Gzip, "gzip"),
        rpm::CompressionType::Xz => limited_decoder(cursor, CompressionFormat::Xz, "xz"),
        rpm::CompressionType::Zstd => limited_decoder(cursor, CompressionFormat::Zstd, "zstd"),
        rpm::CompressionType::Bzip2 => Ok(Box::new(
            bzip2::read::BzDecoder::new(cursor).take(compression::MAX_DECOMPRESS_SIZE + 1),
        )),
    }
}

fn limited_decoder<'a>(
    cursor: io::Cursor<&'a [u8]>,
    format: CompressionFormat,
    label: &str,
) -> Result<Box<dyn Read + 'a>> {
    compression::create_decoder_limited(cursor, format, compression::MAX_DECOMPRESS_SIZE)
        .map_err(|error| parse_error(format!("create RPM {label} payload decoder: {error}")))
}

struct RpmPayloadReader<'a> {
    reader: Box<dyn Read + 'a>,
    records: &'a [HeaderRecord],
    members: Vec<PayloadMember>,
    seen: HashSet<usize>,
    flavor: Option<ArchiveFlavor>,
    standard_path_indexes: HashMap<&'a str, usize>,
    total_content: u64,
}

impl<'a> RpmPayloadReader<'a> {
    fn new(reader: Box<dyn Read + 'a>, records: &'a [HeaderRecord]) -> Self {
        Self {
            reader,
            records,
            members: Vec::new(),
            seen: HashSet::new(),
            flavor: None,
            standard_path_indexes: records
                .iter()
                .enumerate()
                .filter(|(_, record)| !record.ghost)
                .map(|(index, record)| (record.path.as_str(), index))
                .collect(),
            total_content: 0,
        }
    }

    fn read_all(mut self) -> Result<Vec<PayloadMember>> {
        loop {
            let mut magic = [0_u8; 6];
            self.reader
                .read_exact(&mut magic)
                .map_err(|error| parse_error(format!("read RPM CPIO magic: {error}")))?;
            if &magic == CPIO_STRIPPED_MAGIC {
                self.require_flavor(ArchiveFlavor::Stripped)?;
                self.read_stripped_member()?;
            } else if &magic == CPIO_NEWC_MAGIC || &magic == CPIO_CRC_MAGIC {
                self.require_flavor(ArchiveFlavor::Newc)?;
                if self.read_newc_member(magic)? {
                    break;
                }
            } else {
                return Err(parse_error(format!(
                    "RPM payload has invalid CPIO magic {:?}",
                    String::from_utf8_lossy(&magic)
                )));
            }
        }
        Ok(self.members)
    }

    fn require_flavor(&mut self, flavor: ArchiveFlavor) -> Result<()> {
        if let Some(existing) = self.flavor
            && existing != flavor
        {
            return Err(parse_error(
                "RPM payload mixes standard and stripped CPIO entries",
            ));
        }
        self.flavor = Some(flavor);
        Ok(())
    }

    fn read_stripped_member(&mut self) -> Result<()> {
        let index = self.read_hex_u32("stripped CPIO file index")? as usize;
        self.read_zero_padding(2, "stripped CPIO header")?;
        let record = self.records.get(index).ok_or_else(|| {
            parse_error(format!(
                "stripped RPM CPIO index {index} is outside {} header entries",
                self.records.len()
            ))
        })?;
        if record.ghost {
            return Err(parse_error(format!(
                "stripped RPM CPIO references ghost path {}",
                record.path
            )));
        }
        let content = self.read_content(record.size, &record.path)?;
        self.finish_member(index, content)
    }

    /// Read a standard newc member. Returns `true` for the trailer.
    fn read_newc_member(&mut self, magic: [u8; 6]) -> Result<bool> {
        let _inode = self.read_hex_u32("inode")?;
        let _mode = self.read_hex_u32("mode")?;
        let _uid = self.read_hex_u32("uid")?;
        let _gid = self.read_hex_u32("gid")?;
        let _nlink = self.read_hex_u32("link count")?;
        let _mtime = self.read_hex_u32("mtime")?;
        let size = u64::from(self.read_hex_u32("content size")?);
        let _device_major = self.read_hex_u32("device major")?;
        let _device_minor = self.read_hex_u32("device minor")?;
        let _rdev_major = self.read_hex_u32("rdev major")?;
        let _rdev_minor = self.read_hex_u32("rdev minor")?;
        let name_size = u64::from(self.read_hex_u32("name size")?);
        let checksum = self.read_hex_u32("checksum")?;
        if name_size == 0 || name_size > MAX_CPIO_NAME_SIZE {
            return Err(parse_error(format!(
                "RPM CPIO name size {name_size} is outside 1..={MAX_CPIO_NAME_SIZE}"
            )));
        }
        let mut name = vec![0_u8; name_size as usize];
        self.reader
            .read_exact(&mut name)
            .map_err(|error| parse_error(format!("read RPM CPIO name: {error}")))?;
        if name.last() != Some(&0) || name[..name.len() - 1].contains(&0) {
            return Err(parse_error(
                "RPM CPIO name must contain exactly one trailing NUL",
            ));
        }
        name.pop();
        let name = String::from_utf8(name)
            .map_err(|error| parse_error(format!("RPM CPIO name is not UTF-8: {error}")))?;
        self.read_alignment_padding(CPIO_HEADER_SIZE + name_size as usize, "CPIO name")?;
        if name == CPIO_TRAILER {
            if size != 0 {
                return Err(parse_error("RPM CPIO trailer carries content"));
            }
            if checksum != 0 {
                return Err(parse_error("RPM CPIO trailer carries a checksum"));
            }
            return Ok(true);
        }
        let path = normalize_path(&name)
            .map_err(|error| parse_error(format!("invalid RPM CPIO path: {error}")))?;
        let index = *self
            .standard_path_indexes
            .get(path.as_str())
            .ok_or_else(|| {
                parse_error(format!(
                    "RPM CPIO path {path} does not identify a non-ghost header entry"
                ))
            })?;
        let content = self.read_content(size, &path)?;
        if &magic == CPIO_CRC_MAGIC {
            let actual = content
                .iter()
                .fold(0_u32, |sum, byte| sum.wrapping_add(u32::from(*byte)));
            if actual != checksum {
                return Err(parse_error(format!(
                    "RPM CPIO CRC mismatch for {path}: expected {checksum:#x}, got {actual:#x}"
                )));
            }
        } else if checksum != 0 {
            return Err(parse_error(format!(
                "RPM newc entry {path} has a nonzero checksum field"
            )));
        }
        self.finish_member(index, content)?;
        Ok(false)
    }

    fn finish_member(&mut self, index: usize, content: Vec<u8>) -> Result<()> {
        if !self.seen.insert(index) {
            return Err(parse_error(format!(
                "RPM payload repeats header path {}",
                self.records[index].path
            )));
        }
        self.members.push(PayloadMember {
            header_index: index,
            content,
        });
        Ok(())
    }

    fn read_content(&mut self, size: u64, path: &str) -> Result<Vec<u8>> {
        if size > MAX_EXTRACTION_FILE_SIZE {
            return Err(parse_error(format!(
                "RPM payload node {path} is {size} bytes; maximum is {MAX_EXTRACTION_FILE_SIZE}"
            )));
        }
        self.total_content = self.total_content.checked_add(size).ok_or_else(|| {
            parse_error("RPM payload cumulative content size arithmetic overflow")
        })?;
        if self.total_content > compression::MAX_DECOMPRESS_SIZE {
            return Err(parse_error(format!(
                "RPM payload content exceeds {} bytes",
                compression::MAX_DECOMPRESS_SIZE
            )));
        }
        let mut content = vec![0_u8; size as usize];
        self.reader
            .read_exact(&mut content)
            .map_err(|error| parse_error(format!("read RPM payload node {path}: {error}")))?;
        self.read_alignment_padding(size as usize, "CPIO content")?;
        Ok(content)
    }

    fn read_hex_u32(&mut self, field: &str) -> Result<u32> {
        let mut bytes = [0_u8; 8];
        self.reader
            .read_exact(&mut bytes)
            .map_err(|error| parse_error(format!("read RPM CPIO {field}: {error}")))?;
        let text = std::str::from_utf8(&bytes)
            .map_err(|error| parse_error(format!("RPM CPIO {field} is not ASCII: {error}")))?;
        u32::from_str_radix(text, 16)
            .map_err(|error| parse_error(format!("RPM CPIO {field} is not hexadecimal: {error}")))
    }

    fn read_alignment_padding(&mut self, length: usize, field: &str) -> Result<()> {
        self.read_zero_padding((4 - (length % 4)) % 4, field)
    }

    fn read_zero_padding(&mut self, count: usize, field: &str) -> Result<()> {
        let mut padding = [0_u8; 3];
        self.reader
            .read_exact(&mut padding[..count])
            .map_err(|error| parse_error(format!("read RPM {field} padding: {error}")))?;
        if padding[..count].iter().any(|byte| *byte != 0) {
            return Err(parse_error(format!(
                "RPM {field} padding contains nonzero bytes"
            )));
        }
        Ok(())
    }
}

fn hardlink_groups(records: &[HeaderRecord]) -> Result<Vec<Vec<usize>>> {
    let mut groups: BTreeMap<(u32, u32), Vec<usize>> = BTreeMap::new();
    for (index, record) in records.iter().enumerate() {
        if !record.ghost && mode_type(record.mode) == libc::S_IFREG && record.inode != 0 {
            groups
                .entry((record.device, record.inode))
                .or_default()
                .push(index);
        }
    }
    groups
        .into_values()
        .filter(|group| group.len() > 1)
        .map(|group| {
            for index in &group {
                if let Some(nlink) = records[*index].nlink
                    && nlink as usize != group.len()
                {
                    return Err(parse_error(format!(
                        "RPM hardlink {} declares {nlink} links but header identity contains {}",
                        records[*index].path,
                        group.len()
                    )));
                }
            }
            Ok(group)
        })
        .collect()
}

fn emit_hardlink_group(
    group: &[usize],
    records: &[HeaderRecord],
    payload_by_index: &mut HashMap<usize, Vec<u8>>,
    output: &mut Vec<ExtractedFile>,
) -> Result<()> {
    let anchor_index = group[0];
    let anchor = &records[anchor_index];
    for index in &group[1..] {
        require_same_inode_metadata(anchor, &records[*index])?;
    }
    let mut effective_content: Option<Vec<u8>> = None;
    for index in group {
        let content = payload_by_index
            .remove(index)
            .expect("payload completeness checked");
        if content.is_empty() {
            continue;
        }
        if let Some(existing) = &effective_content {
            if existing != &content {
                return Err(parse_error(format!(
                    "RPM hardlink group anchored at {} carries conflicting payload bytes",
                    anchor.path
                )));
            }
        } else {
            effective_content = Some(content);
        }
    }
    let content = effective_content.unwrap_or_default();
    require_regular_content(anchor, &content)?;
    let identity = format!("rpm:{}:{}", anchor.device, anchor.inode);
    let mut anchor_node = source_node(anchor)?;
    anchor_node.kind = PayloadNodeKind::Regular {
        hardlink_identity: Some(identity.clone()),
    };
    let authority = content_authority(&content);
    validate_projected(&anchor.path, &anchor_node, Some(&authority))?;
    output.push(ExtractedFile {
        path: anchor.path.clone(),
        node: anchor_node.clone(),
        content: content.clone(),
        content_authority: Some(authority),
    });
    for index in &group[1..] {
        let record = &records[*index];
        let mut node = anchor_node.clone();
        node.kind = PayloadNodeKind::Hardlink {
            target: anchor.path.clone(),
            identity: identity.clone(),
        };
        validate_projected(&record.path, &node, None)?;
        output.push(ExtractedFile {
            path: record.path.clone(),
            node,
            content: Vec::new(),
            content_authority: None,
        });
    }
    Ok(())
}

fn require_same_inode_metadata(anchor: &HeaderRecord, member: &HeaderRecord) -> Result<()> {
    if anchor.mode != member.mode
        || anchor.user != member.user
        || anchor.group != member.group
        || anchor.mtime != member.mtime
        || anchor.size != member.size
        || anchor.digest != member.digest
        || anchor.caps != member.caps
        || anchor.ima_signature != member.ima_signature
    {
        return Err(parse_error(format!(
            "RPM hardlink members {} and {} declare different inode metadata",
            anchor.path, member.path
        )));
    }
    Ok(())
}

fn project_single(record: &HeaderRecord, content: Vec<u8>) -> Result<ExtractedFile> {
    let node = source_node(record)?;
    let content_authority = if matches!(node.kind, PayloadNodeKind::Regular { .. }) {
        require_regular_content(record, &content)?;
        Some(content_authority(&content))
    } else {
        require_non_regular_content(record, &content)?;
        None
    };
    validate_projected(&record.path, &node, content_authority.as_ref())?;
    Ok(ExtractedFile {
        path: record.path.clone(),
        node,
        content: if content_authority.is_some() {
            content
        } else {
            Vec::new()
        },
        content_authority,
    })
}

fn source_node(record: &HeaderRecord) -> Result<PayloadNode> {
    let kind = match mode_type(record.mode) {
        libc::S_IFREG => {
            if record.link_target.is_some() {
                return Err(parse_error(format!(
                    "regular RPM node {} declares a symlink target",
                    record.path
                )));
            }
            PayloadNodeKind::Regular {
                hardlink_identity: None,
            }
        }
        libc::S_IFDIR => PayloadNodeKind::Directory,
        libc::S_IFLNK => {
            let target = record.link_target.clone().ok_or_else(|| {
                parse_error(format!(
                    "RPM symlink {} has no FILELINKTOS target",
                    record.path
                ))
            })?;
            PayloadNodeKind::Symlink { target }
        }
        libc::S_IFBLK => {
            let (major, minor) = rpm_device(record.rdev);
            PayloadNodeKind::BlockDevice { major, minor }
        }
        libc::S_IFCHR => {
            let (major, minor) = rpm_device(record.rdev);
            PayloadNodeKind::CharacterDevice { major, minor }
        }
        libc::S_IFIFO => PayloadNodeKind::Fifo,
        libc::S_IFSOCK => PayloadNodeKind::Socket,
        other => {
            return Err(parse_error(format!(
                "RPM node {} uses unsupported mode type {other:#o}",
                record.path
            )));
        }
    };
    let mut xattrs = BTreeMap::new();
    if let Some(text) = &record.caps {
        if !matches!(kind, PayloadNodeKind::Regular { .. }) {
            return Err(parse_error(format!(
                "non-regular RPM node {} declares file capabilities",
                record.path
            )));
        }
        xattrs.insert(
            "security.capability".to_string(),
            parse_file_capabilities(text, &record.path)?,
        );
    }
    if let Some(hex_signature) = &record.ima_signature {
        if !matches!(kind, PayloadNodeKind::Regular { .. }) {
            return Err(parse_error(format!(
                "non-regular RPM node {} declares an IMA signature",
                record.path
            )));
        }
        let signature = hex::decode(hex_signature).map_err(|error| {
            parse_error(format!(
                "RPM IMA signature for {} is not hexadecimal: {error}",
                record.path
            ))
        })?;
        if signature.is_empty() {
            return Err(parse_error(format!(
                "RPM IMA signature for {} is empty",
                record.path
            )));
        }
        xattrs.insert("security.ima".to_string(), signature);
    }
    let node = PayloadNode {
        kind,
        mode: record.mode,
        user: PayloadIdentity::Named {
            name: record.user.clone(),
        },
        group: PayloadIdentity::Named {
            name: record.group.clone(),
        },
        mtime: PayloadTimestamp {
            seconds: i64::from(record.mtime),
            nanoseconds: 0,
        },
        xattrs,
    };
    node.validate()
        .map_err(|error| parse_error(format!("invalid RPM node {}: {error}", record.path)))?;
    Ok(node)
}

fn require_regular_content(record: &HeaderRecord, content: &[u8]) -> Result<()> {
    if content.len() as u64 != record.size {
        return Err(parse_error(format!(
            "RPM regular node {} declares {} bytes but payload yields {}",
            record.path,
            record.size,
            content.len()
        )));
    }
    let digest = record.digest.as_ref().ok_or_else(|| {
        parse_error(format!(
            "RPM regular node {} has no declared file digest",
            record.path
        ))
    })?;
    let actual = digest_hex(digest.algorithm, content);
    if actual != digest.hex {
        return Err(parse_error(format!(
            "RPM file digest mismatch for {}: expected {}, got {actual}",
            record.path, digest.hex
        )));
    }
    Ok(())
}

fn require_non_regular_content(record: &HeaderRecord, content: &[u8]) -> Result<()> {
    match mode_type(record.mode) {
        libc::S_IFLNK => {
            let target = record
                .link_target
                .as_deref()
                .expect("source_node requires symlink target");
            if content != target.as_bytes() {
                return Err(parse_error(format!(
                    "RPM symlink {} payload target differs from FILELINKTOS",
                    record.path
                )));
            }
            if content.len() as u64 != record.size {
                return Err(parse_error(format!(
                    "RPM symlink {} declares {} bytes but target has {}",
                    record.path,
                    record.size,
                    content.len()
                )));
            }
        }
        _ if !content.is_empty() => {
            return Err(parse_error(format!(
                "non-regular RPM node {} carries {} payload bytes",
                record.path,
                content.len()
            )));
        }
        _ => {}
    }
    Ok(())
}

fn validate_projected(
    path: &str,
    node: &PayloadNode,
    content: Option<&PayloadContentAuthority>,
) -> Result<()> {
    node.validate_content(content)
        .map_err(|error| parse_error(format!("invalid RPM payload authority for {path}: {error}")))
}

fn content_authority(content: &[u8]) -> PayloadContentAuthority {
    PayloadContentAuthority {
        sha256: crate::hash::sha256(content),
        size: content.len() as u64,
    }
}

fn digest_hex(algorithm: DigestAlgorithm, content: &[u8]) -> String {
    match algorithm {
        DigestAlgorithm::Md5 => hex::encode(Md5::digest(content)),
        DigestAlgorithm::Sha2_224 => hex::encode(Sha224::digest(content)),
        DigestAlgorithm::Sha2_256 => hex::encode(Sha256::digest(content)),
        DigestAlgorithm::Sha2_384 => hex::encode(Sha384::digest(content)),
        DigestAlgorithm::Sha2_512 => hex::encode(Sha512::digest(content)),
        DigestAlgorithm::Sha3_256 => hex::encode(<Sha3_256 as Sha3Digest>::digest(content)),
        DigestAlgorithm::Sha3_512 => hex::encode(<Sha3_512 as Sha3Digest>::digest(content)),
    }
}

fn parse_file_capabilities(text: &str, path: &str) -> Result<Vec<u8>> {
    const REVISION_2: u32 = 0x0200_0000;
    const EFFECTIVE: u32 = 0x0000_0001;
    let mut permitted = 0_u64;
    let mut inheritable = 0_u64;
    let mut effective = 0_u64;
    let mut saw_clause = false;
    for clause in text.split_ascii_whitespace() {
        saw_clause = true;
        let operator_index = clause
            .char_indices()
            .find_map(|(index, character)| matches!(character, '=' | '+' | '-').then_some(index))
            .ok_or_else(|| {
                parse_error(format!(
                    "RPM file capability for {path} has no set operator: {clause:?}"
                ))
            })?;
        let names = &clause[..operator_index];
        let operation = clause.as_bytes()[operator_index] as char;
        let flags = &clause[operator_index + 1..];
        if names.is_empty()
            || flags
                .bytes()
                .any(|flag| !matches!(flag, b'e' | b'i' | b'p'))
        {
            return Err(parse_error(format!(
                "RPM file capability for {path} is outside libcap text grammar: {clause:?}"
            )));
        }
        let mut selected = 0_u64;
        if names == "all" {
            selected = (1_u64 << crate::ccs::manifest::LINUX_FILE_CAPABILITY_NAMES.len()) - 1;
        } else {
            for name in names.split(',') {
                let index = crate::ccs::manifest::LINUX_FILE_CAPABILITY_NAMES
                    .iter()
                    .position(|candidate| *candidate == name)
                    .ok_or_else(|| {
                        parse_error(format!(
                            "RPM file capability for {path} names unknown capability {name:?}"
                        ))
                    })?;
                selected |= 1_u64 << index;
            }
        }
        apply_capability_operation(&mut effective, selected, operation, flags.contains('e'));
        apply_capability_operation(&mut inheritable, selected, operation, flags.contains('i'));
        apply_capability_operation(&mut permitted, selected, operation, flags.contains('p'));
    }
    if !saw_clause {
        return Err(parse_error(format!(
            "RPM file capability for {path} is empty"
        )));
    }
    if effective != 0 && effective != permitted {
        return Err(parse_error(format!(
            "RPM file capability for {path} has an effective set not representable by Linux VFS capability revision 2"
        )));
    }
    let mut magic = REVISION_2;
    if effective != 0 {
        magic |= EFFECTIVE;
    }
    let mut encoded = Vec::with_capacity(20);
    encoded.extend_from_slice(&magic.to_le_bytes());
    for word in 0..2 {
        encoded.extend_from_slice(&((permitted >> (word * 32)) as u32).to_le_bytes());
        encoded.extend_from_slice(&((inheritable >> (word * 32)) as u32).to_le_bytes());
    }
    Ok(encoded)
}

fn apply_capability_operation(set: &mut u64, selected: u64, operation: char, flag_present: bool) {
    match operation {
        '=' => {
            *set &= !selected;
            if flag_present {
                *set |= selected;
            }
        }
        '+' if flag_present => *set |= selected,
        '-' if flag_present => *set &= !selected,
        '+' | '-' => {}
        _ => unreachable!("capability operation validated"),
    }
}

fn exact_identity(value: &str, field: &str, path: &str) -> Result<String> {
    if value.is_empty() || value.contains('\0') {
        return Err(parse_error(format!(
            "RPM {field} identity for {path} must be non-empty and NUL-free"
        )));
    }
    Ok(value.to_string())
}

fn exact_text(value: &str, field: &str, path: &str) -> Result<String> {
    if value.is_empty() || value.contains('\0') {
        return Err(parse_error(format!(
            "RPM {field} for {path} must be non-empty and NUL-free"
        )));
    }
    Ok(value.to_string())
}

fn mode_type(mode: u32) -> u32 {
    mode & libc::S_IFMT
}

fn rpm_device(value: u16) -> (u64, u64) {
    (u64::from(value >> 8), u64::from(value & 0xff))
}

fn parse_error(message: impl Into<String>) -> Error {
    Error::ParseError(message.into())
}

#[cfg(test)]
mod tests {
    use super::{apply_capability_operation, parse_file_capabilities};

    #[test]
    fn capability_text_projects_exact_kernel_xattr() {
        let encoded =
            parse_file_capabilities("cap_net_bind_service,cap_net_raw=ep", "/usr/bin/server")
                .expect("parse capability");
        assert_eq!(encoded.len(), 20);
        assert_eq!(
            u32::from_le_bytes(encoded[0..4].try_into().unwrap()),
            0x0200_0001
        );
        assert_eq!(
            u32::from_le_bytes(encoded[4..8].try_into().unwrap()),
            (1 << 10) | (1 << 13)
        );
    }

    #[test]
    fn capability_operations_apply_in_order() {
        let mut value = 0_u64;
        apply_capability_operation(&mut value, 0b11, '=', true);
        apply_capability_operation(&mut value, 0b01, '-', true);
        apply_capability_operation(&mut value, 0b100, '+', true);
        assert_eq!(value, 0b110);
    }

    #[test]
    fn capability_text_rejects_nonrepresentable_effective_subset() {
        let error =
            parse_file_capabilities("cap_net_bind_service=p cap_net_raw=e", "/usr/bin/server")
                .expect_err("effective subset must fail");
        assert!(error.to_string().contains("not representable"));
    }
}
