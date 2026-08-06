// conary-core/src/db/models/trove/identity.rs

//! Persistence-boundary validation for exact trove identity authority.

use crate::error::{Error, Result};
use crate::repository::versioning::{
    VersionScheme, validate_package_release, validate_repo_version,
};

use super::{InstallSource, Trove};

pub(super) fn validated_native_identity_json(trove: &Trove) -> Result<Option<String>> {
    validate_repo_version(trove.version_scheme, &trove.version).map_err(|error| {
        Error::ConfigError(format!(
            "{} has a version outside its declared grammar: {error}",
            trove.name
        ))
    })?;
    if let Some(release) = trove.package_release.as_deref() {
        validate_package_release(release).map_err(|error| {
            Error::ConfigError(format!(
                "{} {} has invalid signed CCS release: {error}",
                trove.name, trove.version
            ))
        })?;
    }
    match (trove.version_scheme, trove.debian_multi_arch) {
        (VersionScheme::Debian, Some(_))
        | (VersionScheme::Conary | VersionScheme::Rpm | VersionScheme::Arch, None) => {}
        (VersionScheme::Debian, None) => {
            return Err(Error::ConfigError(format!(
                "{} {} has Debian version authority without exact Multi-Arch metadata",
                trove.name, trove.version
            )));
        }
        (_, Some(_)) => {
            return Err(Error::ConfigError(format!(
                "{} {} carries Debian Multi-Arch metadata under the {} version scheme",
                trove.name,
                trove.version,
                trove.version_scheme.as_str()
            )));
        }
    }
    let native_source = matches!(
        trove.install_source,
        InstallSource::AdoptedTrack | InstallSource::AdoptedFull | InstallSource::Taken
    );
    match (&trove.native_package_identity, native_source) {
        (Some(identity), true) => {
            identity.validate()?;
            if identity.version_scheme() != trove.version_scheme {
                return Err(Error::ConfigError(format!(
                    "{} {} native package identity uses {} versioning, not the trove's {} scheme",
                    trove.name,
                    trove.version,
                    identity.version_scheme().as_str(),
                    trove.version_scheme.as_str()
                )));
            }
            if identity.name() != trove.name {
                return Err(Error::ConfigError(format!(
                    "{} {} native package identity names {}, not the owning trove",
                    trove.name,
                    trove.version,
                    identity.name()
                )));
            }
            if identity.version() != trove.version {
                return Err(Error::ConfigError(format!(
                    "{} {} native package identity has canonical version {}",
                    trove.name,
                    trove.version,
                    identity.version()
                )));
            }
            if Some(identity.architecture()) != trove.architecture.as_deref() {
                return Err(Error::ConfigError(format!(
                    "{} {} native package identity architecture {} does not match trove architecture {:?}",
                    trove.name,
                    trove.version,
                    identity.architecture(),
                    trove.architecture
                )));
            }
            serde_json::to_string(identity)
                .map(Some)
                .map_err(Into::into)
        }
        (None, false) => Ok(None),
        (None, true) => Err(Error::ConfigError(format!(
            "{} {} uses install source {} without an exact native package identity",
            trove.name, trove.version, trove.install_source
        ))),
        (Some(_), false) => Err(Error::ConfigError(format!(
            "{} {} uses install source {} but carries a native package identity",
            trove.name, trove.version, trove.install_source
        ))),
    }
}
