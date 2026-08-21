// conary-core/src/ccs/convert/file_capabilities.rs

//! Exact Linux file-capability projection from source-native payload authority.

use crate::ccs::manifest::{FileCapability, LINUX_SECURITY_CAPABILITY_XATTR, ManifestError};
use crate::packages::payload::PackagePayloadFile;
use crate::payload::PayloadNodeKind;

pub fn file_capabilities_from_native_payload(
    files: &[PackagePayloadFile],
) -> Result<Vec<FileCapability>, ManifestError> {
    let mut capabilities = files
        .iter()
        .filter_map(|file| {
            file.node
                .xattrs
                .get(LINUX_SECURITY_CAPABILITY_XATTR)
                .map(|encoded| (file, encoded))
        })
        .map(|(file, encoded)| {
            if !matches!(file.node.kind, PayloadNodeKind::Regular { .. }) {
                return Err(ManifestError::Invalid(format!(
                    "native file capability target {} is not a regular payload file",
                    file.path
                )));
            }
            FileCapability::from_security_capability_xattr(file.path.clone(), encoded)
        })
        .collect::<Result<Vec<_>, _>>()?;
    capabilities.sort_by(|left, right| left.path.cmp(&right.path));
    if let Some(duplicate) = capabilities
        .windows(2)
        .find(|pair| pair[0].path == pair[1].path)
    {
        return Err(ManifestError::Invalid(format!(
            "native payload declares duplicate file capability authority for {}",
            duplicate[0].path
        )));
    }
    Ok(capabilities)
}
