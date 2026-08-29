// crates/conary-core/src/repository/catalog/portable_vfs.rs

//! Read-only SQLite VFS over a portable authenticated catalog view.
//!
//! SQLite receives only bytes copied from complete fixed-size chunks which
//! matched the durable portable manifest.  The carrier file may live on any
//! filesystem: it is merely storage, never integrity authority.

use std::collections::{HashMap, VecDeque};
use std::ffi::{CStr, c_char, c_int, c_void};
use std::fs::File;
use std::mem::{ManuallyDrop, size_of};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::slice;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use rusqlite::ffi;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};

use super::portable_integrity::PortableChunkManifestV1;
use crate::error::{Error, Result};

const PORTABLE_VFS_NAME: &[u8] = b"conary-catalog-portable-v1\0";
const VERIFIED_CHUNK_CACHE_BYTES: usize = 8 * 1024 * 1024;

/// Exact VFS work performed by one authenticated catalog connection.
///
/// These counters describe attempted and completed work, not authority.  They
/// are atomically readable so production evidence can sample them without
/// changing SQLite's connection ownership.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableVfsMetricsV1 {
    pub read_calls: u64,
    pub requested_bytes: u64,
    pub returned_bytes: u64,
    pub chunk_accesses: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub carrier_bytes_requested: u64,
    pub authenticated_chunks: u64,
    pub authenticated_bytes: u64,
    pub short_reads: u64,
    pub integrity_failures: u64,
}

#[derive(Debug, Default)]
struct PortableVfsMetrics {
    read_calls: AtomicU64,
    requested_bytes: AtomicU64,
    returned_bytes: AtomicU64,
    chunk_accesses: AtomicU64,
    cache_hits: AtomicU64,
    cache_misses: AtomicU64,
    carrier_bytes_requested: AtomicU64,
    authenticated_chunks: AtomicU64,
    authenticated_bytes: AtomicU64,
    short_reads: AtomicU64,
    integrity_failures: AtomicU64,
}

impl PortableVfsMetrics {
    fn snapshot(&self) -> PortableVfsMetricsV1 {
        PortableVfsMetricsV1 {
            read_calls: self.read_calls.load(Ordering::Relaxed),
            requested_bytes: self.requested_bytes.load(Ordering::Relaxed),
            returned_bytes: self.returned_bytes.load(Ordering::Relaxed),
            chunk_accesses: self.chunk_accesses.load(Ordering::Relaxed),
            cache_hits: self.cache_hits.load(Ordering::Relaxed),
            cache_misses: self.cache_misses.load(Ordering::Relaxed),
            carrier_bytes_requested: self.carrier_bytes_requested.load(Ordering::Relaxed),
            authenticated_chunks: self.authenticated_chunks.load(Ordering::Relaxed),
            authenticated_bytes: self.authenticated_bytes.load(Ordering::Relaxed),
            short_reads: self.short_reads.load(Ordering::Relaxed),
            integrity_failures: self.integrity_failures.load(Ordering::Relaxed),
        }
    }
}

/// Stable failure class retained after an authenticated carrier read refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortableVfsFailureKindV1 {
    Protocol,
    Authentication,
}

/// First exact authenticated-read failure for one connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortableVfsFailureV1 {
    pub kind: PortableVfsFailureKindV1,
    pub chunk_index: Option<u64>,
    pub detail: String,
}

#[derive(Debug)]
struct VerifiedChunkCache {
    capacity: usize,
    chunks: HashMap<u64, Arc<[u8]>>,
    least_to_most_recent: VecDeque<u64>,
}

