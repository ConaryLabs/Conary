// conary-core/src/payload.rs

//! Exact POSIX payload-node authority shared by native parsers and CCS.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

const MODE_TYPE_MASK: u32 = libc::S_IFMT;
const MODE_PERMISSION_MASK: u32 = 0o7777;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PayloadTimestamp {
    pub seconds: i64,
    pub nanoseconds: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PayloadContentAuthority {
    pub sha256: String,
    pub size: u64,
}

/// Source-format authority for deciding whether two package claims may share
/// one selected-root materialization.
///
/// `Exclusive` is deliberately not an "exact bytes" fallback: a source format
/// must own and prove its overlap behavior before Conary can retain multiple
/// owners for a non-directory node. RPM's policy is pinned to
/// `rpmfilesCompare()` at revision
/// `a8f0192aee1c08bd1454ed2ac6ebaf506004b55c`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PayloadSharingPolicy {
    Exclusive,
    Rpm,
}

impl PayloadSharingPolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Exclusive => "exclusive",
            Self::Rpm => "rpm",
        }
    }

    pub fn parse(value: &str) -> std::result::Result<Self, PayloadCompatibilityMismatch> {
        match value {
            "exclusive" => Ok(Self::Exclusive),
            "rpm" => Ok(Self::Rpm),
            other => Err(PayloadCompatibilityMismatch::UnknownPolicy(
                other.to_string(),
            )),
        }
    }

    /// Compare two claims using only source-owned compatibility authority.
    ///
    /// Directory co-ownership has separate typed materialization semantics and
    /// is handled by the claim anchor policy. This comparator owns every
    /// non-directory RPM overlap.
    pub fn compare(
        self,
        other_policy: Self,
        left_node: &PayloadNode,
        left_content: Option<&PayloadContentAuthority>,
        right_node: &PayloadNode,
        right_content: Option<&PayloadContentAuthority>,
    ) -> std::result::Result<(), PayloadCompatibilityMismatch> {
        if self != other_policy {
            return Err(PayloadCompatibilityMismatch::Policy {
                left: self,
                right: other_policy,
            });
        }
        match self {
            Self::Exclusive => Err(PayloadCompatibilityMismatch::Exclusive),
            Self::Rpm => compare_rpm_payload(left_node, left_content, right_node, right_content),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PayloadCompatibilityMismatch {
    UnknownPolicy(String),
    Policy {
        left: PayloadSharingPolicy,
        right: PayloadSharingPolicy,
    },
    Exclusive,
    NodeKind {
        left: &'static str,
        right: &'static str,
    },
    Mode {
        left: u32,
        right: u32,
    },
    User,
    Group,
    ContentSize {
        left: u64,
        right: u64,
    },
    ContentDigest,
    SymlinkTarget,
    DeviceIdentity,
    HardlinkTopology,
    MissingContent,
}

impl fmt::Display for PayloadCompatibilityMismatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownPolicy(policy) => {
                write!(formatter, "unknown payload sharing policy {policy:?}")
            }
            Self::Policy { left, right } => write!(
                formatter,
                "sharing policies differ ('{}' versus '{}')",
                left.as_str(),
                right.as_str()
            ),
            Self::Exclusive => formatter.write_str("source policy is exclusive"),
            Self::NodeKind { left, right } => {
                write!(formatter, "node kinds differ ({left} versus {right})")
            }
            Self::Mode { left, right } => {
                write!(formatter, "modes differ ({left:#o} versus {right:#o})")
            }
            Self::User => formatter.write_str("source users differ"),
            Self::Group => formatter.write_str("source groups differ"),
            Self::ContentSize { left, right } => {
                write!(formatter, "content sizes differ ({left} versus {right})")
            }
            Self::ContentDigest => formatter.write_str("content digests differ"),
            Self::SymlinkTarget => formatter.write_str("symlink targets differ"),
            Self::DeviceIdentity => formatter.write_str("device identities differ"),
            Self::HardlinkTopology => formatter.write_str("hardlink topology differs"),
            Self::MissingContent => {
                formatter.write_str("regular node lacks comparable content authority")
            }
        }
    }
}

impl std::error::Error for PayloadCompatibilityMismatch {}

/// Source-native ownership authority.
///
/// Package conversion preserves whether the source format identifies an owner
/// numerically or by name. Named identities are resolved only against the
/// selected target root at the apply boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum PayloadIdentity {
    Numeric { id: u64 },
    Named { name: String },
}

