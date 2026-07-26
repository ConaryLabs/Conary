// apps/conary/src/commands/live_root/content.rs

//! Reopenable mutable-root content with exact SHA-256 and size authority.

use anyhow::{Context, Result, bail};
use conary_core::packages::payload::{PAYLOAD_IO_BUFFER_SIZE, PayloadSpool, ReopenablePayload};
use conary_core::payload::PayloadContentAuthority;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Write};
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
