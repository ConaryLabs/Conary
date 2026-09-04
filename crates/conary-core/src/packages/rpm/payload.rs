// crates/conary-core/src/packages/rpm/payload.rs

//! Exact RPM header and payload projection.
//!
//! RPM's header is authoritative for installed metadata. The CPIO stream is
//! only authoritative for entry association and bytes; in particular, its
//! numeric uid/gid and inode fields must not override RPM's named ownership or
//! `FILEDEVICES`/`FILEINODES` hard-link identity.

use crate::error::{Error, Result};
use crate::packages::payload::{PackagePayload, PackagePayloadFile, PayloadSpool};
use crate::payload::{
    PayloadContentAuthority, PayloadIdentity, PayloadNode, PayloadNodeKind, PayloadTimestamp,
};
use rpm::{IndexTag, Package};
use std::collections::{BTreeMap, HashMap};
use std::io::Read;

mod digest;
mod hardlinks;
mod header;
mod stream;

#[cfg(test)]
use header::RpmFileDigestAlgorithm;
use header::{HeaderRecord, header_records};

pub(super) fn parse_stream(
    package: &Package,
    payload: Box<dyn Read>,
    decompressed_bytes: &crate::packages::parse_metrics::ReadCounter,
) -> Result<(PackagePayload, crate::packages::NativePackageParseMetrics)> {
    require_cpio_payload(package)?;
    let records = header_records(package)?;
    let spool = PayloadSpool::new(stream::required_spool_bytes(&records)?)?;
    let members = stream::parse_members(package, payload, &records, &spool, decompressed_bytes)?;
    let payload_files_spooled = members
        .iter()
        .filter(|member| member.regular.is_some())
        .count();
    let payload_bytes_spooled = members
        .iter()
        .filter(|member| member.regular.is_some())
        .try_fold(0_u64, |total, member| {
            total
                .checked_add(member.content_size)
                .ok_or_else(|| parse_error("RPM payload spool byte count overflow"))
        })?;
    let payload_bytes_hashed = members
        .iter()
        .filter_map(|member| member.regular.as_ref())
        .try_fold(0_u64, |total, computed| {
            total
                .checked_add(computed.computed.bytes_hashed)
                .ok_or_else(|| parse_error("RPM payload hash byte count overflow"))
        })?;
    let archive_entries_traversed = u64::try_from(members.len())
        .map_err(|_| parse_error("RPM payload member count exceeds u64"))?;
    let payload = project_records(&records, members)?;
    Ok((
        payload,
        crate::packages::NativePackageParseMetrics {
            archive_passes: 1,
            archive_entries_traversed,
            decompressed_archive_bytes_read: decompressed_bytes.bytes(),
            payload_files_spooled: u64::try_from(payload_files_spooled)
                .map_err(|_| parse_error("RPM payload spool file count exceeds u64"))?,
            payload_bytes_spooled,
            payload_spool_bytes_reread: 0,
            payload_spool_file_reopens: 0,
            payload_spool_file_syncs: 0,
            payload_bytes_hashed,
            ..Default::default()
        },
    ))
}

fn project_records(
    records: &[HeaderRecord],
    members: Vec<stream::PayloadMember>,
) -> Result<PackagePayload> {
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

    let hardlink_sets = hardlinks::sets(records)?;
    let mut group_by_index = HashMap::new();
    for set in &hardlink_sets {
        for index in set.member_indexes() {
            group_by_index.insert(*index, set);
        }
    }

    let mut output = Vec::with_capacity(records.len());
    for (index, record) in records.iter().enumerate() {
        if record.ghost {
            continue;
        }
        if record.is_root_anchor() {
            payload_by_index
                .remove(&index)
                .expect("payload completeness checked");
            continue;
        }
        if let Some(set) = group_by_index.get(&index) {
            if index != set.emission_index() {
                continue;
            }
            output.extend(set.project(records, &mut payload_by_index)?);
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
    let decompressed_bytes = crate::packages::parse_metrics::ReadCounter::default();
    parse_stream(
        package,
        Box::new(std::io::Cursor::new(package.payload.clone())),
        &decompressed_bytes,
    )
    .map(|(payload, _metrics)| payload)
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

fn project_single(
    record: &HeaderRecord,
    member: stream::PayloadMember,
) -> Result<PackagePayloadFile> {
    let node = source_node(record)?;
    let (content_authority, source) = match &node.kind {
        PayloadNodeKind::Regular { .. } => {
            let regular = member.regular.ok_or_else(|| {
                parse_error(format!(
                    "RPM regular node {} has no payload source and computed content evidence",
                    record.path
                ))
            })?;
            require_regular_content(record, member.content_size, &regular.computed)?;
            (
                Some(PayloadContentAuthority {
                    sha256: regular.computed.sha256,
                    size: member.content_size,
                }),
                Some(regular.source),
            )
        }
        PayloadNodeKind::Symlink { target }
            if member.content_size == target.len() as u64 && member.regular.is_none() =>
        {
            (None, None)
        }
        _ => {
            if member.content_size != 0 || member.regular.is_some() {
                return Err(parse_error(format!(
                    "non-regular RPM node {} retained ambiguous payload content",
                    record.path
                )));
            }
            (None, None)
        }
    };
    validate_projected(&record.path, &node, content_authority.as_ref())?;
    PackagePayloadFile::new(record.path.clone(), node, content_authority, source)
}

fn require_regular_content(
    record: &HeaderRecord,
    content_size: u64,
    computed: &digest::ComputedRegularContent,
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
    if computed.declared.algorithm != digest.algorithm {
        return Err(parse_error(format!(
            "RPM computed file digest algorithm for {} disagrees with its header authority",
            record.path
        )));
    }
    if computed.declared.hex != digest.hex {
        return Err(parse_error(format!(
            "RPM file digest mismatch for {}: expected {}, got {}",
            record.path, digest.hex, computed.declared.hex
        )));
    }
    Ok(())
}

#[cfg(test)]
fn digest_hex(algorithm: RpmFileDigestAlgorithm, content: &[u8]) -> String {
    let mut output = Vec::new();
    stream::copy_exact_payload(
        &mut std::io::Cursor::new(content),
        &mut output,
        content.len() as u64,
        algorithm,
        false,
    )
    .expect("in-memory digest")
    .0
    .declared
    .hex
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
mod tests;
