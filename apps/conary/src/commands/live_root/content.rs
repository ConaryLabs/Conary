// apps/conary/src/commands/live_root/content.rs

//! Reopenable mutable-root content with exact SHA-256 and size authority.

use super::LiveRootFile;
use super::durability::MutationDurability;
use super::path::selected_root_target_path;
use anyhow::{Context, Result, bail};
use conary_core::packages::payload::{PAYLOAD_IO_BUFFER_SIZE, PayloadSpool, ReopenablePayload};
use conary_core::payload::{PayloadContentAuthority, PayloadNodeKind};
use sha2::{Digest, Sha256};
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixListener;
use std::path::Path;
#[cfg(test)]
use std::sync::Arc;

#[derive(Debug, Clone)]
pub(crate) enum LiveRootContent {
    Absent,
    Regular {
        authority: PayloadContentAuthority,
        source: ReopenablePayload,
    },
}

impl LiveRootContent {
    #[must_use]
    pub(crate) const fn absent() -> Self {
        Self::Absent
    }

    pub(crate) fn regular(
        authority: PayloadContentAuthority,
        source: ReopenablePayload,
    ) -> Result<Self> {
        if authority.sha256.len() != 64
            || !authority
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            bail!(
                "live-root content digest {:?} is not canonical lowercase SHA-256",
                authority.sha256
            );
        }
        Ok(Self::Regular { authority, source })
    }

    pub(crate) fn from_cas(
        cas: &conary_core::filesystem::CasStore,
        authority: PayloadContentAuthority,
        cas_hash: &str,
    ) -> Result<Self> {
        if authority.sha256 != cas_hash {
            bail!(
                "CAS identity {cas_hash} disagrees with payload authority {}",
                authority.sha256
            );
        }
        Self::regular(
            authority,
            ReopenablePayload::from_path(cas.hash_to_path(cas_hash)?),
        )
    }

    /// Capture a mutable file into a private spool while hashing it.
    pub(crate) fn capture_file(path: &Path) -> Result<Self> {
        let declared_size = std::fs::metadata(path)
            .with_context(|| format!("inspect mutable file {}", path.display()))?
            .len();
        let spool = PayloadSpool::new(declared_size)?;
        let output_path = spool.indexed_path(0);
        let mut input =
            File::open(path).with_context(|| format!("open mutable file {}", path.display()))?;
        let mut output = File::create(&output_path)?;
        let (size, sha256) = copy_and_hash(&mut input, &mut output, Some(declared_size))?;
        if size != declared_size {
            bail!(
                "mutable file {} changed size while being captured: expected {declared_size}, got {size}",
                path.display()
            );
        }
        output.sync_all()?;
        Self::regular(
            PayloadContentAuthority { sha256, size },
            spool.source(output_path),
        )
    }

    #[cfg(test)]
    pub(crate) fn from_in_memory_bytes(bytes: &[u8]) -> Self {
        Self::Regular {
            authority: PayloadContentAuthority {
                sha256: conary_core::hash::sha256(bytes),
                size: bytes.len() as u64,
            },
            source: ReopenablePayload::from_in_memory_bytes(Arc::<[u8]>::from(bytes)),
        }
    }

    #[must_use]
    pub(crate) fn is_absent(&self) -> bool {
        matches!(self, Self::Absent)
    }

    #[must_use]
    pub(crate) fn authority(&self) -> Option<&PayloadContentAuthority> {
        match self {
            Self::Absent => None,
            Self::Regular { authority, .. } => Some(authority),
        }
    }

    pub(crate) fn open(&self) -> Result<Box<dyn Read + Send>> {
        match self {
            Self::Absent => bail!("non-regular live-root node has no content stream"),
            Self::Regular { source, .. } => source.open().map_err(anyhow::Error::from),
        }
    }

    pub(crate) fn copy_verified_to(&self, output: &mut dyn Write) -> Result<()> {
        let authority = self
            .authority()
            .context("non-regular live-root node has no content authority")?;
        let mut input = self.open()?;
        let (size, sha256) = copy_and_hash(&mut input, output, Some(authority.size))?;
        if size != authority.size {
            bail!(
                "live-root content size mismatch: expected {}, got {size}",
                authority.size
            );
        }
        if sha256 != authority.sha256 {
            bail!(
                "live-root content digest mismatch: expected {}, got {sha256}",
                authority.sha256
            );
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn to_in_memory(&self) -> Result<Vec<u8>> {
        let mut content = Vec::new();
        self.copy_verified_to(&mut content)?;
        Ok(content)
    }
}

pub(super) fn create_live_root_leaf(
    root: &Path,
    path: &Path,
    file: &LiveRootFile,
    durability: &mut MutationDurability,
) -> Result<()> {
    match &file.node.source.kind {
        PayloadNodeKind::Regular { .. } => {
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .with_context(|| format!("Failed to create {}", path.display()))?;
            file.content
                .copy_verified_to(&mut output)
                .with_context(|| format!("Failed to write {}", path.display()))?;
            durability
                .sync_file(&output)
                .with_context(|| format!("Failed to sync {}", path.display()))?;
        }
        PayloadNodeKind::Symlink { target } => {
            std::os::unix::fs::symlink(target, path)
                .with_context(|| format!("Failed to create symlink {}", path.display()))?;
        }
        PayloadNodeKind::Hardlink { target, .. } => {
            let target = selected_root_target_path(root, target)?;
            let metadata = fs::symlink_metadata(&target).with_context(|| {
                format!(
                    "payload hardlink {} target is unavailable: {}",
                    file.path,
                    target.display()
                )
            })?;
            if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
                bail!(
                    "payload hardlink {} target is not a regular file: {}",
                    file.path,
                    target.display()
                );
            }
            fs::hard_link(&target, path).with_context(|| {
                format!(
                    "Failed to create hardlink {} to {}",
                    path.display(),
                    target.display()
                )
            })?;
        }
        PayloadNodeKind::BlockDevice { major, minor } => {
            create_live_root_device(path, libc::S_IFBLK, *major, *minor)?;
        }
        PayloadNodeKind::CharacterDevice { major, minor } => {
            create_live_root_device(path, libc::S_IFCHR, *major, *minor)?;
        }
        PayloadNodeKind::Fifo => {
            let c_path = c_path(path)?;
            if unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) } != 0 {
                return Err(io::Error::last_os_error())
                    .with_context(|| format!("Failed to create FIFO {}", path.display()));
            }
        }
        PayloadNodeKind::Socket => {
            let socket = UnixListener::bind(path)
                .with_context(|| format!("Failed to create socket {}", path.display()))?;
            drop(socket);
        }
        PayloadNodeKind::Directory => unreachable!("directories use apply_directory"),
    }
    Ok(())
}