impl PayloadIdentity {
    pub fn validate(&self) -> Result<()> {
        if let Self::Named { name } = self
            && (name.is_empty() || name.contains('\0'))
        {
            bail!("named payload identity must be non-empty and NUL-free");
        }
        Ok(())
    }
}

impl PayloadContentAuthority {
    pub fn validate(&self) -> Result<()> {
        if self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            bail!("payload content authority requires a lowercase raw 64-hex SHA-256 digest");
        }
        Ok(())
    }
}

impl PayloadTimestamp {
    pub const UNIX_EPOCH: Self = Self {
        seconds: 0,
        nanoseconds: 0,
    };
}

/// Complete authority for one POSIX payload node, excluding path and content
/// digest/size.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PayloadNode {
    pub kind: PayloadNodeKind,
    pub mode: u32,
    pub user: PayloadIdentity,
    pub group: PayloadIdentity,
    pub mtime: PayloadTimestamp,
    pub xattrs: BTreeMap<String, Vec<u8>>,
}

impl PayloadNode {
    /// Construct a regular node from its POSIX permission and special bits.
    pub fn regular(permissions: u32) -> Self {
        Self {
            kind: PayloadNodeKind::Regular {
                hardlink_identity: None,
            },
            mode: libc::S_IFREG | (permissions & MODE_PERMISSION_MASK),
            user: PayloadIdentity::Numeric { id: 0 },
            group: PayloadIdentity::Numeric { id: 0 },
            mtime: PayloadTimestamp::UNIX_EPOCH,
            xattrs: BTreeMap::new(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        self.kind.validate()?;
        self.user.validate()?;
        self.group.validate()?;
        let expected_type = self.kind.mode_type();
        if self.mode & MODE_TYPE_MASK != expected_type {
            bail!(
                "payload mode type {:#o} does not match {} node kind",
                self.mode & MODE_TYPE_MASK,
                self.kind.name()
            );
        }
        if self.mode & !(MODE_TYPE_MASK | MODE_PERMISSION_MASK) != 0 {
            bail!("payload mode contains bits outside POSIX type and permission authority");
        }
        if self.mtime.nanoseconds >= 1_000_000_000 {
            bail!("payload mtime nanoseconds must be below one billion");
        }
        if self
            .xattrs
            .iter()
            .any(|(name, _)| name.is_empty() || name.contains('\0'))
        {
            bail!("payload xattr names must be non-empty and NUL-free");
        }
        Ok(())
    }

    pub fn validate_content(&self, content: Option<&PayloadContentAuthority>) -> Result<()> {
        match (&self.kind, content) {
            (PayloadNodeKind::Regular { .. }, Some(content)) => content.validate(),
            (PayloadNodeKind::Regular { .. }, None) => {
                bail!("regular payload node requires content authority")
            }
            (_, Some(_)) => bail!("non-regular payload node must not carry content authority"),
            (_, None) => Ok(()),
        }
    }
}

/// A source payload node after ownership has been resolved against the
/// selected target root.
///
/// Installed state and generation materialization use this type so a named
/// source identity can never be mistaken for an already-resolved numeric ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedPayloadNode {
    pub source: PayloadNode,
    pub uid: u64,
    pub gid: u64,
}

impl ResolvedPayloadNode {
    pub fn validate(&self) -> Result<()> {
        self.source.validate()?;
        if let PayloadIdentity::Numeric { id } = &self.source.user
            && *id != self.uid
        {
            bail!("resolved payload uid differs from numeric source authority");
        }
        if let PayloadIdentity::Numeric { id } = &self.source.group
            && *id != self.gid
        {
            bail!("resolved payload gid differs from numeric source authority");
        }
        Ok(())
    }

    pub fn from_numeric_source(source: PayloadNode) -> Result<Self> {
        let PayloadIdentity::Numeric { id: uid } = &source.user else {
            bail!("payload user identity is unresolved");
        };
        let PayloadIdentity::Numeric { id: gid } = &source.group else {
            bail!("payload group identity is unresolved");
        };
        let uid = *uid;
        let gid = *gid;
        let resolved = Self { source, uid, gid };
        resolved.validate()?;
        Ok(resolved)
    }
}

/// A source package payload node as declared by its native archive grammar.
///
/// File kind is never inferred from mode bits, path spelling, payload length,
/// or the presence of another field. Variant data is the sole authority for
/// link targets and device identities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum PayloadNodeKind {
    Regular {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hardlink_identity: Option<String>,
    },
    Directory,
    Symlink {
        target: String,
    },
    Hardlink {
        target: String,
        identity: String,
    },
    BlockDevice {
        major: u64,
        minor: u64,
    },
    CharacterDevice {
        major: u64,
        minor: u64,
    },
    Fifo,
    Socket,
}

