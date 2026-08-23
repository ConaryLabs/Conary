// crates/conary-core/src/repository/catalog/parity/rpm/ffi.rs

//! Narrow ownership-safe wrapper around the private libsolv C shim.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::Path;
use std::ptr::NonNull;

use crate::error::{Error, Result};

pub(super) const REL_GT: i32 = 1;
pub(super) const REL_EQ: i32 = 2;
pub(super) const REL_LT: i32 = 4;
pub(super) const REL_AND: i32 = 16;
pub(super) const REL_OR: i32 = 17;
pub(super) const REL_WITH: i32 = 18;
pub(super) const REL_COND: i32 = 22;
pub(super) const REL_ELSE: i32 = 26;
pub(super) const REL_WITHOUT: i32 = 28;
pub(super) const REL_UNLESS: i32 = 29;

#[derive(Debug, Clone, Copy)]
#[repr(i32)]
pub(super) enum DependencyField {
    Provides = 1,
    Requires = 2,
    Conflicts = 3,
    Obsoletes = 4,
    Recommends = 5,
    Suggests = 6,
    Supplements = 7,
    Enhances = 8,
}

unsafe extern "C" {
    fn conary_solv_create() -> *mut c_void;
    fn conary_solv_free(handle: *mut c_void);
    fn conary_solv_version() -> *const c_char;
    fn conary_solv_error(handle: *mut c_void) -> *const c_char;
    fn conary_solv_load_rpmmd(
        handle: *mut c_void,
        name: *const c_char,
        primary_path: *const c_char,
        filelists_path: *const c_char,
        member: u32,
    ) -> c_int;
    fn conary_solv_package_count(handle: *mut c_void) -> usize;
    fn conary_solv_package_member(handle: *mut c_void, index: usize) -> u32;
    fn conary_solv_package_name(handle: *mut c_void, index: usize) -> *const c_char;
    fn conary_solv_package_arch(handle: *mut c_void, index: usize) -> *const c_char;
    fn conary_solv_package_evr(handle: *mut c_void, index: usize) -> *const c_char;
    fn conary_solv_package_location(handle: *mut c_void, index: usize) -> *const c_char;
    fn conary_solv_package_checksum(
        handle: *mut c_void,
        index: usize,
        is_sha256: *mut c_int,
    ) -> *const c_char;
    fn conary_solv_package_size(handle: *mut c_void, index: usize, found: *mut c_int) -> u64;
    fn conary_solv_dependency_count(handle: *mut c_void, index: usize, field: c_int) -> usize;
    fn conary_solv_dependency_at(
        handle: *mut c_void,
        index: usize,
        field: c_int,
        dependency_index: usize,
    ) -> c_int;
    fn conary_solv_dependency_is_relation(dependency: c_int) -> c_int;
    fn conary_solv_dependency_flags(handle: *mut c_void, dependency: c_int) -> c_int;
    fn conary_solv_dependency_name(handle: *mut c_void, dependency: c_int) -> c_int;
    fn conary_solv_dependency_evr(handle: *mut c_void, dependency: c_int) -> c_int;
    fn conary_solv_dependency_atom(handle: *mut c_void, dependency: c_int) -> *const c_char;
    fn conary_solv_dependency_text(handle: *mut c_void, dependency: c_int) -> *const c_char;
    fn conary_solv_dependency_is_prereq_marker(dependency: c_int) -> c_int;
    fn conary_solv_file_iterator(handle: *mut c_void, index: usize) -> *mut c_void;
    fn conary_solv_file_next(files: *mut c_void) -> *const c_char;
    fn conary_solv_file_iterator_free(files: *mut c_void);
}

pub(super) struct SolvPool {
    handle: NonNull<c_void>,
}

impl SolvPool {
    pub(super) fn create() -> Result<Self> {
        let handle = unsafe { conary_solv_create() };
        let handle = NonNull::new(handle)
            .ok_or_else(|| Error::InitError("initialize pinned libsolv RPM pool".to_string()))?;
        Ok(Self { handle })
    }

