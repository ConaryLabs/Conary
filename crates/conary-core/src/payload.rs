// conary-core/src/payload.rs

//! Exact POSIX payload-node authority shared by native parsers and CCS.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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