impl VerifiedChunkCache {
    fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            chunks: HashMap::new(),
            least_to_most_recent: VecDeque::new(),
        }
    }

    fn get(&mut self, index: u64) -> Option<Arc<[u8]>> {
        let bytes = self.chunks.get(&index)?.clone();
        self.touch(index);
        Some(bytes)
    }

    fn insert(&mut self, index: u64, bytes: Arc<[u8]>) {
        self.chunks.insert(index, bytes);
        self.touch(index);
        while self.chunks.len() > self.capacity {
            let Some(evicted) = self.least_to_most_recent.pop_front() else {
                break;
            };
            self.chunks.remove(&evicted);
        }
    }

    fn touch(&mut self, index: u64) {
        if let Some(position) = self
            .least_to_most_recent
            .iter()
            .position(|candidate| *candidate == index)
        {
            self.least_to_most_recent.remove(position);
        }
        self.least_to_most_recent.push_back(index);
    }
}

#[derive(Debug)]
struct VerifiedCatalogFile {
    carrier: File,
    manifest: PortableChunkManifestV1,
    cache: Mutex<VerifiedChunkCache>,
    metrics: PortableVfsMetrics,
    first_failure: Mutex<Option<PortableVfsFailureV1>>,
}

impl VerifiedCatalogFile {
    fn new(carrier: File, manifest: PortableChunkManifestV1) -> Result<Self> {
        let actual_size = carrier.metadata()?.len();
        if actual_size != manifest.catalog_size() {
            return Err(Error::ChecksumMismatch {
                expected: format!("{} bytes", manifest.catalog_size()),
                actual: format!("{actual_size} bytes"),
            });
        }
        let chunk_size = usize::try_from(manifest.chunk_size()).map_err(|_| {
            Error::ConfigError("portable catalog chunk size exceeds platform usize".to_string())
        })?;
        let capacity = VERIFIED_CHUNK_CACHE_BYTES
            .checked_div(chunk_size)
            .unwrap_or(0)
            .max(1);
        Ok(Self {
            carrier,
            manifest,
            cache: Mutex::new(VerifiedChunkCache::new(capacity)),
            metrics: PortableVfsMetrics::default(),
            first_failure: Mutex::new(None),
        })
    }

    fn catalog_size(&self) -> u64 {
        self.manifest.catalog_size()
    }

    fn metrics(&self) -> PortableVfsMetricsV1 {
        self.metrics.snapshot()
    }

    fn first_failure(&self) -> Option<PortableVfsFailureV1> {
        self.first_failure
            .lock()
            .ok()
            .and_then(|failure| failure.clone())
    }

    fn record_failure(&self, failure: PortableVfsFailureV1) {
        self.metrics
            .integrity_failures
            .fetch_add(1, Ordering::Relaxed);
        if let Ok(mut first_failure) = self.first_failure.lock()
            && first_failure.is_none()
        {
            *first_failure = Some(failure);
        }
    }

    fn read(&self, offset: u64, output: &mut [u8]) -> std::result::Result<bool, ()> {
        self.metrics.read_calls.fetch_add(1, Ordering::Relaxed);
        self.metrics
            .requested_bytes
            .fetch_add(saturating_u64(output.len()), Ordering::Relaxed);
        output.fill(0);

        let requested_end = offset
            .checked_add(saturating_u64(output.len()))
            .ok_or_else(|| {
                self.record_failure(PortableVfsFailureV1 {
                    kind: PortableVfsFailureKindV1::Protocol,
                    chunk_index: None,
                    detail: "SQLite read offset overflowed the portable catalog address space"
                        .to_string(),
                });
            })?;
        let available_end = requested_end.min(self.catalog_size());
        let mut position = offset.min(available_end);
        let mut output_offset = 0usize;

        while position < available_end {
            let chunk_index = position / u64::from(self.manifest.chunk_size());
            self.metrics.chunk_accesses.fetch_add(1, Ordering::Relaxed);
            let chunk = self.verified_chunk(chunk_index)?;
            let range = self.manifest.chunk_range(chunk_index).map_err(|error| {
                self.record_failure(PortableVfsFailureV1 {
                    kind: PortableVfsFailureKindV1::Protocol,
                    chunk_index: Some(chunk_index),
                    detail: error.to_string(),
                });
            })?;
            let within_chunk = usize::try_from(position - range.offset).map_err(|_| {
                self.record_failure(PortableVfsFailureV1 {
                    kind: PortableVfsFailureKindV1::Protocol,
                    chunk_index: Some(chunk_index),
                    detail: "portable catalog chunk offset exceeds platform usize".to_string(),
                });
            })?;
            let remaining = usize::try_from(available_end - position).map_err(|_| {
                self.record_failure(PortableVfsFailureV1 {
                    kind: PortableVfsFailureKindV1::Protocol,
                    chunk_index: Some(chunk_index),
                    detail: "portable catalog read length exceeds platform usize".to_string(),
                });
            })?;
            let copied = remaining.min(chunk.len().saturating_sub(within_chunk));
            if copied == 0 {
                self.record_failure(PortableVfsFailureV1 {
                    kind: PortableVfsFailureKindV1::Protocol,
                    chunk_index: Some(chunk_index),
                    detail: "portable catalog manifest exposed an empty overlapping chunk"
                        .to_string(),
                });
                return Err(());
            }
            output[output_offset..output_offset + copied]
                .copy_from_slice(&chunk[within_chunk..within_chunk + copied]);
            output_offset += copied;
            position = position.saturating_add(saturating_u64(copied));
        }

        self.metrics
            .returned_bytes
            .fetch_add(saturating_u64(output_offset), Ordering::Relaxed);
        let short = available_end != requested_end;
        if short {
            self.metrics.short_reads.fetch_add(1, Ordering::Relaxed);
        }
        Ok(short)
    }

