// conary-core/src/packages/rpm/payload/hardlinks.rs

//! RPM hardlink-set transaction projection.
//!
//! At the pinned upstream RPM revision, `rpmfilesBuildNLink` groups non-ghost
//! regular files solely by positive inode plus device and retains header-index
//! order. `rpmfiArchiveHasContent` designates the last packaged header member
//! as the content authority, including for an all-zero-length set. `fsmMkfile`
//! applies that completing member's metadata once to the shared inode.

use super::header::HeaderRecord;
use super::stream::PayloadMember;
use super::{mode_type, parse_error, require_regular_content, source_node, validate_projected};
use crate::error::Result;
use crate::packages::payload::PackagePayloadFile;
use crate::payload::{PayloadContentAuthority, PayloadNodeKind};
use std::collections::{BTreeMap, HashMap};

/// One RPM device/inode set with its transaction-selected content and metadata
/// owner.
#[derive(Debug)]
pub(super) struct HardlinkSet {
    member_indexes: Vec<usize>,
    content_authority_index: usize,
}

impl HardlinkSet {
    fn new(member_indexes: Vec<usize>, records: &[HeaderRecord]) -> Result<Self> {
        if member_indexes.len() < 2 {
            return Err(parse_error(
                "RPM hardlink set projection requires at least two packaged members",
            ));
        }
        let first_index = member_indexes[0];
        let first = records.get(first_index).ok_or_else(|| {
            parse_error(format!(
                "RPM hardlink set references missing header index {first_index}"
            ))
        })?;
        if first.ghost || mode_type(first.mode) != libc::S_IFREG || first.inode == 0 {
            return Err(parse_error(format!(
                "RPM hardlink set starts with impossible identity at {}",
                first.path
            )));
        }
        for index in &member_indexes {
            let record = records.get(*index).ok_or_else(|| {
                parse_error(format!(
                    "RPM hardlink set references missing header index {index}"
                ))
            })?;
            if record.ghost
                || mode_type(record.mode) != libc::S_IFREG
                || record.inode == 0
                || record.device != first.device
                || record.inode != first.inode
            {
                return Err(parse_error(format!(
                    "RPM hardlink members {} and {} have impossible device/inode identity",
                    first.path, record.path
                )));
            }
            if let Some(declared) = record.nlink {
                require_partial_hardlink_count(&record.path, declared, member_indexes.len())?;
            }
        }
        let content_authority_index = *member_indexes
            .last()
            .expect("hardlink set has at least two members");
        Ok(Self {
            member_indexes,
            content_authority_index,
        })
    }

    pub(super) fn member_indexes(&self) -> &[usize] {
        &self.member_indexes
    }

    pub(super) fn emission_index(&self) -> usize {
        self.member_indexes[0]
    }

