// conary-core/src/ccs/hooks/capabilities/filesystem_security.rs

//! Target-supplied filesystem security authority for generation carriers.

use serde::{Deserialize, Serialize};
use std::ffi::CString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use crate::payload::{PayloadNodeKind, ResolvedPayloadNode};

const SELINUX_XATTR: &str = "security.selinux";
const MAX_SECURITY_XATTR_VALUE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImmutableBackingSecurityMechanism {
    Selinux,
}

/// Opaque target authority applied to carrier-owned immutable payload backing.
///
/// Composefs preserves logical per-path labels in EROFS. The kernel also
/// authorizes reads from the external CAS object that backs a logical regular
/// file, so the carrier must label that separate storage with a target-owned
/// context. Conary never parses or invents the context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImmutableBackingSecurity {
    pub mechanism: ImmutableBackingSecurityMechanism,
    pub xattr_value: Vec<u8>,
}

impl ImmutableBackingSecurity {
    /// Probe the target's own immutable `/usr` semantic without a distro gate.
    pub fn probe(target_usr: &Path) -> Result<Option<Self>, ImmutableBackingSecurityError> {
        match xattr::get(target_usr, SELINUX_XATTR) {
            Ok(Some(xattr_value)) => {
                let capability = Self {
                    mechanism: ImmutableBackingSecurityMechanism::Selinux,
                    xattr_value,
                };
                capability.validate()?;
                Ok(Some(capability))
            }
            Ok(None) => Ok(None),
            Err(source)
                if source.kind() == std::io::ErrorKind::Unsupported
                    || source.raw_os_error() == Some(libc::EOPNOTSUPP) =>
            {
                Ok(None)
            }
            Err(source) => Err(ImmutableBackingSecurityError::Io {
                operation: "read target immutable-backing security xattr",
                path: target_usr.to_path_buf(),
                source,
            }),
        }
    }

    /// Project the same target fact from an exact, already-captured `/usr` node.
    pub fn from_target_usr_node(
        target_usr: &ResolvedPayloadNode,
    ) -> Result<Option<Self>, ImmutableBackingSecurityError> {
        if !matches!(target_usr.source.kind, PayloadNodeKind::Directory) {
            return Err(ImmutableBackingSecurityError::TargetUsrNotDirectory);
        }
        let Some(xattr_value) = target_usr.source.xattrs.get(SELINUX_XATTR) else {
            return Ok(None);
        };
        let capability = Self {
            mechanism: ImmutableBackingSecurityMechanism::Selinux,
            xattr_value: xattr_value.clone(),
        };
        capability.validate()?;
        Ok(Some(capability))
    }

    pub fn validate(&self) -> Result<(), ImmutableBackingSecurityError> {
        if self.xattr_value.is_empty() || self.xattr_value.len() > MAX_SECURITY_XATTR_VALUE_BYTES {
            return Err(ImmutableBackingSecurityError::InvalidValueSize {
                mechanism: self.mechanism,
                size: self.xattr_value.len(),
                maximum: MAX_SECURITY_XATTR_VALUE_BYTES,
            });
        }
        Ok(())
    }

    /// Apply the opaque target context to every carrier CAS directory and file.
    pub fn apply_to_tree(&self, root: &Path) -> Result<(), ImmutableBackingSecurityError> {
        self.apply_to_tree_with(root, &mut |path, name, value| {
            set_xattr_without_following(path, name, value)
        })
    }

    fn xattr_name(&self) -> &'static str {
        match self.mechanism {
            ImmutableBackingSecurityMechanism::Selinux => SELINUX_XATTR,
        }
    }

    fn apply_to_tree_with(
        &self,
        root: &Path,
        set_xattr: &mut impl FnMut(&Path, &str, &[u8]) -> Result<(), ImmutableBackingSecurityError>,
    ) -> Result<(), ImmutableBackingSecurityError> {
        self.validate()?;
        apply_node_recursive(root, self.xattr_name(), &self.xattr_value, set_xattr)
    }
}