    fn verified_chunk(&self, index: u64) -> std::result::Result<Arc<[u8]>, ()> {
        let mut cache = self.cache.lock().map_err(|_| {
            self.record_failure(PortableVfsFailureV1 {
                kind: PortableVfsFailureKindV1::Protocol,
                chunk_index: Some(index),
                detail: "portable catalog verified-byte cache is poisoned".to_string(),
            });
        })?;
        if let Some(bytes) = cache.get(index) {
            self.metrics.cache_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(bytes);
        }

        self.metrics.cache_misses.fetch_add(1, Ordering::Relaxed);
        let range = self.manifest.chunk_range(index).map_err(|error| {
            self.record_failure(PortableVfsFailureV1 {
                kind: PortableVfsFailureKindV1::Protocol,
                chunk_index: Some(index),
                detail: error.to_string(),
            });
        })?;
        self.metrics
            .carrier_bytes_requested
            .fetch_add(u64::from(range.length), Ordering::Relaxed);
        let bytes = self
            .manifest
            .read_verified_chunk(&self.carrier, index)
            .map_err(|error| {
                self.record_failure(PortableVfsFailureV1 {
                    kind: PortableVfsFailureKindV1::Authentication,
                    chunk_index: Some(index),
                    detail: error.to_string(),
                });
            })?;
        self.metrics
            .authenticated_chunks
            .fetch_add(1, Ordering::Relaxed);
        self.metrics
            .authenticated_bytes
            .fetch_add(saturating_u64(bytes.len()), Ordering::Relaxed);
        let bytes: Arc<[u8]> = Arc::from(bytes);
        cache.insert(index, bytes.clone());
        Ok(bytes)
    }
}

fn saturating_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[derive(Debug)]
struct RegisteredCatalog {
    file: Arc<VerifiedCatalogFile>,
    claimed: bool,
}

fn catalog_registry() -> &'static Mutex<HashMap<Vec<u8>, RegisteredCatalog>> {
    static REGISTRY: OnceLock<Mutex<HashMap<Vec<u8>, RegisteredCatalog>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Debug)]
struct CatalogTokenLease {
    token: Vec<u8>,
}

impl CatalogTokenLease {
    fn register(file: Arc<VerifiedCatalogFile>) -> Result<Self> {
        let token = format!("conary-catalog-{}", uuid::Uuid::new_v4().simple()).into_bytes();
        let mut registry = catalog_registry().lock().map_err(|_| {
            Error::InternalError("portable catalog VFS registry is poisoned".to_string())
        })?;
        if registry
            .insert(
                token.clone(),
                RegisteredCatalog {
                    file,
                    claimed: false,
                },
            )
            .is_some()
        {
            return Err(Error::InternalError(
                "portable catalog VFS token collision".to_string(),
            ));
        }
        Ok(Self { token })
    }