impl PayloadNodeKind {
    fn mode_type(&self) -> u32 {
        match self {
            Self::Regular { .. } | Self::Hardlink { .. } => libc::S_IFREG,
            Self::Directory => libc::S_IFDIR,
            Self::Symlink { .. } => libc::S_IFLNK,
            Self::BlockDevice { .. } => libc::S_IFBLK,
            Self::CharacterDevice { .. } => libc::S_IFCHR,
            Self::Fifo => libc::S_IFIFO,
            Self::Socket => libc::S_IFSOCK,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Self::Regular { .. } => "regular",
            Self::Directory => "directory",
            Self::Symlink { .. } => "symlink",
            Self::Hardlink { .. } => "hardlink",
            Self::BlockDevice { .. } => "block-device",
            Self::CharacterDevice { .. } => "character-device",
            Self::Fifo => "fifo",
            Self::Socket => "socket",
        }
    }

    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Symlink { target } => {
                if target.is_empty() || target.contains('\0') {
                    bail!("payload link target must be non-empty and NUL-free");
                }
            }
            Self::Hardlink { target, identity } => {
                if target.is_empty()
                    || target.contains('\0')
                    || identity.is_empty()
                    || identity.contains('\0')
                {
                    bail!("payload hardlink target and identity must be non-empty and NUL-free");
                }
            }
            Self::Regular { hardlink_identity } => {
                if hardlink_identity
                    .as_deref()
                    .is_some_and(|identity| identity.is_empty() || identity.contains('\0'))
                {
                    bail!("payload hardlink identity must be non-empty and NUL-free");
                }
            }
            Self::Directory
            | Self::BlockDevice { .. }
            | Self::CharacterDevice { .. }
            | Self::Fifo
            | Self::Socket => {}
        }
        Ok(())
    }

    pub fn link_target(&self) -> Option<&str> {
        match self {
            Self::Symlink { target } | Self::Hardlink { target, .. } => Some(target),
            _ => None,
        }
    }

    pub fn is_regular(&self) -> bool {
        matches!(self, Self::Regular { .. })
    }

    pub fn is_directory(&self) -> bool {
        matches!(self, Self::Directory)
    }

    pub fn is_symlink(&self) -> bool {
        matches!(self, Self::Symlink { .. })
    }
}

