// conary-core/src/repository/trust/openpgp/arch.rs

//! Arch (ALPM) keyring authority.
//!
//! Ownership inside this subtree:
//!
//! * [`keyring`] -- the byte grammar of a pacman keyring source.
//! * [`snapshot`] -- the authenticated trust snapshot: pinned master
//!   authority, certified packager certificates, disabled-key state, and the
//!   single reference time everything is judged at.
//! * [`verify`] -- pacman-conformant signature evaluation against a snapshot.
//!
//! Repository-metadata trust roles (Debian `Release`, rpm-md, RPM packages)
//! use a pinned-certificate model and stay in [`super::pinned`].

pub mod keyring;
pub mod snapshot;
pub mod verify;

#[cfg(test)]
mod corpus;
#[cfg(test)]
pub(crate) mod testkeys;

use crate::error::Result;
use std::path::Path;
use std::time::SystemTime;

use super::super::ArchKeyringTrust;
use super::store;
use keyring::{ArchDisabledKeys, extract_keyring_source, parse_certificates, parse_revoked_list};
use snapshot::ArchTrustSnapshot;

const CERTIFICATES_OBJECT: &str = "certificates.pgp";
const REVOKED_OBJECT: &str = "revoked";
const NOT_CARRIED_OBJECT: &str = "revoked.not-carried";

/// Fetches an Arch keyring, authenticates it, and persists the snapshot.
///
/// Both halves of the keyring authority -- certificate material and the
/// disabled-key list -- are written together so a later load cannot silently
/// pick up a stale revoked list.
pub(super) async fn prepare(
    repository_name: &str,
    keyring_dir: &Path,
    keyring: &ArchKeyringTrust,
) -> Result<()> {
    keyring.validate()?;
    let fetched = store::fetch_key_source(&keyring.url).await?;
    let source = extract_keyring_source(&fetched, keyring.format)?;
    ArchTrustSnapshot::authenticate(
        parse_certificates(&source.certificates, keyring)?,
        &source.disabled,
        keyring,
        SystemTime::now(),
    )?;

    let dir = store::arch_snapshot_dir(repository_name, keyring_dir)?;
    store::write_trust_object(&dir.join(CERTIFICATES_OBJECT), &source.certificates)?;
    match source.disabled {
        ArchDisabledKeys::Carried(_) => {
            store::write_trust_object(&dir.join(REVOKED_OBJECT), &source.disabled.serialize())?;
            let _ = std::fs::remove_file(dir.join(NOT_CARRIED_OBJECT));
        }
        ArchDisabledKeys::NotCarried => {
            store::write_trust_object(&dir.join(NOT_CARRIED_OBJECT), b"")?;
            let _ = std::fs::remove_file(dir.join(REVOKED_OBJECT));
        }
    }
    Ok(())
}

/// Loads the prepared keyring and re-authenticates it at the current instant.
///
/// The snapshot is rebuilt on every load because its reference time is "now":
/// expiry and revocation must not be frozen at preparation time.
pub(super) fn load_snapshot(
    repository_name: &str,
    keyring_dir: &Path,
    keyring: &ArchKeyringTrust,
) -> Result<ArchTrustSnapshot> {
    let dir = store::arch_snapshot_dir(repository_name, keyring_dir)?;
    let certificates = store::read_trust_object(
        &dir.join(CERTIFICATES_OBJECT),
        "Arch keyring certificates",
        repository_name,
    )?;
    let disabled = if dir.join(NOT_CARRIED_OBJECT).exists() {
        ArchDisabledKeys::NotCarried
    } else {
        ArchDisabledKeys::Carried(parse_revoked_list(&store::read_trust_object(
            &dir.join(REVOKED_OBJECT),
            "Arch keyring revoked list",
            repository_name,
        )?)?)
    };
    ArchTrustSnapshot::authenticate(
        parse_certificates(&certificates, keyring)?,
        &disabled,
        keyring,
        SystemTime::now(),
    )
}