    pub(super) fn project(
        &self,
        records: &[HeaderRecord],
        payload_by_index: &mut HashMap<usize, PayloadMember>,
    ) -> Result<Vec<PackagePayloadFile>> {
        let owner = &records[self.content_authority_index];
        self.require_archive_completion(owner, payload_by_index)?;
        self.require_digest_agreement(records, owner)?;

        for index in &self.member_indexes {
            if *index == self.content_authority_index {
                continue;
            }
            let member = payload_by_index
                .remove(index)
                .expect("payload completeness checked");
            require_regular_member_authority(&records[*index], &member)?;
            if member.content_size != 0 {
                return Err(parse_error(format!(
                    "RPM hardlink member {} carries nonzero payload bytes but content authority is {}",
                    records[*index].path, owner.path
                )));
            }
        }

        let content_member = payload_by_index
            .remove(&self.content_authority_index)
            .expect("payload completeness checked");
        require_regular_member_authority(owner, &content_member)?;
        let sha256 = content_member
            .sha256
            .expect("regular member authority checked");
        let source = content_member
            .source
            .expect("regular member authority checked");
        require_regular_content(owner, content_member.content_size, &source)?;

        let identity = format!("rpm:{}:{}", owner.device, owner.inode);
        let mut owner_node = source_node(owner)?;
        owner_node.kind = PayloadNodeKind::Regular {
            hardlink_identity: Some(identity.clone()),
        };
        let authority = PayloadContentAuthority {
            sha256,
            size: content_member.content_size,
        };
        validate_projected(&owner.path, &owner_node, Some(&authority))?;

        let mut output = Vec::with_capacity(self.member_indexes.len());
        output.push(PackagePayloadFile::new(
            owner.path.clone(),
            owner_node.clone(),
            Some(authority),
            Some(source),
        )?);
        for index in &self.member_indexes {
            if *index == self.content_authority_index {
                continue;
            }
            let record = &records[*index];
            let mut node = owner_node.clone();
            node.kind = PayloadNodeKind::Hardlink {
                target: owner.path.clone(),
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
        Ok(output)
    }

    fn require_archive_completion(
        &self,
        owner: &HeaderRecord,
        payload_by_index: &HashMap<usize, PayloadMember>,
    ) -> Result<()> {
        let owner_position = payload_by_index
            .get(&self.content_authority_index)
            .expect("payload completeness checked")
            .archive_position;
        for index in &self.member_indexes {
            let position = payload_by_index
                .get(index)
                .expect("payload completeness checked")
                .archive_position;
            if *index != self.content_authority_index && position >= owner_position {
                return Err(parse_error(format!(
                    "RPM hardlink content authority {} does not complete its archived set",
                    owner.path
                )));
            }
        }
        Ok(())
    }

    fn require_digest_agreement(
        &self,
        records: &[HeaderRecord],
        owner: &HeaderRecord,
    ) -> Result<()> {
        let owner_digest = owner.digest.as_ref().ok_or_else(|| {
            parse_error(format!(
                "RPM hardlink content authority {} has no declared file digest",
                owner.path
            ))
        })?;
        for index in &self.member_indexes {
            let member = &records[*index];
            if member.digest.as_ref() != Some(owner_digest) {
                return Err(parse_error(format!(
                    "RPM hardlink members {} and {} declare different file digests",
                    owner.path, member.path
                )));
            }
        }
        Ok(())
    }
}

pub(super) fn sets(records: &[HeaderRecord]) -> Result<Vec<HardlinkSet>> {
    let mut groups: BTreeMap<(u32, u32), Vec<usize>> = BTreeMap::new();
    for (index, record) in records.iter().enumerate() {
        // Pinned RPM authority:
        // lib/rpmfi.cc rpmfilesBuildNLink at a8f0192a, lines 1395-1408.
        if !record.ghost && mode_type(record.mode) == libc::S_IFREG && record.inode != 0 {
            groups
                .entry((record.device, record.inode))
                .or_default()
                .push(index);
        }
    }
    groups
        .into_values()
        .filter(|members| members.len() > 1)
        .map(|members| HardlinkSet::new(members, records))
        .collect()
}

fn require_regular_member_authority(record: &HeaderRecord, member: &PayloadMember) -> Result<()> {
    if member.sha256.is_none() || member.source.is_none() {
        return Err(parse_error(format!(
            "RPM regular hardlink member {} has no payload authority",
            record.path
        )));
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packages::payload::ReopenablePayload;
    use crate::payload::{PayloadIdentity, PayloadTimestamp};
    use std::sync::Arc;

    use super::super::header::{DeclaredDigest, RpmFileDigestAlgorithm};

    const CONTENT: &[u8] = b"payload";

    fn record(path: &str, device: u32, inode: u32, size: u64) -> HeaderRecord {
        HeaderRecord {
            path: path.to_string(),
            mode: libc::S_IFREG | 0o644,
            user: "root".to_string(),
            group: "root".to_string(),
            mtime: 1,
            size,
            ghost: false,
            digest: Some(DeclaredDigest {
                algorithm: RpmFileDigestAlgorithm::Sha2_256,
                hex: crate::hash::sha256(CONTENT),
            }),
            link_target: None,
            caps: None,
            ima_signature: None,
            device,
            inode,
            rdev: 0,
            nlink: None,
        }
    }

    fn empty_record(path: &str, device: u32, inode: u32) -> HeaderRecord {
        let mut record = record(path, device, inode, 0);
        record.digest = Some(DeclaredDigest {
            algorithm: RpmFileDigestAlgorithm::Sha2_256,
            hex: crate::hash::sha256(b""),
        });
        record
    }

    fn member(header_index: usize, archive_position: usize, content: &[u8]) -> PayloadMember {
        PayloadMember {
            header_index,
            archive_position,
            content_size: content.len() as u64,
            sha256: Some(crate::hash::sha256(content)),
            source: Some(ReopenablePayload::from_in_memory_bytes(Arc::<[u8]>::from(
                content,
            ))),
        }
    }

    fn payload_map(members: Vec<PayloadMember>) -> HashMap<usize, PayloadMember> {
        members
            .into_iter()
            .map(|member| (member.header_index, member))
            .collect()
    }

    #[test]
    fn completing_member_supplies_effective_inode_metadata() {
        let mut records = vec![
            record("/usr/bin/first", 1, 7, CONTENT.len() as u64),
            record("/usr/bin/owner", 1, 7, CONTENT.len() as u64),
        ];
        records[0].mode = libc::S_IFREG | 0o755;
        records[0].user = "first-user".to_string();
        records[0].group = "first-group".to_string();
        records[0].mtime = 10;
        records[1].mode = libc::S_IFREG | 0o4750;
        records[1].user = "owner-user".to_string();
        records[1].group = "owner-group".to_string();
        records[1].mtime = 20;
        let set = sets(&records).unwrap().pop().unwrap();
        let mut payload = payload_map(vec![member(0, 0, b""), member(1, 1, CONTENT)]);

        let files = set.project(&records, &mut payload).unwrap();

        assert_eq!(files[0].path, "/usr/bin/owner");
        assert_eq!(
            files[0].node.kind,
            PayloadNodeKind::Regular {
                hardlink_identity: Some("rpm:1:7".to_string())
            }
        );
        assert_eq!(files[0].node.mode, libc::S_IFREG | 0o4750);
        assert_eq!(
            files[0].node.user,
            PayloadIdentity::Named {
                name: "owner-user".to_string()
            }
        );
        assert_eq!(
            files[0].node.group,
            PayloadIdentity::Named {
                name: "owner-group".to_string()
            }
        );
        assert_eq!(
            files[0].node.mtime,
            PayloadTimestamp {
                seconds: 20,
                nanoseconds: 0
            }
        );
        assert_eq!(
            files[1].node.kind,
            PayloadNodeKind::Hardlink {
                target: "/usr/bin/owner".to_string(),
                identity: "rpm:1:7".to_string()
            }
        );
        assert_eq!(files[1].node.mode, files[0].node.mode);
        assert_eq!(files[1].node.user, files[0].node.user);
        assert_eq!(files[1].node.group, files[0].node.group);
        assert_eq!(files[1].node.mtime, files[0].node.mtime);
    }

    #[test]
    fn last_member_completes_all_zero_length_set() {
        let records = vec![
            empty_record("/usr/share/empty-a", 1, 9),
            empty_record("/usr/share/empty-b", 1, 9),
        ];
        let set = sets(&records).unwrap().pop().unwrap();
        let mut payload = payload_map(vec![member(0, 0, b""), member(1, 1, b"")]);

        let files = set.project(&records, &mut payload).unwrap();

        assert_eq!(files[0].path, "/usr/share/empty-b");
        assert_eq!(files[0].content_authority.as_ref().unwrap().size, 0);
        assert_eq!(
            files[1].node.kind,
            PayloadNodeKind::Hardlink {
                target: "/usr/share/empty-b".to_string(),
                identity: "rpm:1:9".to_string()
            }
        );
    }

    #[test]
    fn partial_set_accepts_larger_source_count_and_rejects_smaller_count() {
        let mut records = vec![
            record("/usr/bin/a", 1, 11, CONTENT.len() as u64),
            record("/usr/bin/b", 1, 11, CONTENT.len() as u64),
        ];
        records[0].nlink = Some(3);
        records[1].nlink = Some(3);
        assert_eq!(sets(&records).unwrap().len(), 1);

        records[1].nlink = Some(1);
        let error = sets(&records).unwrap_err();
        assert!(error.to_string().contains("package contains 2 members"));
    }

    #[test]
    fn ghost_records_do_not_participate_in_device_inode_set() {
        let mut records = vec![
            record("/usr/bin/a", 1, 13, CONTENT.len() as u64),
            record("/usr/bin/ghost", 1, 13, CONTENT.len() as u64),
            record("/usr/bin/b", 1, 13, CONTENT.len() as u64),
        ];
        records[1].ghost = true;

        let set = sets(&records).unwrap().pop().unwrap();

        assert_eq!(set.member_indexes(), [0, 2]);
        assert_eq!(set.content_authority_index, 2);
    }

    #[test]
    fn completing_member_caps_and_ima_are_effective_inode_xattrs() {
        let mut records = vec![
            record("/usr/bin/a", 1, 15, CONTENT.len() as u64),
            record("/usr/bin/b", 1, 15, CONTENT.len() as u64),
        ];
        records[0].caps = Some("cap_net_raw=ep".to_string());
        records[0].ima_signature = Some("0101".to_string());
        records[1].caps = Some("cap_net_bind_service=ep".to_string());
        records[1].ima_signature = Some("0202".to_string());
        let set = sets(&records).unwrap().pop().unwrap();
        let mut payload = payload_map(vec![member(0, 0, b""), member(1, 1, CONTENT)]);

        let files = set.project(&records, &mut payload).unwrap();

        assert_eq!(files[0].node.xattrs.get("security.ima"), Some(&vec![2, 2]));
        assert_eq!(
            files[1].node.xattrs.get("security.ima"),
            files[0].node.xattrs.get("security.ima")
        );
        assert_ne!(
            files[0].node.xattrs.get("security.capability"),
            Some(&super::super::parse_file_capabilities("cap_net_raw=ep", "/usr/bin/a").unwrap())
        );
    }

    #[test]
    fn digest_disagreement_remains_fail_closed() {
        let mut records = vec![
            record("/usr/bin/a", 1, 17, CONTENT.len() as u64),
            record("/usr/bin/b", 1, 17, CONTENT.len() as u64),
        ];
        records[0].digest.as_mut().unwrap().hex = crate::hash::sha256(b"different");
        let set = sets(&records).unwrap().pop().unwrap();
        let mut payload = payload_map(vec![member(0, 0, b""), member(1, 1, CONTENT)]);

        let error = set.project(&records, &mut payload).unwrap_err();

        assert!(error.to_string().contains("different file digests"));
    }

    #[test]
    fn non_authority_payload_bytes_remain_fail_closed() {
        let records = vec![
            record("/usr/bin/a", 1, 19, CONTENT.len() as u64),
            record("/usr/bin/b", 1, 19, CONTENT.len() as u64),
        ];
        let set = sets(&records).unwrap().pop().unwrap();
        let mut payload = payload_map(vec![member(0, 0, b"conflict"), member(1, 1, CONTENT)]);

        let error = set.project(&records, &mut payload).unwrap_err();

        assert!(error.to_string().contains("carries nonzero payload bytes"));
    }

    #[test]
    fn header_set_without_designated_content_remains_fail_closed() {
        let records = vec![
            record("/usr/bin/a", 1, 21, CONTENT.len() as u64),
            record("/usr/bin/b", 1, 21, CONTENT.len() as u64),
        ];
        let set = sets(&records).unwrap().pop().unwrap();
        let mut payload = payload_map(vec![member(0, 0, b""), member(1, 1, b"")]);

        let error = set.project(&records, &mut payload).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("declares 7 bytes but payload yields 0")
        );
    }

    #[test]
    fn designated_content_member_must_complete_archive_set() {
        let records = vec![
            record("/usr/bin/a", 1, 23, CONTENT.len() as u64),
            record("/usr/bin/b", 1, 23, CONTENT.len() as u64),
        ];
        let set = sets(&records).unwrap().pop().unwrap();
        let mut payload = payload_map(vec![member(0, 1, b""), member(1, 0, CONTENT)]);

        let error = set.project(&records, &mut payload).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("does not complete its archived set")
        );
    }

    #[test]
    fn impossible_device_inode_identity_remains_fail_closed() {
        let records = vec![
            record("/usr/bin/a", 1, 25, CONTENT.len() as u64),
            record("/usr/bin/b", 2, 25, CONTENT.len() as u64),
        ];

        let error = HardlinkSet::new(vec![0, 1], &records).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("impossible device/inode identity")
        );
    }
}