fn create_live_root_device(path: &Path, kind: libc::mode_t, major: u64, minor: u64) -> Result<()> {
    let major = libc::c_uint::try_from(major)
        .with_context(|| format!("device major is not representable at {}", path.display()))?;
    let minor = libc::c_uint::try_from(minor)
        .with_context(|| format!("device minor is not representable at {}", path.display()))?;
    let c_path = c_path(path)?;
    if unsafe { libc::mknod(c_path.as_ptr(), kind | 0o600, libc::makedev(major, minor)) } != 0 {
        return Err(io::Error::last_os_error())
            .with_context(|| format!("Failed to create device {}", path.display()));
    }
    Ok(())
}

fn c_path(path: &Path) -> Result<CString> {
    CString::new(path.as_os_str().as_bytes())
        .with_context(|| format!("filesystem path contains NUL: {}", path.display()))
}

fn copy_and_hash(
    input: &mut dyn Read,
    output: &mut dyn Write,
    expected_size: Option<u64>,
) -> Result<(u64, String)> {
    let mut digest = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = [0_u8; PAYLOAD_IO_BUFFER_SIZE];
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        size = size
            .checked_add(read as u64)
            .context("live-root content size arithmetic overflow")?;
        if expected_size.is_some_and(|expected| size > expected) {
            bail!("live-root content exceeds declared size {expected_size:?}");
        }
        digest.update(&buffer[..read]);
        output.write_all(&buffer[..read])?;
    }
    Ok((size, hex::encode(digest.finalize())))
}