    fn path(&self) -> Result<&str> {
        std::str::from_utf8(&self.token).map_err(|_| {
            Error::InternalError("portable catalog VFS token is not UTF-8".to_string())
        })
    }
}

impl Drop for CatalogTokenLease {
    fn drop(&mut self) {
        if let Ok(mut registry) = catalog_registry().lock() {
            registry.remove(&self.token);
        }
    }
}

/// SQLite connection whose main database is a portable authenticated view.
pub struct PortableCatalogConnection {
    connection: Option<Connection>,
    lease: Option<CatalogTokenLease>,
    file: Arc<VerifiedCatalogFile>,
}

impl PortableCatalogConnection {
    /// Open an already anchored catalog through the process-wide authenticated
    /// VFS.  The VFS token is unforgeable, connection-scoped, and never names
    /// a carrier path.
    pub fn open(carrier: File, manifest: PortableChunkManifestV1) -> Result<Self> {
        ensure_portable_vfs_registered()?;
        let file = Arc::new(VerifiedCatalogFile::new(carrier, manifest)?);
        let lease = CatalogTokenLease::register(file.clone())?;
        let connection = match Connection::open_with_flags_and_vfs(
            lease.path()?,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
            CStr::from_bytes_with_nul(PORTABLE_VFS_NAME).expect("static VFS name is valid"),
        ) {
            Ok(connection) => connection,
            Err(error) => return Err(vfs_or_database_error(&file, error)),
        };
        if let Err(error) = connection.execute_batch(
            "PRAGMA query_only = ON;
             PRAGMA foreign_keys = ON;
             PRAGMA trusted_schema = OFF;
             PRAGMA temp_store = MEMORY;
             PRAGMA mmap_size = 0;",
        ) {
            return Err(vfs_or_database_error(&file, error));
        }
        if let Err(error) =
            connection.query_row("PRAGMA schema_version", [], |row| row.get::<_, i64>(0))
        {
            return Err(vfs_or_database_error(&file, error));
        }
        Ok(Self {
            connection: Some(connection),
            lease: Some(lease),
            file,
        })
    }

    #[must_use]
    pub fn connection(&self) -> &Connection {
        self.connection
            .as_ref()
            .expect("portable catalog connection remains present until drop")
    }

    #[must_use]
    pub fn metrics(&self) -> PortableVfsMetricsV1 {
        self.file.metrics()
    }

    #[must_use]
    pub fn first_failure(&self) -> Option<PortableVfsFailureV1> {
        self.file.first_failure()
    }

    pub fn close(mut self) -> Result<()> {
        let result = self
            .connection
            .take()
            .expect("portable catalog connection is closed once")
            .close()
            .map_err(|(_, error)| vfs_or_database_error(&self.file, error));
        self.lease.take();
        result
    }
}

impl Drop for PortableCatalogConnection {
    fn drop(&mut self) {
        self.connection.take();
        self.lease.take();
    }
}

fn vfs_or_database_error(file: &VerifiedCatalogFile, error: rusqlite::Error) -> Error {
    match file.first_failure() {
        Some(failure) => Error::ConflictError(format!(
            "portable catalog authenticated read failed ({:?}, chunk {:?}): {}",
            failure.kind, failure.chunk_index, failure.detail
        )),
        None => Error::Database(error),
    }
}

fn ensure_portable_vfs_registered() -> Result<()> {
    static REGISTRATION: OnceLock<std::result::Result<(), String>> = OnceLock::new();
    match REGISTRATION.get_or_init(register_portable_vfs) {
        Ok(()) => Ok(()),
        Err(error) => Err(Error::InitError(error.clone())),
    }
}

