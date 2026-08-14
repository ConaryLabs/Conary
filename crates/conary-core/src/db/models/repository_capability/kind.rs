// conary-core/src/db/models/repository_capability/kind.rs

//! Current-schema decoding for normalized repository capability kinds.

use crate::error::{Error, Result};
use crate::repository::dependency_model::RepositoryCapabilityKind;

pub(super) fn decode(value: &str) -> Result<RepositoryCapabilityKind> {
    let kind = match value {
        "package" => RepositoryCapabilityKind::PackageName,
        "virtual" => RepositoryCapabilityKind::Virtual,
        "soname" => RepositoryCapabilityKind::Soname,
        "file" => RepositoryCapabilityKind::File,
        "path" => RepositoryCapabilityKind::Path,
        "binary" => RepositoryCapabilityKind::Binary,
        "pkgconfig" => RepositoryCapabilityKind::PkgConfig,
        "pkgconfig32" => RepositoryCapabilityKind::PkgConfig32,
        "comar" => RepositoryCapabilityKind::Comar,
        "generic" => RepositoryCapabilityKind::Generic,
        other => {
            return Err(Error::InternalError(format!(
                "unsupported normalized repository provide kind '{other}'"
            )));
        }
    };
    Ok(kind)
}