    pub(super) fn version() -> Result<String> {
        copy_required(unsafe { conary_solv_version() }, "libsolv version")
    }

    pub(super) fn load(
        &mut self,
        name: &str,
        primary: &Path,
        filelists: &Path,
        member: u32,
    ) -> Result<()> {
        let name = CString::new(name)
            .map_err(|_| Error::ConfigError("libsolv repository name contains NUL".to_string()))?;
        let primary = path_cstring(primary, "RPM primary metadata")?;
        let filelists = path_cstring(filelists, "RPM filelists metadata")?;
        let result = unsafe {
            conary_solv_load_rpmmd(
                self.handle.as_ptr(),
                name.as_ptr(),
                primary.as_ptr(),
                filelists.as_ptr(),
                member,
            )
        };
        if result == 0 {
            return Err(Error::ParseError(self.last_error()?));
        }
        Ok(())
    }

    pub(super) fn package_count(&self) -> usize {
        unsafe { conary_solv_package_count(self.handle.as_ptr()) }
    }

    pub(super) fn package(&self, index: usize) -> Result<SolvPackage<'_>> {
        if index >= self.package_count() {
            return Err(Error::InternalError(format!(
                "libsolv package index {index} is out of bounds"
            )));
        }
        Ok(SolvPackage { pool: self, index })
    }

    fn last_error(&self) -> Result<String> {
        copy_required(
            unsafe { conary_solv_error(self.handle.as_ptr()) },
            "libsolv error",
        )
    }

    pub(super) fn dependency(&self, id: i32) -> Result<SolvDependency<'_>> {
        // libsolv tags relation IDs with bit 31, so valid relation IDs are
        // negative when carried through its signed `Id` type.
        if id == 0 {
            return Err(Error::ParseError(format!(
                "libsolv returned invalid dependency id {id}"
            )));
        }
        Ok(SolvDependency { pool: self, id })
    }
}

impl Drop for SolvPool {
    fn drop(&mut self) {
        unsafe { conary_solv_free(self.handle.as_ptr()) };
    }
}

pub(super) struct SolvPackage<'a> {
    pool: &'a SolvPool,
    index: usize,
}

impl SolvPackage<'_> {
    pub(super) fn member(&self) -> u32 {
        unsafe { conary_solv_package_member(self.pool.handle.as_ptr(), self.index) }
    }

    pub(super) fn name(&self) -> Result<String> {
        self.required(conary_solv_package_name, "name")
    }

    pub(super) fn arch(&self) -> Result<String> {
        self.required(conary_solv_package_arch, "architecture")
    }

    pub(super) fn evr(&self) -> Result<String> {
        self.required(conary_solv_package_evr, "EVR")
    }

    pub(super) fn location(&self) -> Result<String> {
        self.required(conary_solv_package_location, "location")
    }

    pub(super) fn checksum(&self) -> Result<String> {
        let mut is_sha256 = 0;
        let value = unsafe {
            conary_solv_package_checksum(self.pool.handle.as_ptr(), self.index, &mut is_sha256)
        };
        if is_sha256 != 1 {
            return Err(Error::ParseError(format!(
                "libsolv package {} does not carry an RPMMD SHA-256 checksum",
                self.index
            )));
        }
        copy_required(value, "RPM package checksum")
    }

    pub(super) fn size(&self) -> Result<u64> {
        let mut found = 0;
        let value =
            unsafe { conary_solv_package_size(self.pool.handle.as_ptr(), self.index, &mut found) };
        if found != 1 {
            return Err(Error::ParseError(format!(
                "libsolv package {} has no RPMMD download size",
                self.index
            )));
        }
        Ok(value)
    }

    pub(super) fn dependencies(&self, field: DependencyField) -> Result<Vec<SolvDependency<'_>>> {
        let count = unsafe {
            conary_solv_dependency_count(self.pool.handle.as_ptr(), self.index, field as c_int)
        };
        (0..count)
            .map(|dependency_index| {
                let id = unsafe {
                    conary_solv_dependency_at(
                        self.pool.handle.as_ptr(),
                        self.index,
                        field as c_int,
                        dependency_index,
                    )
                };
                self.pool.dependency(id)
            })
            .collect()
    }

    pub(super) fn files(&self) -> Result<Vec<String>> {
        let files = unsafe { conary_solv_file_iterator(self.pool.handle.as_ptr(), self.index) };
        let files = NonNull::new(files).ok_or_else(|| {
            Error::InitError(format!(
                "initialize libsolv file iterator for package {}",
                self.index
            ))
        })?;
        let iterator = FileIterator { files };
        let mut paths = Vec::new();
        loop {
            let path = unsafe { conary_solv_file_next(iterator.files.as_ptr()) };
            if path.is_null() {
                break;
            }
            paths.push(copy_required(path, "RPM package file path")?);
        }
        Ok(paths)
    }

    fn required(
        &self,
        getter: unsafe extern "C" fn(*mut c_void, usize) -> *const c_char,
        field: &str,
    ) -> Result<String> {
        let value = unsafe { getter(self.pool.handle.as_ptr(), self.index) };
        copy_required(value, &format!("RPM package {field}"))
    }
}