fn register_portable_vfs() -> std::result::Result<(), String> {
    // SAFETY: SQLite owns one process-global VFS registry.  The boxed VFS is
    // deliberately leaked after successful non-default registration, so every
    // callback pointer and pAppData target remains valid until process exit.
    unsafe {
        let initialized = ffi::sqlite3_initialize();
        if initialized != ffi::SQLITE_OK {
            return Err(format!(
                "initialize SQLite before registering portable catalog VFS: {initialized}"
            ));
        }
        let lower = ffi::sqlite3_vfs_find(ptr::null());
        if lower.is_null() {
            return Err("SQLite has no default VFS for process services".to_string());
        }
        let size = c_int::try_from(size_of::<PortableSqliteFile>())
            .map_err(|_| "portable catalog sqlite3_file size exceeds c_int".to_string())?;
        let vfs = Box::new(ffi::sqlite3_vfs {
            iVersion: 1,
            szOsFile: size,
            mxPathname: 255,
            pNext: ptr::null_mut(),
            zName: PORTABLE_VFS_NAME.as_ptr().cast(),
            pAppData: lower.cast(),
            xOpen: Some(portable_vfs_open),
            xDelete: Some(portable_vfs_delete),
            xAccess: Some(portable_vfs_access),
            xFullPathname: Some(portable_vfs_full_pathname),
            xDlOpen: None,
            xDlError: None,
            xDlSym: None,
            xDlClose: None,
            xRandomness: Some(portable_vfs_randomness),
            xSleep: Some(portable_vfs_sleep),
            xCurrentTime: Some(portable_vfs_current_time),
            xGetLastError: Some(portable_vfs_last_error),
            xCurrentTimeInt64: None,
            xSetSystemCall: None,
            xGetSystemCall: None,
            xNextSystemCall: None,
        });
        let raw = Box::into_raw(vfs);
        let registered = ffi::sqlite3_vfs_register(raw, 0);
        if registered != ffi::SQLITE_OK {
            drop(Box::from_raw(raw));
            return Err(format!(
                "register portable catalog SQLite VFS: {registered}"
            ));
        }
        Ok(())
    }
}

#[repr(C)]
struct PortableSqliteFile {
    base: ffi::sqlite3_file,
    file: ManuallyDrop<Arc<VerifiedCatalogFile>>,
}

static PORTABLE_IO_METHODS: ffi::sqlite3_io_methods = ffi::sqlite3_io_methods {
    iVersion: 1,
    xClose: Some(portable_file_close),
    xRead: Some(portable_file_read),
    xWrite: Some(portable_file_write),
    xTruncate: Some(portable_file_truncate),
    xSync: Some(portable_file_sync),
    xFileSize: Some(portable_file_size),
    xLock: Some(portable_file_lock),
    xUnlock: Some(portable_file_unlock),
    xCheckReservedLock: Some(portable_file_reserved_lock),
    xFileControl: Some(portable_file_control),
    xSectorSize: Some(portable_file_sector_size),
    xDeviceCharacteristics: Some(portable_file_characteristics),
    xShmMap: None,
    xShmLock: None,
    xShmBarrier: None,
    xShmUnmap: None,
    xFetch: None,
    xUnfetch: None,
};

unsafe extern "C" fn portable_vfs_open(
    _vfs: *mut ffi::sqlite3_vfs,
    name: ffi::sqlite3_filename,
    output: *mut ffi::sqlite3_file,
    flags: c_int,
    output_flags: *mut c_int,
) -> c_int {
    ffi_boundary(ffi::SQLITE_CANTOPEN, || {
        // SAFETY: SQLite supplies storage of the registered szOsFile size.
        unsafe { portable_vfs_open_inner(name, output, flags, output_flags) }
    })
}