fn apply_node_recursive(
    path: &Path,
    xattr_name: &str,
    xattr_value: &[u8],
    set_xattr: &mut impl FnMut(&Path, &str, &[u8]) -> Result<(), ImmutableBackingSecurityError>,
) -> Result<(), ImmutableBackingSecurityError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|source| ImmutableBackingSecurityError::Io {
            operation: "inspect immutable backing node",
            path: path.to_path_buf(),
            source,
        })?;
    if metadata.file_type().is_symlink()
        || (!metadata.file_type().is_dir() && !metadata.file_type().is_file())
    {
        return Err(ImmutableBackingSecurityError::UnsupportedNode {
            path: path.to_path_buf(),
        });
    }

    set_xattr(path, xattr_name, xattr_value)?;
    if metadata.file_type().is_dir() {
        let mut children = fs::read_dir(path)
            .map_err(|source| ImmutableBackingSecurityError::Io {
                operation: "read immutable backing directory",
                path: path.to_path_buf(),
                source,
            })?
            .map(|entry| {
                entry.map(|entry| entry.path()).map_err(|source| {
                    ImmutableBackingSecurityError::Io {
                        operation: "read immutable backing directory entry",
                        path: path.to_path_buf(),
                        source,
                    }
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        children.sort();
        for child in children {
            apply_node_recursive(&child, xattr_name, xattr_value, set_xattr)?;
        }
    }
    Ok(())
}

fn set_xattr_without_following(
    path: &Path,
    name: &str,
    value: &[u8],
) -> Result<(), ImmutableBackingSecurityError> {
    let c_path = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        ImmutableBackingSecurityError::PathContainsNul {
            path: path.to_path_buf(),
        }
    })?;
    let c_name = CString::new(name).expect("fixed security xattr name is NUL-free");
    let result = unsafe {
        libc::lsetxattr(
            c_path.as_ptr(),
            c_name.as_ptr(),
            value.as_ptr().cast::<libc::c_void>(),
            value.len(),
            0,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(ImmutableBackingSecurityError::Io {
            operation: "set immutable backing security xattr",
            path: path.to_path_buf(),
            source: std::io::Error::last_os_error(),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ImmutableBackingSecurityError {
    #[error(
        "{mechanism:?} immutable-backing xattr has {size} bytes; expected 1..={maximum} opaque bytes"
    )]
    InvalidValueSize {
        mechanism: ImmutableBackingSecurityMechanism,
        size: usize,
        maximum: usize,
    },
    #[error("target /usr immutable-backing authority must be a directory")]
    TargetUsrNotDirectory,
    #[error("immutable backing tree contains an unsupported non-file node: {path}")]
    UnsupportedNode { path: PathBuf },
    #[error("immutable backing path contains NUL: {path}")]
    PathContainsNul { path: PathBuf },
    #[error("failed to {operation} at {path}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn selinux_capability() -> ImmutableBackingSecurity {
        ImmutableBackingSecurity {
            mechanism: ImmutableBackingSecurityMechanism::Selinux,
            xattr_value: b"system_u:object_r:usr_t:s0\0".to_vec(),
        }
    }

    #[test]
    fn opaque_context_round_trips_without_parsing() {
        let capability = selinux_capability();
        let encoded = serde_json::to_vec(&capability).unwrap();
        let decoded: ImmutableBackingSecurity = serde_json::from_slice(&encoded).unwrap();

        assert_eq!(decoded, capability);
        decoded.validate().unwrap();
        assert_eq!(decoded.xattr_name(), SELINUX_XATTR);
    }

    #[test]
    fn captured_target_usr_node_projects_only_exact_xattr_authority() {
        let mut usr = crate::payload::PayloadNode::regular(0o755);
        usr.kind = PayloadNodeKind::Directory;
        usr.mode = libc::S_IFDIR | 0o755;
        usr.xattrs.insert(
            SELINUX_XATTR.to_string(),
            b"system_u:object_r:usr_t:s0\0".to_vec(),
        );
        let usr = ResolvedPayloadNode::from_numeric_source(usr).unwrap();

        assert_eq!(
            ImmutableBackingSecurity::from_target_usr_node(&usr).unwrap(),
            Some(selinux_capability())
        );

        let directory_without_security =
            ResolvedPayloadNode::from_numeric_source(crate::payload::PayloadNode {
                xattrs: Default::default(),
                ..usr.source.clone()
            })
            .unwrap();
        assert_eq!(
            ImmutableBackingSecurity::from_target_usr_node(&directory_without_security).unwrap(),
            None
        );

        let regular =
            ResolvedPayloadNode::from_numeric_source(crate::payload::PayloadNode::regular(0o644))
                .unwrap();
        assert!(matches!(
            ImmutableBackingSecurity::from_target_usr_node(&regular),
            Err(ImmutableBackingSecurityError::TargetUsrNotDirectory)
        ));
    }

    #[test]
    fn empty_or_oversized_context_is_rejected() {
        for xattr_value in [Vec::new(), vec![b'x'; MAX_SECURITY_XATTR_VALUE_BYTES + 1]] {
            let error = ImmutableBackingSecurity {
                mechanism: ImmutableBackingSecurityMechanism::Selinux,
                xattr_value,
            }
            .validate()
            .unwrap_err();
            assert!(matches!(
                error,
                ImmutableBackingSecurityError::InvalidValueSize { .. }
            ));
        }
    }

    #[test]
    fn tree_application_covers_only_regular_files_and_directories() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("objects/ab")).unwrap();
        fs::write(temp.path().join("objects/ab/cdef"), b"payload").unwrap();
        let root = temp.path().join("objects");
        let mut observed = BTreeMap::new();

        selinux_capability()
            .apply_to_tree_with(&root, &mut |path, name, value| {
                observed.insert(
                    path.strip_prefix(&root).unwrap().to_path_buf(),
                    (name.to_string(), value.to_vec()),
                );
                Ok(())
            })
            .unwrap();

        assert_eq!(observed.len(), 3);
        for relative in [Path::new(""), Path::new("ab"), Path::new("ab/cdef")] {
            assert_eq!(
                observed.get(relative),
                Some(&(
                    SELINUX_XATTR.to_string(),
                    b"system_u:object_r:usr_t:s0\0".to_vec()
                ))
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn tree_application_rejects_symlink_nodes_without_following_them() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("objects")).unwrap();
        fs::write(temp.path().join("outside"), b"outside").unwrap();
        std::os::unix::fs::symlink("../outside", temp.path().join("objects/link")).unwrap();

        let error = selinux_capability()
            .apply_to_tree_with(&temp.path().join("objects"), &mut |_, _, _| Ok(()))
            .unwrap_err();

        assert!(matches!(
            error,
            ImmutableBackingSecurityError::UnsupportedNode { path }
                if path.ends_with("objects/link")
        ));
    }
}