fn compare_rpm_payload(
    left: &PayloadNode,
    left_content: Option<&PayloadContentAuthority>,
    right: &PayloadNode,
    right_content: Option<&PayloadContentAuthority>,
) -> std::result::Result<(), PayloadCompatibilityMismatch> {
    left.validate_content(left_content)
        .map_err(|_| PayloadCompatibilityMismatch::MissingContent)?;
    right
        .validate_content(right_content)
        .map_err(|_| PayloadCompatibilityMismatch::MissingContent)?;

    let left_class = RpmNodeClass::of(&left.kind);
    let right_class = RpmNodeClass::of(&right.kind);
    if left_class != right_class {
        return Err(PayloadCompatibilityMismatch::NodeKind {
            left: left.kind.name(),
            right: right.kind.name(),
        });
    }

    // rpmfilesCompare() deliberately ignores symlink modes.
    if left_class != RpmNodeClass::Symlink && left.mode != right.mode {
        return Err(PayloadCompatibilityMismatch::Mode {
            left: left.mode,
            right: right.mode,
        });
    }
    if left.user != right.user {
        return Err(PayloadCompatibilityMismatch::User);
    }
    if left.group != right.group {
        return Err(PayloadCompatibilityMismatch::Group);
    }

    match (&left.kind, &right.kind) {
        (PayloadNodeKind::Regular { .. }, PayloadNodeKind::Regular { .. }) => {
            compare_regular_content(left_content, right_content)
        }
        (
            PayloadNodeKind::Symlink {
                target: left_target,
            },
            PayloadNodeKind::Symlink {
                target: right_target,
            },
        ) => {
            if left_target == right_target {
                Ok(())
            } else {
                Err(PayloadCompatibilityMismatch::SymlinkTarget)
            }
        }
        (
            PayloadNodeKind::BlockDevice {
                major: left_major,
                minor: left_minor,
            }
            | PayloadNodeKind::CharacterDevice {
                major: left_major,
                minor: left_minor,
            },
            PayloadNodeKind::BlockDevice {
                major: right_major,
                minor: right_minor,
            }
            | PayloadNodeKind::CharacterDevice {
                major: right_major,
                minor: right_minor,
            },
        ) => {
            if (left_major, left_minor) == (right_major, right_minor) {
                Ok(())
            } else {
                Err(PayloadCompatibilityMismatch::DeviceIdentity)
            }
        }
        (
            PayloadNodeKind::Hardlink {
                target: left_target,
                ..
            },
            PayloadNodeKind::Hardlink {
                target: right_target,
                ..
            },
        ) => {
            // Conary preserves archive hardlinks as typed graph edges instead
            // of duplicating the RPM digest on every member. The target path
            // is therefore the comparable topology authority; the target
            // claims independently prove the shared regular content.
            if left_target == right_target {
                Ok(())
            } else {
                Err(PayloadCompatibilityMismatch::HardlinkTopology)
            }
        }
        (PayloadNodeKind::Directory, PayloadNodeKind::Directory)
        | (PayloadNodeKind::Fifo, PayloadNodeKind::Fifo)
        | (PayloadNodeKind::Socket, PayloadNodeKind::Socket) => Ok(()),
        // RPM models hardlinks as regular file records with duplicated digest
        // authority. Conary's explicit hardlink edge does not carry such a
        // digest, so regular/hardlink sharing cannot be proven locally.
        (PayloadNodeKind::Regular { .. }, PayloadNodeKind::Hardlink { .. })
        | (PayloadNodeKind::Hardlink { .. }, PayloadNodeKind::Regular { .. }) => {
            Err(PayloadCompatibilityMismatch::MissingContent)
        }
        _ => Err(PayloadCompatibilityMismatch::NodeKind {
            left: left.kind.name(),
            right: right.kind.name(),
        }),
    }
}