pub(super) struct SolvDependency<'a> {
    pool: &'a SolvPool,
    id: i32,
}

impl SolvDependency<'_> {
    pub(super) const fn id(&self) -> i32 {
        self.id
    }

    pub(super) fn is_prereq_marker(&self) -> bool {
        unsafe { conary_solv_dependency_is_prereq_marker(self.id) == 1 }
    }

    pub(super) fn is_relation(&self) -> bool {
        unsafe { conary_solv_dependency_is_relation(self.id) == 1 }
    }

    pub(super) fn atom(&self) -> Result<String> {
        copy_required(
            unsafe { conary_solv_dependency_atom(self.pool.handle.as_ptr(), self.id) },
            "libsolv dependency atom",
        )
    }

    pub(super) fn text(&self) -> Result<String> {
        copy_required(
            unsafe { conary_solv_dependency_text(self.pool.handle.as_ptr(), self.id) },
            "libsolv dependency text",
        )
    }

    pub(super) fn relation(&self) -> Result<SolvRelation<'_>> {
        if !self.is_relation() {
            return Err(Error::InternalError(format!(
                "libsolv dependency {} is not a relation",
                self.id
            )));
        }
        Ok(SolvRelation {
            pool: self.pool,
            flags: unsafe { conary_solv_dependency_flags(self.pool.handle.as_ptr(), self.id) },
            name: unsafe { conary_solv_dependency_name(self.pool.handle.as_ptr(), self.id) },
            evr: unsafe { conary_solv_dependency_evr(self.pool.handle.as_ptr(), self.id) },
        })
    }
}

pub(super) struct SolvRelation<'a> {
    pool: &'a SolvPool,
    pub(super) flags: i32,
    pub(super) name: i32,
    pub(super) evr: i32,
}

impl SolvRelation<'_> {
    pub(super) fn name_dependency(&self) -> Result<SolvDependency<'_>> {
        self.pool.dependency(self.name)
    }

    pub(super) fn evr_dependency(&self) -> Result<SolvDependency<'_>> {
        self.pool.dependency(self.evr)
    }
}

struct FileIterator {
    files: NonNull<c_void>,
}

impl Drop for FileIterator {
    fn drop(&mut self) {
        unsafe { conary_solv_file_iterator_free(self.files.as_ptr()) };
    }
}

fn path_cstring(path: &Path, label: &str) -> Result<CString> {
    use std::os::unix::ffi::OsStrExt;

    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| Error::ConfigError(format!("{label} path contains NUL")))
}

fn copy_required(value: *const c_char, label: &str) -> Result<String> {
    if value.is_null() {
        return Err(Error::ParseError(format!("libsolv returned no {label}")));
    }
    let value = unsafe { CStr::from_ptr(value) };
    value
        .to_str()
        .map(str::to_string)
        .map_err(|error| Error::ParseError(format!("libsolv {label} is not UTF-8: {error}")))
}
