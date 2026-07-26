// conary-core/src/packages/rpm/payload.rs

//! Exact RPM header and payload projection.
//!
//! RPM's header is authoritative for installed metadata. The CPIO stream is
//! only authoritative for entry association and bytes; in particular, its
//! numeric uid/gid and inode fields must not override RPM's named ownership or
//! `FILEDEVICES`/`FILEINODES` hard-link identity.

use crate::error::{Error, Result};
use crate::packages::payload::{
    PackagePayload, PackagePayloadFile, PayloadSpool, ReopenablePayload,
};
use crate::payload::{
    PayloadContentAuthority, PayloadIdentity, PayloadNode, PayloadNodeKind, PayloadTimestamp,
};
use md5::Md5;
use rpm::{IndexTag, Package};
use sha1::{Digest as Digest10, Sha1};
use sha2::{Digest as Digest11, Sha224, Sha256, Sha384, Sha512};
use sha3::{Sha3_256, Sha3_512};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;

mod header;
mod stream;

use header::{HeaderRecord, RpmFileDigestAlgorithm, header_records};

pub(super) fn parse_stream(package: &Package, payload: Box<dyn Read>) -> Result<PackagePayload> {
    require_cpio_payload(package)?;
    let records = header_records(package)?;
    let spool = PayloadSpool::new(stream::required_spool_bytes(&records)?)?;
    let members = stream::parse_members(package, payload, &records, &spool)?;
    let mut payload_by_index = HashMap::with_capacity(members.len());
    for member in members {
        let header_index = member.header_index;
        if payload_by_index.insert(header_index, member).is_some() {
            return Err(parse_error(format!(
                "RPM payload contains header index {} more than once",
                header_index
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
            let member = payload_by_index
                .remove(&index)
                .expect("payload completeness checked");
            output.push(project_single(record, member)?);
        }
    }
    if !payload_by_index.is_empty() {
        return Err(parse_error(
            "RPM payload projection left unconsumed header members",
        ));
    }
    Ok(PackagePayload::new(output))
}

#[cfg(test)]
fn parse(package: &Package) -> Result<PackagePayload> {
    parse_stream(
        package,
        Box::new(std::io::Cursor::new(package.payload.clone())),
    )
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
                if let Some(nlink) = records[*index].nlink {
                    require_partial_hardlink_count(&records[*index].path, nlink, group.len())?;
                }
            }
            Ok(group)
        })
        .collect()
}

/// RPM's `rpmlib(PartialHardlinkSets)` ABI explicitly permits a package to
/// contain fewer members than the source inode's link count. It may not claim
/// fewer links than the members it does contain.
///
/// Upstream authority:
/// <https://github.com/rpm-software-management/rpm/blob/a8f0192aee1c08bd1454ed2ac6ebaf506004b55c/lib/rpmds.cc#L995-L997>.
fn require_partial_hardlink_count(path: &str, declared: u32, packaged: usize) -> Result<()> {
    if declared as usize >= packaged {
        return Ok(());
    }
    Err(parse_error(format!(
        "RPM hardlink {path} declares {declared} links but the package contains {packaged} members"
    )))
}