fn compare_regular_content(
    left: Option<&PayloadContentAuthority>,
    right: Option<&PayloadContentAuthority>,
) -> std::result::Result<(), PayloadCompatibilityMismatch> {
    let (Some(left), Some(right)) = (left, right) else {
        return Err(PayloadCompatibilityMismatch::MissingContent);
    };
    if left.size != right.size {
        return Err(PayloadCompatibilityMismatch::ContentSize {
            left: left.size,
            right: right.size,
        });
    }
    if left.sha256 != right.sha256 {
        return Err(PayloadCompatibilityMismatch::ContentDigest);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RpmNodeClass {
    Regular,
    Directory,
    Symlink,
    BlockDevice,
    CharacterDevice,
    Fifo,
    Socket,
}

impl RpmNodeClass {
    const fn of(kind: &PayloadNodeKind) -> Self {
        match kind {
            PayloadNodeKind::Regular { .. } | PayloadNodeKind::Hardlink { .. } => Self::Regular,
            PayloadNodeKind::Directory => Self::Directory,
            PayloadNodeKind::Symlink { .. } => Self::Symlink,
            PayloadNodeKind::BlockDevice { .. } => Self::BlockDevice,
            PayloadNodeKind::CharacterDevice { .. } => Self::CharacterDevice,
            PayloadNodeKind::Fifo => Self::Fifo,
            PayloadNodeKind::Socket => Self::Socket,
        }
    }
}

#[cfg(test)]
mod sharing_tests {
    use super::*;

    fn content(bytes: &[u8]) -> PayloadContentAuthority {
        PayloadContentAuthority {
            sha256: crate::hash::sha256(bytes),
            size: bytes.len() as u64,
        }
    }

    #[test]
    fn rpm_regular_compatibility_uses_mode_source_identity_size_and_digest() {
        let mut left = PayloadNode::regular(0o644);
        left.kind = PayloadNodeKind::Regular {
            hardlink_identity: Some("rpm:1:1".to_string()),
        };
        let mut right = left.clone();
        right.kind = PayloadNodeKind::Regular {
            hardlink_identity: Some("rpm:9:9".to_string()),
        };
        right.mtime.seconds = 99;
        right
            .xattrs
            .insert("security.example".to_string(), b"ignored".to_vec());
        assert_eq!(
            PayloadSharingPolicy::Rpm.compare(
                PayloadSharingPolicy::Rpm,
                &left,
                Some(&content(b"same")),
                &right,
                Some(&content(b"same")),
            ),
            Ok(())
        );

        right.mode = libc::S_IFREG | 0o600;
        assert!(matches!(
            PayloadSharingPolicy::Rpm
                .compare(
                    PayloadSharingPolicy::Rpm,
                    &left,
                    Some(&content(b"same")),
                    &right,
                    Some(&content(b"same")),
                )
                .unwrap_err(),
            PayloadCompatibilityMismatch::Mode { .. }
        ));
        right.mode = left.mode;
        right.user = PayloadIdentity::Numeric { id: 1 };
        assert_eq!(
            PayloadSharingPolicy::Rpm
                .compare(
                    PayloadSharingPolicy::Rpm,
                    &left,
                    Some(&content(b"same")),
                    &right,
                    Some(&content(b"same")),
                )
                .unwrap_err(),
            PayloadCompatibilityMismatch::User
        );
        right.user = left.user.clone();
        right.group = PayloadIdentity::Numeric { id: 1 };
        assert_eq!(
            PayloadSharingPolicy::Rpm
                .compare(
                    PayloadSharingPolicy::Rpm,
                    &left,
                    Some(&content(b"same")),
                    &right,
                    Some(&content(b"same")),
                )
                .unwrap_err(),
            PayloadCompatibilityMismatch::Group
        );
        right.group = left.group.clone();
        let mut wrong_size = content(b"same");
        wrong_size.size += 1;
        assert!(matches!(
            PayloadSharingPolicy::Rpm
                .compare(
                    PayloadSharingPolicy::Rpm,
                    &left,
                    Some(&content(b"same")),
                    &right,
                    Some(&wrong_size),
                )
                .unwrap_err(),
            PayloadCompatibilityMismatch::ContentSize { .. }
        ));
        assert_eq!(
            PayloadSharingPolicy::Rpm
                .compare(
                    PayloadSharingPolicy::Rpm,
                    &left,
                    Some(&content(b"same")),
                    &right,
                    Some(&content(b"diff")),
                )
                .unwrap_err(),
            PayloadCompatibilityMismatch::ContentDigest
        );
        assert_eq!(
            PayloadSharingPolicy::Rpm
                .compare(
                    PayloadSharingPolicy::Rpm,
                    &left,
                    None,
                    &right,
                    Some(&content(b"same")),
                )
                .unwrap_err(),
            PayloadCompatibilityMismatch::MissingContent
        );
    }

    #[test]
    fn rpm_symlink_compatibility_ignores_permissions_but_not_target_or_owner() {
        let mut left = PayloadNode::regular(0o777);
        left.kind = PayloadNodeKind::Symlink {
            target: "target".to_string(),
        };
        left.mode = libc::S_IFLNK | 0o777;
        let mut right = left.clone();
        right.mode = libc::S_IFLNK;
        assert_eq!(
            PayloadSharingPolicy::Rpm.compare(PayloadSharingPolicy::Rpm, &left, None, &right, None),
            Ok(())
        );
        right.kind = PayloadNodeKind::Symlink {
            target: "other".to_string(),
        };
        assert_eq!(
            PayloadSharingPolicy::Rpm
                .compare(PayloadSharingPolicy::Rpm, &left, None, &right, None)
                .unwrap_err(),
            PayloadCompatibilityMismatch::SymlinkTarget
        );
        right.kind = left.kind.clone();
        right.user = PayloadIdentity::Numeric { id: 1 };
        assert_eq!(
            PayloadSharingPolicy::Rpm
                .compare(PayloadSharingPolicy::Rpm, &left, None, &right, None)
                .unwrap_err(),
            PayloadCompatibilityMismatch::User
        );
    }

    #[test]
    fn rpm_device_compatibility_uses_typed_kind_mode_and_device_identity() {
        let mut left = PayloadNode::regular(0o600);
        left.kind = PayloadNodeKind::BlockDevice { major: 8, minor: 1 };
        left.mode = libc::S_IFBLK | 0o600;
        let mut right = left.clone();
        assert_eq!(
            PayloadSharingPolicy::Rpm.compare(PayloadSharingPolicy::Rpm, &left, None, &right, None),
            Ok(())
        );

        right.kind = PayloadNodeKind::BlockDevice { major: 8, minor: 2 };
        assert_eq!(
            PayloadSharingPolicy::Rpm
                .compare(PayloadSharingPolicy::Rpm, &left, None, &right, None)
                .unwrap_err(),
            PayloadCompatibilityMismatch::DeviceIdentity
        );
        right.kind = PayloadNodeKind::CharacterDevice { major: 8, minor: 1 };
        right.mode = libc::S_IFCHR | 0o600;
        assert!(matches!(
            PayloadSharingPolicy::Rpm
                .compare(PayloadSharingPolicy::Rpm, &left, None, &right, None)
                .unwrap_err(),
            PayloadCompatibilityMismatch::NodeKind { .. }
        ));
        right.kind = left.kind.clone();
        right.mode = libc::S_IFBLK | 0o640;
        assert!(matches!(
            PayloadSharingPolicy::Rpm
                .compare(PayloadSharingPolicy::Rpm, &left, None, &right, None)
                .unwrap_err(),
            PayloadCompatibilityMismatch::Mode { .. }
        ));
    }

    #[test]
    fn rpm_hardlink_compatibility_uses_target_topology_not_archive_identity() {
        let mut left = PayloadNode::regular(0o644);
        left.kind = PayloadNodeKind::Hardlink {
            target: "/usr/share/target".to_string(),
            identity: "rpm:1:1".to_string(),
        };
        let mut right = left.clone();
        right.kind = PayloadNodeKind::Hardlink {
            target: "/usr/share/target".to_string(),
            identity: "rpm:9:9".to_string(),
        };
        assert_eq!(
            PayloadSharingPolicy::Rpm.compare(PayloadSharingPolicy::Rpm, &left, None, &right, None),
            Ok(())
        );

        right.kind = PayloadNodeKind::Hardlink {
            target: "/usr/share/other".to_string(),
            identity: "rpm:9:9".to_string(),
        };
        assert_eq!(
            PayloadSharingPolicy::Rpm
                .compare(PayloadSharingPolicy::Rpm, &left, None, &right, None)
                .unwrap_err(),
            PayloadCompatibilityMismatch::HardlinkTopology
        );

        let regular = PayloadNode::regular(0o644);
        assert_eq!(
            PayloadSharingPolicy::Rpm
                .compare(
                    PayloadSharingPolicy::Rpm,
                    &regular,
                    Some(&content(b"same")),
                    &left,
                    None,
                )
                .unwrap_err(),
            PayloadCompatibilityMismatch::MissingContent
        );
    }

    #[test]
    fn rpm_non_content_node_classes_remain_typed() {
        let mut fifo = PayloadNode::regular(0o600);
        fifo.kind = PayloadNodeKind::Fifo;
        fifo.mode = libc::S_IFIFO | 0o600;
        assert_eq!(
            PayloadSharingPolicy::Rpm.compare(PayloadSharingPolicy::Rpm, &fifo, None, &fifo, None),
            Ok(())
        );
        let mut socket = fifo.clone();
        socket.kind = PayloadNodeKind::Socket;
        socket.mode = libc::S_IFSOCK | 0o600;
        assert!(matches!(
            PayloadSharingPolicy::Rpm
                .compare(PayloadSharingPolicy::Rpm, &fifo, None, &socket, None)
                .unwrap_err(),
            PayloadCompatibilityMismatch::NodeKind { .. }
        ));
    }

    #[test]
    fn exclusive_and_mixed_policies_never_create_shared_authority() {
        let node = PayloadNode::regular(0o644);
        let authority = content(b"same");
        assert_eq!(
            PayloadSharingPolicy::Exclusive
                .compare(
                    PayloadSharingPolicy::Exclusive,
                    &node,
                    Some(&authority),
                    &node,
                    Some(&authority),
                )
                .unwrap_err(),
            PayloadCompatibilityMismatch::Exclusive
        );
        assert!(matches!(
            PayloadSharingPolicy::Rpm
                .compare(
                    PayloadSharingPolicy::Exclusive,
                    &node,
                    Some(&authority),
                    &node,
                    Some(&authority),
                )
                .unwrap_err(),
            PayloadCompatibilityMismatch::Policy { .. }
        ));
    }
}
