// conary-core/src/repository/download/eopkg.rs

//! Final package-identity verification for authenticated eopkg downloads.

use crate::db::models::RepositoryPackage;
use crate::error::{Error, Result};
use crate::packages::traits::PackageFormat as _;
use std::path::Path;

pub(super) fn verify(repo_pkg: &RepositoryPackage, path: &Path, origin: &str) -> Result<()> {
    if !repo_pkg.download_url.starts_with(origin) {
        return Err(Error::TrustError(format!(
            "eopkg package URL '{}' is outside enrolled origin '{}'",
            repo_pkg.download_url, origin
        )));
    }
    let package = crate::packages::eopkg::EopkgPackage::parse(
        path.to_str()
            .ok_or_else(|| Error::ParseError("downloaded eopkg path is not UTF-8".to_string()))?,
    )?;
    if package.name() != repo_pkg.name || package.version() != repo_pkg.version {
        return Err(Error::TrustError(format!(
            "downloaded eopkg identity {} {} disagrees with authenticated index {} {}",
            package.name(),
            package.version(),
            repo_pkg.name,
            repo_pkg.version
        )));
    }
    Ok(())
}