unsafe fn portable_vfs_open_inner(
    name: ffi::sqlite3_filename,
    output: *mut ffi::sqlite3_file,
    flags: c_int,
    output_flags: *mut c_int,
) -> c_int {
    if output.is_null() {
        return ffi::SQLITE_CANTOPEN;
    }
    // SAFETY: output is non-null SQLite-owned sqlite3_file storage.
    unsafe { (*output).pMethods = ptr::null() };
    if name.is_null() {
        return ffi::SQLITE_CANTOPEN;
    }
    let forbidden = ffi::SQLITE_OPEN_READWRITE
        | ffi::SQLITE_OPEN_CREATE
        | ffi::SQLITE_OPEN_DELETEONCLOSE
        | ffi::SQLITE_OPEN_MAIN_JOURNAL
        | ffi::SQLITE_OPEN_TEMP_DB
        | ffi::SQLITE_OPEN_TEMP_JOURNAL
        | ffi::SQLITE_OPEN_TRANSIENT_DB
        | ffi::SQLITE_OPEN_SUBJOURNAL
        | ffi::SQLITE_OPEN_SUPER_JOURNAL
        | ffi::SQLITE_OPEN_WAL;
    if flags & ffi::SQLITE_OPEN_MAIN_DB == 0
        || flags & ffi::SQLITE_OPEN_READONLY == 0
        || flags & forbidden != 0
    {
        return ffi::SQLITE_CANTOPEN;
    }
    // SAFETY: SQLite documents zName as a NUL-terminated filename.
    let token = unsafe { CStr::from_ptr(name) }.to_bytes();
    let file = {
        let Ok(mut registry) = catalog_registry().lock() else {
            return ffi::SQLITE_IOERR;
        };
        let Some(registered) = registry.get_mut(token) else {
            return ffi::SQLITE_CANTOPEN;
        };
        if registered.claimed {
            return ffi::SQLITE_CANTOPEN;
        }
        registered.claimed = true;
        registered.file.clone()
    };
    let sqlite_file = PortableSqliteFile {
        base: ffi::sqlite3_file {
            pMethods: &PORTABLE_IO_METHODS,
        },
        file: ManuallyDrop::new(file),
    };
    // SAFETY: the registered szOsFile equals PortableSqliteFile's size and
    // SQLite provides suitably aligned storage for a sqlite3_file subclass.
    unsafe { ptr::write(output.cast::<PortableSqliteFile>(), sqlite_file) };
    if !output_flags.is_null() {
        // SAFETY: SQLite supplies writable pOutFlags when it requests them.
        unsafe { *output_flags = ffi::SQLITE_OPEN_READONLY | ffi::SQLITE_OPEN_MAIN_DB };
    }
    ffi::SQLITE_OK
}

unsafe extern "C" fn portable_vfs_delete(
    _vfs: *mut ffi::sqlite3_vfs,
    _name: *const c_char,
    _sync_directory: c_int,
) -> c_int {
    ffi::SQLITE_READONLY
}

unsafe extern "C" fn portable_vfs_access(
    _vfs: *mut ffi::sqlite3_vfs,
    name: *const c_char,
    flags: c_int,
    output: *mut c_int,
) -> c_int {
    ffi_boundary(ffi::SQLITE_IOERR, || {
        if output.is_null() {
            return ffi::SQLITE_IOERR;
        }
        if name.is_null() || flags == ffi::SQLITE_ACCESS_READWRITE {
            // SAFETY: output was checked non-null.
            unsafe { *output = 0 };
            return ffi::SQLITE_OK;
        }
        // SAFETY: SQLite documents zName as a NUL-terminated filename.
        let token = unsafe { CStr::from_ptr(name) }.to_bytes();
        let Ok(registry) = catalog_registry().lock() else {
            return ffi::SQLITE_IOERR;
        };
        // SAFETY: output was checked non-null.
        unsafe { *output = i32::from(registry.contains_key(token)) };
        ffi::SQLITE_OK
    })
}

