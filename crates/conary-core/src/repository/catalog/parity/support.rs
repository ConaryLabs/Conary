// crates/conary-core/src/repository/catalog/parity/support.rs

//! Shared counter and input-file checks with caller-owned error contracts.

use crate::error::{Error, Result};

pub(super) enum Counter<'a> {
    #[cfg(any(
        feature = "native-rpm-oracle",
        feature = "native-debian-oracle",
        feature = "native-alpm-oracle"
    ))]
    NativeRows(&'a str),
    Survey(&'a str),
}

pub(super) fn checked_increment(value: u64, counter: Counter<'_>) -> Result<u64> {
    value.checked_add(1).ok_or_else(|| match counter {
        #[cfg(any(
            feature = "native-rpm-oracle",
            feature = "native-debian-oracle",
            feature = "native-alpm-oracle"
        ))]
        Counter::NativeRows(label) => Error::InternalError(format!("{label} exceed u64")),
        Counter::Survey(label) => {
            Error::ConfigError(format!("{label} resolution survey count exceeds u64"))
        }
    })
}

#[cfg(any(
    feature = "native-rpm-oracle",
    feature = "native-debian-oracle",
    feature = "native-alpm-oracle"
))]
pub(super) enum RegularFileError {
    #[cfg(feature = "native-rpm-oracle")]
    Config,
    #[cfg(any(feature = "native-debian-oracle", feature = "native-alpm-oracle"))]
    InvalidPath,
}

#[cfg(any(
    feature = "native-rpm-oracle",
    feature = "native-debian-oracle",
    feature = "native-alpm-oracle"
))]
pub(super) fn require_regular_file(
    path: &std::path::Path,
    label: &str,
    errors: RegularFileError,
) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| match errors {
        #[cfg(feature = "native-rpm-oracle")]
        RegularFileError::Config => {
            Error::ConfigError(format!("inspect {label} '{}': {error}", path.display()))
        }
        #[cfg(any(feature = "native-debian-oracle", feature = "native-alpm-oracle"))]
        RegularFileError::InvalidPath => Error::from(error),
    })?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(match errors {
            #[cfg(feature = "native-rpm-oracle")]
            RegularFileError::Config => Error::ConfigError(format!(
                "{label} '{}' must be a regular file, never a symlink",
                path.display()
            )),
            #[cfg(any(feature = "native-debian-oracle", feature = "native-alpm-oracle"))]
            RegularFileError::InvalidPath => Error::InvalidPath(format!(
                "{label} {} must be a regular file, never a symlink",
                path.display()
            )),
        });
    }
    Ok(())
}
