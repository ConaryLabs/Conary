// crates/conary-core/src/repository/catalog/parity/debian/ffi.rs

//! Ownership-safe Rust projection of the narrow apt-pkg C++ ABI.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::Path;
use std::ptr::NonNull;
use std::sync::{Mutex, MutexGuard};

use crate::error::{Error, Result};

static APT_PKG_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AptArchitectureQualifier {
    Unqualified,
    Any,
    Native,
    Exact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AptRelationKind {
    Depends,
    PreDepends,
    Recommends,
    Suggests,
    Enhances,
    Conflicts,
    Breaks,
    Replaces,
}

#[derive(Debug)]
pub(super) struct AptAtom {
    pub(super) name: String,
    pub(super) version: Option<String>,
    pub(super) relation: i32,
    pub(super) native_text: String,
    pub(super) architecture_qualifier: AptArchitectureQualifier,
    pub(super) architecture: Option<String>,
}

#[derive(Debug)]
pub(super) struct AptRelationGroup {
    pub(super) kind: AptRelationKind,
    pub(super) native_text: String,
    pub(super) atoms: Vec<AptAtom>,
}

#[derive(Debug)]
pub(super) struct AptProvide {
    pub(super) name: String,
    pub(super) version: Option<String>,
    pub(super) native_text: String,
    pub(super) architecture_qualifier: AptArchitectureQualifier,
    pub(super) architecture: Option<String>,
}

#[derive(Debug)]
pub(super) struct AptPackage {
    pub(super) name: String,
    pub(super) version: String,
    pub(super) architecture: String,
    pub(super) multi_arch: Option<String>,
    pub(super) filename: String,
    pub(super) sha256: String,
    pub(super) size: String,
    pub(super) provides: Vec<AptProvide>,
    pub(super) relation_groups: Vec<AptRelationGroup>,
}

unsafe extern "C" {
    fn conary_apt_pkg_version() -> *const c_char;
    fn conary_apt_last_error() -> *const c_char;
    fn conary_apt_open(path: *const c_char) -> *mut c_void;
    fn conary_apt_free(handle: *mut c_void);
    fn conary_apt_package_count(handle: *const c_void) -> usize;
    fn conary_apt_package_name(handle: *const c_void, package: usize) -> *const c_char;
    fn conary_apt_package_version(handle: *const c_void, package: usize) -> *const c_char;
    fn conary_apt_package_architecture(handle: *const c_void, package: usize) -> *const c_char;
    fn conary_apt_package_multi_arch(handle: *const c_void, package: usize) -> *const c_char;
    fn conary_apt_package_filename(handle: *const c_void, package: usize) -> *const c_char;
    fn conary_apt_package_sha256(handle: *const c_void, package: usize) -> *const c_char;
    fn conary_apt_package_size(handle: *const c_void, package: usize) -> *const c_char;
    fn conary_apt_provide_count(handle: *const c_void, package: usize) -> usize;
    fn conary_apt_provide_name(
        handle: *const c_void,
        package: usize,
        provide: usize,
    ) -> *const c_char;
    fn conary_apt_provide_version(
        handle: *const c_void,
        package: usize,
        provide: usize,
    ) -> *const c_char;
    fn conary_apt_provide_native_text(
        handle: *const c_void,
        package: usize,
        provide: usize,
    ) -> *const c_char;
    fn conary_apt_provide_architecture(
        handle: *const c_void,
        package: usize,
        provide: usize,
    ) -> *const c_char;
    fn conary_apt_provide_architecture_qualifier(
        handle: *const c_void,
        package: usize,
        provide: usize,
    ) -> c_int;
    fn conary_apt_relation_group_count(handle: *const c_void, package: usize) -> usize;
    fn conary_apt_relation_group_kind(handle: *const c_void, package: usize, group: usize)
    -> c_int;
    fn conary_apt_relation_group_native_text(
        handle: *const c_void,
        package: usize,
        group: usize,
    ) -> *const c_char;
    fn conary_apt_relation_atom_count(handle: *const c_void, package: usize, group: usize)
    -> usize;
    fn conary_apt_relation_atom_name(
        handle: *const c_void,
        package: usize,
        group: usize,
        atom: usize,
    ) -> *const c_char;
    fn conary_apt_relation_atom_version(
        handle: *const c_void,
        package: usize,
        group: usize,
        atom: usize,
    ) -> *const c_char;
    fn conary_apt_relation_atom_native_text(
        handle: *const c_void,
        package: usize,
        group: usize,
        atom: usize,
    ) -> *const c_char;
    fn conary_apt_relation_atom_architecture(
        handle: *const c_void,
        package: usize,
        group: usize,
        atom: usize,
    ) -> *const c_char;
    fn conary_apt_relation_atom_relation(
        handle: *const c_void,
        package: usize,
        group: usize,
        atom: usize,
    ) -> c_int;
    fn conary_apt_relation_atom_architecture_qualifier(
        handle: *const c_void,
        package: usize,
        group: usize,
        atom: usize,
    ) -> c_int;
}

#[derive(Debug)]
pub(super) struct AptPackages {
    handle: NonNull<c_void>,
    _guard: MutexGuard<'static, ()>,
}

impl AptPackages {
    pub(super) fn version() -> Result<String> {
        let _guard = apt_pkg_lock()?;
        copy_required(unsafe { conary_apt_pkg_version() }, "apt-pkg version")
    }

    pub(super) fn open(path: &Path) -> Result<Self> {
        let guard = apt_pkg_lock()?;
        let path = path
            .to_str()
            .ok_or_else(|| Error::InvalidPath(format!("path is not UTF-8: {}", path.display())))?;
        let path = CString::new(path)
            .map_err(|_| Error::InvalidPath("Debian Packages path contains NUL".to_string()))?;
        let handle = NonNull::new(unsafe { conary_apt_open(path.as_ptr()) }).ok_or_else(|| {
            Error::ParseError(
                copy_required(unsafe { conary_apt_last_error() }, "apt-pkg error")
                    .unwrap_or_else(|error| error.to_string()),
            )
        })?;
        Ok(Self {
            handle,
            _guard: guard,
        })
    }

    pub(super) fn packages(&self) -> Result<Vec<AptPackage>> {
        let count = unsafe { conary_apt_package_count(self.handle.as_ptr()) };
        (0..count).map(|index| self.package(index)).collect()
    }

    fn package(&self, package: usize) -> Result<AptPackage> {
        let provides = (0..unsafe { conary_apt_provide_count(self.handle.as_ptr(), package) })
            .map(|provide| self.provide(package, provide))
            .collect::<Result<Vec<_>>>()?;
        let relation_groups =
            (0..unsafe { conary_apt_relation_group_count(self.handle.as_ptr(), package) })
                .map(|group| self.relation_group(package, group))
                .collect::<Result<Vec<_>>>()?;
        Ok(AptPackage {
            name: self.package_string(package, conary_apt_package_name, "package name")?,
            version: self.package_string(package, conary_apt_package_version, "package version")?,
            architecture: self.package_string(
                package,
                conary_apt_package_architecture,
                "package architecture",
            )?,
            multi_arch: nonempty(self.package_string(
                package,
                conary_apt_package_multi_arch,
                "package Multi-Arch",
            )?),
            filename: self.package_string(
                package,
                conary_apt_package_filename,
                "package filename",
            )?,
            sha256: self.package_string(package, conary_apt_package_sha256, "package SHA256")?,
            size: self.package_string(package, conary_apt_package_size, "package size")?,
            provides,
            relation_groups,
        })
    }

    fn provide(&self, package: usize, provide: usize) -> Result<AptProvide> {
        Ok(AptProvide {
            name: copy_required(
                unsafe { conary_apt_provide_name(self.handle.as_ptr(), package, provide) },
                "apt-pkg provide name",
            )?,
            version: nonempty(copy_required(
                unsafe { conary_apt_provide_version(self.handle.as_ptr(), package, provide) },
                "apt-pkg provide version",
            )?),
            native_text: copy_required(
                unsafe { conary_apt_provide_native_text(self.handle.as_ptr(), package, provide) },
                "apt-pkg provide native text",
            )?,
            architecture_qualifier: architecture_qualifier(unsafe {
                conary_apt_provide_architecture_qualifier(self.handle.as_ptr(), package, provide)
            })?,
            architecture: nonempty(copy_required(
                unsafe { conary_apt_provide_architecture(self.handle.as_ptr(), package, provide) },
                "apt-pkg provide architecture",
            )?),
        })
    }

    fn relation_group(&self, package: usize, group: usize) -> Result<AptRelationGroup> {
        let kind =
            match unsafe { conary_apt_relation_group_kind(self.handle.as_ptr(), package, group) } {
                1 => AptRelationKind::Depends,
                2 => AptRelationKind::PreDepends,
                3 => AptRelationKind::Recommends,
                4 => AptRelationKind::Suggests,
                5 => AptRelationKind::Enhances,
                6 => AptRelationKind::Conflicts,
                7 => AptRelationKind::Breaks,
                8 => AptRelationKind::Replaces,
                value => {
                    return Err(Error::ParseError(format!(
                        "apt-pkg returned unknown relation kind {value}"
                    )));
                }
            };
        let atoms =
            (0..unsafe { conary_apt_relation_atom_count(self.handle.as_ptr(), package, group) })
                .map(|atom| self.relation_atom(package, group, atom))
                .collect::<Result<Vec<_>>>()?;
        Ok(AptRelationGroup {
            kind,
            native_text: copy_required(
                unsafe {
                    conary_apt_relation_group_native_text(self.handle.as_ptr(), package, group)
                },
                "apt-pkg relation group native text",
            )?,
            atoms,
        })
    }

    fn relation_atom(&self, package: usize, group: usize, atom: usize) -> Result<AptAtom> {
        Ok(AptAtom {
            name: copy_required(
                unsafe {
                    conary_apt_relation_atom_name(self.handle.as_ptr(), package, group, atom)
                },
                "apt-pkg relation atom name",
            )?,
            version: nonempty(copy_required(
                unsafe {
                    conary_apt_relation_atom_version(self.handle.as_ptr(), package, group, atom)
                },
                "apt-pkg relation atom version",
            )?),
            relation: unsafe {
                conary_apt_relation_atom_relation(self.handle.as_ptr(), package, group, atom)
            },
            native_text: copy_required(
                unsafe {
                    conary_apt_relation_atom_native_text(self.handle.as_ptr(), package, group, atom)
                },
                "apt-pkg relation atom native text",
            )?,
            architecture_qualifier: architecture_qualifier(unsafe {
                conary_apt_relation_atom_architecture_qualifier(
                    self.handle.as_ptr(),
                    package,
                    group,
                    atom,
                )
            })?,
            architecture: nonempty(copy_required(
                unsafe {
                    conary_apt_relation_atom_architecture(
                        self.handle.as_ptr(),
                        package,
                        group,
                        atom,
                    )
                },
                "apt-pkg relation atom architecture",
            )?),
        })
    }

    fn package_string(
        &self,
        package: usize,
        getter: unsafe extern "C" fn(*const c_void, usize) -> *const c_char,
        label: &str,
    ) -> Result<String> {
        copy_required(unsafe { getter(self.handle.as_ptr(), package) }, label)
    }
}

impl Drop for AptPackages {
    fn drop(&mut self) {
        unsafe { conary_apt_free(self.handle.as_ptr()) };
    }
}

fn architecture_qualifier(value: i32) -> Result<AptArchitectureQualifier> {
    match value {
        0 => Ok(AptArchitectureQualifier::Unqualified),
        1 => Ok(AptArchitectureQualifier::Any),
        2 => Ok(AptArchitectureQualifier::Native),
        3 => Ok(AptArchitectureQualifier::Exact),
        _ => Err(Error::ParseError(format!(
            "apt-pkg returned unknown architecture qualifier {value}"
        ))),
    }
}

fn apt_pkg_lock() -> Result<MutexGuard<'static, ()>> {
    APT_PKG_LOCK
        .lock()
        .map_err(|_| Error::InternalError("apt-pkg parser lock is poisoned".to_string()))
}

fn nonempty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

fn copy_required(pointer: *const c_char, label: &str) -> Result<String> {
    if pointer.is_null() {
        return Err(Error::ParseError(format!("apt-pkg returned no {label}")));
    }
    unsafe { CStr::from_ptr(pointer) }
        .to_str()
        .map(str::to_string)
        .map_err(|error| Error::ParseError(format!("apt-pkg {label} is not UTF-8: {error}")))
}