unsafe extern "C" fn portable_vfs_full_pathname(
    _vfs: *mut ffi::sqlite3_vfs,
    name: *const c_char,
    output_size: c_int,
    output: *mut c_char,
) -> c_int {
    ffi_boundary(ffi::SQLITE_CANTOPEN, || {
        if name.is_null() || output.is_null() || output_size <= 0 {
            return ffi::SQLITE_CANTOPEN;
        }
        // SAFETY: SQLite documents zName as a NUL-terminated filename.
        let name = unsafe { CStr::from_ptr(name) }.to_bytes_with_nul();
        let Ok(output_size) = usize::try_from(output_size) else {
            return ffi::SQLITE_CANTOPEN;
        };
        if name.len() > output_size {
            return ffi::SQLITE_CANTOPEN;
        }
        // SAFETY: SQLite supplies output_size writable bytes and the bounds
        // above include the terminating NUL.
        unsafe { ptr::copy_nonoverlapping(name.as_ptr().cast(), output, name.len()) };
        ffi::SQLITE_OK
    })
}

unsafe extern "C" fn portable_file_close(file: *mut ffi::sqlite3_file) -> c_int {
    ffi_boundary(ffi::SQLITE_IOERR_CLOSE, || {
        if file.is_null() {
            return ffi::SQLITE_IOERR_CLOSE;
        }
        // SAFETY: only portable_vfs_open installs PORTABLE_IO_METHODS and it
        // initializes the complete PortableSqliteFile value before returning.
        let file = unsafe { &mut *file.cast::<PortableSqliteFile>() };
        // SAFETY: xClose runs exactly once for a successfully opened file.
        unsafe { ManuallyDrop::drop(&mut file.file) };
        file.base.pMethods = ptr::null();
        ffi::SQLITE_OK
    })
}

unsafe extern "C" fn portable_file_read(
    file: *mut ffi::sqlite3_file,
    output: *mut c_void,
    amount: c_int,
    offset: ffi::sqlite3_int64,
) -> c_int {
    ffi_boundary(ffi::SQLITE_IOERR_READ, || {
        if file.is_null() || output.is_null() || amount < 0 || offset < 0 {
            return ffi::SQLITE_IOERR_READ;
        }
        let Ok(amount) = usize::try_from(amount) else {
            return ffi::SQLITE_IOERR_READ;
        };
        let Ok(offset) = u64::try_from(offset) else {
            return ffi::SQLITE_IOERR_READ;
        };
        // SAFETY: SQLite supplies amount writable bytes for xRead.
        let output = unsafe { slice::from_raw_parts_mut(output.cast::<u8>(), amount) };
        // SAFETY: only portable_vfs_open installs these methods.
        let file = unsafe { &*file.cast::<PortableSqliteFile>() };
        match file.file.read(offset, output) {
            Ok(false) => ffi::SQLITE_OK,
            Ok(true) => ffi::SQLITE_IOERR_SHORT_READ,
            Err(()) => ffi::SQLITE_IOERR_DATA,
        }
    })
}

unsafe extern "C" fn portable_file_write(
    _file: *mut ffi::sqlite3_file,
    _input: *const c_void,
    _amount: c_int,
    _offset: ffi::sqlite3_int64,
) -> c_int {
    ffi::SQLITE_READONLY
}

unsafe extern "C" fn portable_file_truncate(
    _file: *mut ffi::sqlite3_file,
    _size: ffi::sqlite3_int64,
) -> c_int {
    ffi::SQLITE_READONLY
}

unsafe extern "C" fn portable_file_sync(_file: *mut ffi::sqlite3_file, _flags: c_int) -> c_int {
    ffi::SQLITE_READONLY
}

unsafe extern "C" fn portable_file_size(
    file: *mut ffi::sqlite3_file,
    output: *mut ffi::sqlite3_int64,
) -> c_int {
    ffi_boundary(ffi::SQLITE_IOERR_FSTAT, || {
        if file.is_null() || output.is_null() {
            return ffi::SQLITE_IOERR_FSTAT;
        }
        // SAFETY: only portable_vfs_open installs these methods.
        let file = unsafe { &*file.cast::<PortableSqliteFile>() };
        let Ok(size) = i64::try_from(file.file.catalog_size()) else {
            return ffi::SQLITE_TOOBIG;
        };
        // SAFETY: SQLite supplies writable storage for xFileSize.
        unsafe { *output = size };
        ffi::SQLITE_OK
    })
}