fn emit_hardlink_group(
    group: &[usize],
    records: &[HeaderRecord],
    payload_by_index: &mut HashMap<usize, stream::PayloadMember>,
    output: &mut Vec<PackagePayloadFile>,
) -> Result<()> {
    let anchor_index = group[0];
    let anchor = &records[anchor_index];
    for index in &group[1..] {
        require_same_inode_metadata(anchor, &records[*index])?;
    }
    let mut effective_content: Option<(u64, String, ReopenablePayload)> = None;
    for index in group {
        let member = payload_by_index
            .remove(index)
            .expect("payload completeness checked");
        let sha256 = member
            .sha256
            .ok_or_else(|| parse_error("RPM regular hardlink member has no SHA-256 authority"))?;
        let source = member
            .source
            .ok_or_else(|| parse_error("RPM regular hardlink member has no payload source"))?;
        if member.content_size == 0 {
            effective_content.get_or_insert((0, sha256, source));
            continue;
        }
        if let Some((size, existing, _)) = &effective_content {
            if *size != 0 && (*size != member.content_size || existing != &sha256) {
                return Err(parse_error(format!(
                    "RPM hardlink group anchored at {} carries conflicting payload bytes",
                    anchor.path
                )));
            }
            if *size == 0 {
                effective_content = Some((member.content_size, sha256, source));
            }
        } else {
            effective_content = Some((member.content_size, sha256, source));
        }
    }
    let (size, sha256, source) =
        effective_content.ok_or_else(|| parse_error("RPM hardlink group has no payload source"))?;
    require_regular_content(anchor, size, &source)?;
    let identity = format!("rpm:{}:{}", anchor.device, anchor.inode);
    let mut anchor_node = source_node(anchor)?;
    anchor_node.kind = PayloadNodeKind::Regular {
        hardlink_identity: Some(identity.clone()),
    };
    let authority = PayloadContentAuthority { sha256, size };
    validate_projected(&anchor.path, &anchor_node, Some(&authority))?;
    output.push(PackagePayloadFile::new(
        anchor.path.clone(),
        anchor_node.clone(),
        Some(authority),
        Some(source),
    )?);
    for index in &group[1..] {
        let record = &records[*index];
        let mut node = anchor_node.clone();
        node.kind = PayloadNodeKind::Hardlink {
            target: anchor.path.clone(),
            identity: identity.clone(),
        };
        validate_projected(&record.path, &node, None)?;
        output.push(PackagePayloadFile::new(
            record.path.clone(),
            node,
            None,
            None,
        )?);
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

fn project_single(
    record: &HeaderRecord,
    member: stream::PayloadMember,
) -> Result<PackagePayloadFile> {
    let node = source_node(record)?;
    let (content_authority, source) = if matches!(node.kind, PayloadNodeKind::Regular { .. }) {
        let source = member.source.ok_or_else(|| {
            parse_error(format!(
                "RPM regular node {} has no payload source",
                record.path
            ))
        })?;
        require_regular_content(record, member.content_size, &source)?;
        let sha256 = member.sha256.ok_or_else(|| {
            parse_error(format!(
                "RPM regular node {} has no SHA-256 authority",
                record.path
            ))
        })?;
        (
            Some(PayloadContentAuthority {
                sha256,
                size: member.content_size,
            }),
            Some(source),
        )
    } else {
        if member.content_size != 0 || member.source.is_some() || member.sha256.is_some() {
            return Err(parse_error(format!(
                "non-regular RPM node {} retained ambiguous payload content",
                record.path
            )));
        }
        (None, None)
    };
    validate_projected(&record.path, &node, content_authority.as_ref())?;
    PackagePayloadFile::new(record.path.clone(), node, content_authority, source)
}

fn require_regular_content(
    record: &HeaderRecord,
    content_size: u64,
    source: &ReopenablePayload,
) -> Result<()> {
    if content_size != record.size {
        return Err(parse_error(format!(
            "RPM regular node {} declares {} bytes but payload yields {}",
            record.path, record.size, content_size
        )));
    }
    let digest = record.digest.as_ref().ok_or_else(|| {
        parse_error(format!(
            "RPM regular node {} has no declared file digest",
            record.path
        ))
    })?;
    let mut reader = source.open()?;
    let actual = digest_reader(digest.algorithm, reader.as_mut(), content_size)?;
    if actual != digest.hex {
        return Err(parse_error(format!(
            "RPM file digest mismatch for {}: expected {}, got {actual}",
            record.path, digest.hex
        )));
    }
    let mut trailing = [0_u8; 1];
    if reader.read(&mut trailing)? != 0 {
        return Err(parse_error(format!(
            "RPM payload source for {} exceeds declared size {content_size}",
            record.path
        )));
    }
    Ok(())
}

fn digest_reader(
    algorithm: RpmFileDigestAlgorithm,
    reader: &mut dyn Read,
    size: u64,
) -> Result<String> {
    macro_rules! hash_exact {
        ($hasher:expr) => {{
            let mut hasher = $hasher;
            let mut remaining = size;
            let mut buffer = [0_u8; crate::packages::payload::PAYLOAD_IO_BUFFER_SIZE];
            while remaining > 0 {
                let wanted = usize::try_from(remaining.min(buffer.len() as u64))
                    .expect("bounded by the fixed buffer");
                reader.read_exact(&mut buffer[..wanted]).map_err(|error| {
                    parse_error(format!("read RPM payload for digest verification: {error}"))
                })?;
                hasher.update(&buffer[..wanted]);
                remaining -= wanted as u64;
            }
            hex::encode(hasher.finalize())
        }};
    }
    Ok(match algorithm {
        RpmFileDigestAlgorithm::Md5 => hash_exact!(Md5::new()),
        RpmFileDigestAlgorithm::Sha1 => hash_exact!(Sha1::new()),
        RpmFileDigestAlgorithm::Sha2_224 => hash_exact!(Sha224::new()),
        RpmFileDigestAlgorithm::Sha2_256 => hash_exact!(Sha256::new()),
        RpmFileDigestAlgorithm::Sha2_384 => hash_exact!(Sha384::new()),
        RpmFileDigestAlgorithm::Sha2_512 => hash_exact!(Sha512::new()),
        RpmFileDigestAlgorithm::Sha3_256 => hash_exact!(Sha3_256::new()),
        RpmFileDigestAlgorithm::Sha3_512 => hash_exact!(Sha3_512::new()),
    })
}

#[cfg(test)]
fn digest_hex(algorithm: RpmFileDigestAlgorithm, content: &[u8]) -> String {
    digest_reader(
        algorithm,
        &mut std::io::Cursor::new(content),
        content.len() as u64,
    )
    .expect("in-memory digest")
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

fn validate_projected(
    path: &str,
    node: &PayloadNode,
    content: Option<&PayloadContentAuthority>,
) -> Result<()> {
    node.validate_content(content)
        .map_err(|error| parse_error(format!("invalid RPM payload authority for {path}: {error}")))
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
    use super::{
        RpmFileDigestAlgorithm, apply_capability_operation, digest_hex, parse,
        parse_file_capabilities, require_partial_hardlink_count,
    };
    use rpm::IndexTag;
    use std::io::Cursor;

    #[test]
    fn partial_hardlink_set_may_omit_source_inode_members() {
        require_partial_hardlink_count("/usr/bin/tool", 3, 2).expect("partial source hardlink set");
        let error = require_partial_hardlink_count("/usr/bin/tool", 1, 2)
            .expect_err("declared count smaller than packaged members");
        assert!(error.to_string().contains("package contains 2 members"));
    }

    #[test]
    fn sha1_file_digest_uses_rpm_algorithm_code_two_semantics() {
        assert_eq!(
            digest_hex(RpmFileDigestAlgorithm::Sha1, b"abc"),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
    }

    #[test]
    fn sha1_file_digest_header_projects_payload_without_md5_fallback() {
        let content = b"SHA-1 RPM payload fixture\n";
        let mut builder =
            rpm::PackageBuilder::new("sha1-fixture", "1", "MIT", "noarch", "SHA-1 fixture");
        builder
            .with_file_contents(
                content,
                rpm::FileOptions::new("/usr/share/sha1-fixture/data"),
            )
            .unwrap();
        let package = builder.build().unwrap();
        let header_start = package.metadata.get_package_segment_offsets().header as usize;
        let mut bytes = Vec::new();
        package.write(&mut bytes).unwrap();

        let algorithm_offset =
            main_header_value_offset(&bytes, header_start, IndexTag::RPMTAG_FILEDIGESTALGO);
        bytes[algorithm_offset..algorithm_offset + 4].copy_from_slice(&2_u32.to_be_bytes());
        let digest_offset =
            main_header_value_offset(&bytes, header_start, IndexTag::RPMTAG_FILEDIGESTS);
        let sha1 = digest_hex(RpmFileDigestAlgorithm::Sha1, content);
        bytes[digest_offset..digest_offset + sha1.len()].copy_from_slice(sha1.as_bytes());
        bytes[digest_offset + sha1.len()] = 0;

        let patched = rpm::Package::parse(&mut Cursor::new(bytes)).unwrap();
        let payload = parse(&patched).unwrap();
        let files = payload.to_extracted_in_memory().unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "/usr/share/sha1-fixture/data");
        assert_eq!(files[0].content, content);
    }

    fn main_header_value_offset(bytes: &[u8], header_start: usize, wanted: IndexTag) -> usize {
        let entry_count = u32::from_be_bytes(
            bytes[header_start + 8..header_start + 12]
                .try_into()
                .unwrap(),
        ) as usize;
        let store_start = header_start + 16 + entry_count * 16;
        for index in 0..entry_count {
            let entry_start = header_start + 16 + index * 16;
            let tag = u32::from_be_bytes(bytes[entry_start..entry_start + 4].try_into().unwrap());
            if tag == wanted as u32 {
                let offset = i32::from_be_bytes(
                    bytes[entry_start + 8..entry_start + 12].try_into().unwrap(),
                );
                return store_start + usize::try_from(offset).unwrap();
            }
        }
        panic!("missing {wanted} in fixture header");
    }

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