unsafe extern "C" fn portable_file_lock(_file: *mut ffi::sqlite3_file, _lock: c_int) -> c_int {
    ffi::SQLITE_OK
}

unsafe extern "C" fn portable_file_unlock(_file: *mut ffi::sqlite3_file, _lock: c_int) -> c_int {
    ffi::SQLITE_OK
}

unsafe extern "C" fn portable_file_reserved_lock(
    _file: *mut ffi::sqlite3_file,
    output: *mut c_int,
) -> c_int {
    if output.is_null() {
        return ffi::SQLITE_IOERR;
    }
    // SAFETY: output was checked non-null.
    unsafe { *output = 0 };
    ffi::SQLITE_OK
}

unsafe extern "C" fn portable_file_control(
    _file: *mut ffi::sqlite3_file,
    _operation: c_int,
    _argument: *mut c_void,
) -> c_int {
    ffi::SQLITE_NOTFOUND
}

unsafe extern "C" fn portable_file_sector_size(_file: *mut ffi::sqlite3_file) -> c_int {
    4096
}

unsafe extern "C" fn portable_file_characteristics(_file: *mut ffi::sqlite3_file) -> c_int {
    ffi::SQLITE_IOCAP_IMMUTABLE
}

unsafe extern "C" fn portable_vfs_randomness(
    vfs: *mut ffi::sqlite3_vfs,
    amount: c_int,
    output: *mut c_char,
) -> c_int {
    ffi_boundary(0, || {
        // SAFETY: callback receives the registered VFS pointer.
        let lower = unsafe { lower_vfs(vfs) };
        // SAFETY: the default VFS remains registered for process lifetime.
        unsafe {
            (*lower)
                .xRandomness
                .map_or(0, |callback| callback(lower, amount, output))
        }
    })
}

unsafe extern "C" fn portable_vfs_sleep(vfs: *mut ffi::sqlite3_vfs, micros: c_int) -> c_int {
    ffi_boundary(0, || {
        // SAFETY: callback receives the registered VFS pointer.
        let lower = unsafe { lower_vfs(vfs) };
        // SAFETY: the default VFS remains registered for process lifetime.
        unsafe {
            (*lower)
                .xSleep
                .map_or(0, |callback| callback(lower, micros))
        }
    })
}

unsafe extern "C" fn portable_vfs_current_time(
    vfs: *mut ffi::sqlite3_vfs,
    output: *mut f64,
) -> c_int {
    ffi_boundary(ffi::SQLITE_ERROR, || {
        // SAFETY: callback receives the registered VFS pointer.
        let lower = unsafe { lower_vfs(vfs) };
        // SAFETY: the default VFS remains registered for process lifetime.
        unsafe {
            (*lower)
                .xCurrentTime
                .map_or(ffi::SQLITE_ERROR, |callback| callback(lower, output))
        }
    })
}

unsafe extern "C" fn portable_vfs_last_error(
    vfs: *mut ffi::sqlite3_vfs,
    amount: c_int,
    output: *mut c_char,
) -> c_int {
    ffi_boundary(0, || {
        // SAFETY: callback receives the registered VFS pointer.
        let lower = unsafe { lower_vfs(vfs) };
        // SAFETY: the default VFS remains registered for process lifetime.
        unsafe {
            (*lower)
                .xGetLastError
                .map_or(0, |callback| callback(lower, amount, output))
        }
    })
}

unsafe fn lower_vfs(vfs: *mut ffi::sqlite3_vfs) -> *mut ffi::sqlite3_vfs {
    if vfs.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: vfs is the registered object and pAppData is the default VFS.
    unsafe { (*vfs).pAppData.cast() }
}

fn ffi_boundary(fallback: c_int, callback: impl FnOnce() -> c_int) -> c_int {
    catch_unwind(AssertUnwindSafe(callback)).unwrap_or(fallback)
}

#[cfg(test)]
#[path = "portable_vfs/tests.rs"]
mod tests;
