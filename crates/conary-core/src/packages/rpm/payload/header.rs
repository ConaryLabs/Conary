// conary-core/src/packages/rpm/payload/header.rs

//! Exact projection of RPM's parallel file-header arrays.
//!
//! The `rpm` crate currently treats every unknown `FILEDIGESTALGO` value as
//! MD5 while constructing file entries. That loses RPM's SHA-1 value (`2`), so
//! this module reads the source-native tag directly and keeps the algorithm
//! typed through payload verification.

use crate::error::Result;
use crate::packages::archive_utils::normalize_path;
use rpm::{FileFlags, IndexSignatureTag, IndexTag, Package};

use super::{exact_identity, exact_text, parse_error};

#[derive(Debug, Clone)]
pub(super) struct HeaderRecord {
    pub(super) path: String,
    pub(super) path_kind: HeaderPathKind,
    pub(super) mode: u32,
    pub(super) user: String,
    pub(super) group: String,
    pub(super) mtime: u32,
    pub(super) size: u64,
    pub(super) ghost: bool,
    pub(super) digest: Option<DeclaredDigest>,
    pub(super) link_target: Option<String>,
    pub(super) caps: Option<String>,
    pub(super) ima_signature: Option<String>,
    pub(super) device: u32,
    pub(super) inode: u32,
    pub(super) rdev: u16,
    pub(super) nlink: Option<u32>,
}

impl HeaderRecord {
    pub(super) fn is_root_anchor(&self) -> bool {
        self.path_kind == HeaderPathKind::RootAnchor
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HeaderPathKind {
    Deployable,
    RootAnchor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DeclaredDigest {
    pub(super) algorithm: RpmFileDigestAlgorithm,
    pub(super) hex: String,
}

/// RPM's exact supported file-digest algorithm table at the pinned upstream
/// source revision.
///
/// <https://github.com/rpm-software-management/rpm/blob/a8f0192aee1c08bd1454ed2ac6ebaf506004b55c/rpmio/digest_openssl.cc>
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RpmFileDigestAlgorithm {
    Md5,
    Sha1,
    Sha2_224,
    Sha2_256,
    Sha2_384,
    Sha2_512,
    Sha3_256,
    Sha3_512,
}

impl RpmFileDigestAlgorithm {
    fn from_rpm_code(code: u32) -> Result<Self> {
        match code {
            1 => Ok(Self::Md5),
            2 => Ok(Self::Sha1),
            8 => Ok(Self::Sha2_256),
            9 => Ok(Self::Sha2_384),
            10 => Ok(Self::Sha2_512),
            11 => Ok(Self::Sha2_224),
            12 => Ok(Self::Sha3_256),
            14 => Ok(Self::Sha3_512),
            _ => Err(parse_error(format!(
                "RPM FILEDIGESTALGO value {code} is not implemented by the pinned RPM digest ABI"
            ))),
        }
    }

    const fn hex_len(self) -> usize {
        match self {
            Self::Md5 => 32,
            Self::Sha1 => 40,
            Self::Sha2_224 => 56,
            Self::Sha2_256 | Self::Sha3_256 => 64,
            Self::Sha2_384 => 96,
            Self::Sha2_512 | Self::Sha3_512 => 128,
        }
    }
}

pub(super) fn header_records(package: &Package) -> Result<Vec<HeaderRecord>> {
    let modes = match package
        .metadata
        .header
        .get_entry_data_as_u16_array(IndexTag::RPMTAG_FILEMODES)
    {
        Ok(modes) => modes,
        Err(rpm::Error::TagNotFound(_)) => return Ok(Vec::new()),
        Err(error) => return Err(read_error(IndexTag::RPMTAG_FILEMODES, error)),
    };
    let count = modes.len();
    let users = required_string_array(package, IndexTag::RPMTAG_FILEUSERNAME, count)?;
    let groups = required_string_array(package, IndexTag::RPMTAG_FILEGROUPNAME, count)?;
    let digests = required_string_array(package, IndexTag::RPMTAG_FILEDIGESTS, count)?;
    let mtimes = required_u32_array(package, IndexTag::RPMTAG_FILEMTIMES, count)?;
    let sizes = file_sizes(package, count)?;
    let flags = required_u32_array(package, IndexTag::RPMTAG_FILEFLAGS, count)?;
    let links = required_string_array(package, IndexTag::RPMTAG_FILELINKTOS, count)?;
    let basenames = required_string_array(package, IndexTag::RPMTAG_BASENAMES, count)?;
    let directory_indexes = required_u32_array(package, IndexTag::RPMTAG_DIRINDEXES, count)?;
    let directories = package
        .metadata
        .header
        .get_entry_data_as_string_array(IndexTag::RPMTAG_DIRNAMES)
        .map_err(|error| read_error(IndexTag::RPMTAG_DIRNAMES, error))?;
    let caps = optional_string_array(package, IndexTag::RPMTAG_FILECAPS, count)?;
    let ima_signatures =
        optional_signature_array(package, IndexSignatureTag::RPMSIGTAG_FILESIGNATURES, count)?;
    let devices = required_u32_array(package, IndexTag::RPMTAG_FILEDEVICES, count)?;
    let inodes = required_u32_array(package, IndexTag::RPMTAG_FILEINODES, count)?;
    let rdevs = required_u16_array(package, IndexTag::RPMTAG_FILERDEVS, count)?;
    let nlinks = optional_u32_array(package, IndexTag::RPMTAG_FILENLINKS, count)?;
    let digest_algorithm = file_digest_algorithm(package)?;

    let mut records = Vec::with_capacity(count);
    let mut seen_paths = std::collections::HashSet::with_capacity(count);
    for index in 0..count {
        let directory_index = directory_indexes[index] as usize;
        let directory = directories.get(directory_index).ok_or_else(|| {
            parse_error(format!(
                "RPM DIRINDEXES value {} at file index {index} exceeds {} directories",
                directory_indexes[index],
                directories.len()
            ))
        })?;
        let (path, path_kind) = rpm_header_path(directory, basenames[index])?;
        if !seen_paths.insert(path.clone()) {
            return Err(parse_error(format!("duplicate RPM header path {path}")));
        }

        let file_flags = FileFlags::from_bits_retain(flags[index]);
        let record = HeaderRecord {
            path: path.clone(),
            path_kind,
            mode: u32::from(modes[index]),
            user: exact_identity(users[index], "user", &path)?,
            group: exact_identity(groups[index], "group", &path)?,
            mtime: mtimes[index],
            size: sizes[index],
            ghost: file_flags.contains(FileFlags::GHOST),
            digest: declared_digest(digest_algorithm, digests[index], &path)?,
            link_target: (!links[index].is_empty())
                .then(|| exact_text(links[index], "symlink target", &path))
                .transpose()?,
            caps: caps
                .as_ref()
                .map(|values| values[index])
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            ima_signature: ima_signatures
                .as_ref()
                .map(|values| values[index])
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            device: devices[index],
            inode: inodes[index],
            rdev: rdevs[index],
            nlink: nlinks.as_ref().map(|values| values[index]),
        };
        validate_root_anchor(&record, file_flags)?;
        records.push(record);
    }
    Ok(records)
}

/// RPM represents the filesystem root as `DIRNAMES="/"` plus an empty
/// `BASENAMES` value. The upstream filesystem state machine handles that empty
/// basename explicitly rather than treating `/` as an ordinary payload path:
/// <https://github.com/rpm-software-management/rpm/blob/a8f0192aee1c08bd1454ed2ac6ebaf506004b55c/lib/fsm.cc#L71-L82>.
///
/// Keep that source-format ownership anchor distinct from a path Conary may
/// deploy below a selected root. All other entries still pass through the
/// shared deployment-path authority.
fn rpm_header_path(directory: &str, basename: &str) -> Result<(String, HeaderPathKind)> {
    if directory == "/" && basename.is_empty() {
        return Ok(("/".to_string(), HeaderPathKind::RootAnchor));
    }

    // RPM reconstructs the filename by concatenating its directory and
    // basename tables. Do not import host Path joining semantics here.
    let raw_path = format!("{directory}{basename}");
    let path = normalize_path(&raw_path)
        .map_err(|error| parse_error(format!("invalid RPM header path: {error}")))?;
    if raw_path != path {
        return Err(parse_error(format!(
            "RPM header path {raw_path:?} is not canonical absolute form {path:?}"
        )));
    }
    Ok((path, HeaderPathKind::Deployable))
}

fn validate_root_anchor(record: &HeaderRecord, flags: FileFlags) -> Result<()> {
    if !record.is_root_anchor() {
        if record.path == "/" {
            return Err(parse_error(
                "RPM filesystem root cannot be classified as a deployable path",
            ));
        }
        return Ok(());
    }
    if record.path != "/" {
        return Err(parse_error(
            "RPM root ownership classification requires the exact / header path",
        ));
    }
    if record.mode & libc::S_IFMT != libc::S_IFDIR {
        return Err(parse_error("RPM root ownership anchor must be a directory"));
    }
    if !flags.is_empty() {
        return Err(parse_error(format!(
            "RPM root ownership anchor carries unsupported file flags {:#x}",
            flags.bits()
        )));
    }
    if record.digest.is_some()
        || record.link_target.is_some()
        || record.caps.is_some()
        || record.ima_signature.is_some()
        || record.size != 0
        || record.rdev != 0
    {
        return Err(parse_error(
            "RPM root ownership anchor carries non-directory payload metadata",
        ));
    }
    Ok(())
}

fn file_digest_algorithm(package: &Package) -> Result<RpmFileDigestAlgorithm> {
    match package
        .metadata
        .header
        .get_entry_data_as_u32(IndexTag::RPMTAG_FILEDIGESTALGO)
    {
        Ok(code) => RpmFileDigestAlgorithm::from_rpm_code(code),
        // RPM omits the tag for its historical MD5 default.
        Err(rpm::Error::TagNotFound(_)) => Ok(RpmFileDigestAlgorithm::Md5),
        Err(error) => Err(read_error(IndexTag::RPMTAG_FILEDIGESTALGO, error)),
    }
}

fn declared_digest(
    algorithm: RpmFileDigestAlgorithm,
    hex: &str,
    path: &str,
) -> Result<Option<DeclaredDigest>> {
    if hex.is_empty() {
        return Ok(None);
    }
    if hex.len() != algorithm.hex_len() || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(parse_error(format!(
            "RPM file digest for {path} is not {} hexadecimal characters for {algorithm:?}",
            algorithm.hex_len()
        )));
    }
    // RPM accepts either hex case when decoding and compares the binary
    // digest. Canonicalize so comparison has identical semantics.
    Ok(Some(DeclaredDigest {
        algorithm,
        hex: hex.to_ascii_lowercase(),
    }))
}

fn file_sizes(package: &Package, count: usize) -> Result<Vec<u64>> {
    match package
        .metadata
        .header
        .get_entry_data_as_u64_array(IndexTag::RPMTAG_LONGFILESIZES)
    {
        Ok(values) => require_array_len(IndexTag::RPMTAG_LONGFILESIZES, values, count),
        Err(rpm::Error::TagNotFound(_)) => {
            required_u32_array(package, IndexTag::RPMTAG_FILESIZES, count)
                .map(|values| values.into_iter().map(u64::from).collect())
        }
        Err(error) => Err(read_error(IndexTag::RPMTAG_LONGFILESIZES, error)),
    }
}

fn required_string_array(package: &Package, tag: IndexTag, count: usize) -> Result<Vec<&str>> {
    let values = package
        .metadata
        .header
        .get_entry_data_as_string_array(tag)
        .map_err(|error| read_error(tag, error))?;
    require_array_len(tag, values, count)
}

fn optional_string_array(
    package: &Package,
    tag: IndexTag,
    count: usize,
) -> Result<Option<Vec<&str>>> {
    match package.metadata.header.get_entry_data_as_string_array(tag) {
        Ok(values) => require_array_len(tag, values, count).map(Some),
        Err(rpm::Error::TagNotFound(_)) => Ok(None),
        Err(error) => Err(read_error(tag, error)),
    }
}

fn optional_signature_array(
    package: &Package,
    tag: IndexSignatureTag,
    count: usize,
) -> Result<Option<Vec<&str>>> {
    match package
        .metadata
        .signature
        .get_entry_data_as_string_array(tag)
    {
        Ok(values) if values.len() == count => Ok(Some(values)),
        Ok(values) => Err(parse_error(format!(
            "RPM {tag} contains {} values for {count} files",
            values.len()
        ))),
        Err(rpm::Error::TagNotFound(_)) => Ok(None),
        Err(error) => Err(parse_error(format!("read RPM {tag}: {error}"))),
    }
}

fn required_u32_array(package: &Package, tag: IndexTag, count: usize) -> Result<Vec<u32>> {
    let values = package
        .metadata
        .header
        .get_entry_data_as_u32_array(tag)
        .map_err(|error| read_error(tag, error))?;
    require_array_len(tag, values, count)
}

fn required_u16_array(package: &Package, tag: IndexTag, count: usize) -> Result<Vec<u16>> {
    let values = package
        .metadata
        .header
        .get_entry_data_as_u16_array(tag)
        .map_err(|error| read_error(tag, error))?;
    require_array_len(tag, values, count)
}

fn optional_u32_array(package: &Package, tag: IndexTag, count: usize) -> Result<Option<Vec<u32>>> {
    match package.metadata.header.get_entry_data_as_u32_array(tag) {
        Ok(values) => require_array_len(tag, values, count).map(Some),
        Err(rpm::Error::TagNotFound(_)) => Ok(None),
        Err(error) => Err(read_error(tag, error)),
    }
}

fn require_array_len<T>(tag: IndexTag, values: Vec<T>, count: usize) -> Result<Vec<T>> {
    if values.len() != count {
        return Err(parse_error(format!(
            "RPM {tag} contains {} values for {count} files",
            values.len()
        )));
    }
    Ok(values)
}

fn read_error(tag: IndexTag, error: rpm::Error) -> crate::error::Error {
    parse_error(format!("read RPM {tag}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_digest_algorithm_codes_are_exact() {
        for (code, expected) in [
            (1, RpmFileDigestAlgorithm::Md5),
            (2, RpmFileDigestAlgorithm::Sha1),
            (8, RpmFileDigestAlgorithm::Sha2_256),
            (9, RpmFileDigestAlgorithm::Sha2_384),
            (10, RpmFileDigestAlgorithm::Sha2_512),
            (11, RpmFileDigestAlgorithm::Sha2_224),
            (12, RpmFileDigestAlgorithm::Sha3_256),
            (14, RpmFileDigestAlgorithm::Sha3_512),
        ] {
            assert_eq!(
                RpmFileDigestAlgorithm::from_rpm_code(code).unwrap(),
                expected
            );
        }
        assert!(RpmFileDigestAlgorithm::from_rpm_code(99).is_err());
    }

    #[test]
    fn digest_text_matches_rpm_binary_hex_semantics() {
        let digest = declared_digest(
            RpmFileDigestAlgorithm::Sha1,
            "A9993E364706816ABA3E25717850C26C9CD0D89D",
            "/fixture",
        )
        .unwrap()
        .unwrap();
        assert_eq!(digest.hex, "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    fn root_record() -> HeaderRecord {
        HeaderRecord {
            path: "/".to_string(),
            path_kind: HeaderPathKind::RootAnchor,
            mode: libc::S_IFDIR | 0o555,
            user: "root".to_string(),
            group: "root".to_string(),
            mtime: 1,
            size: 0,
            ghost: false,
            digest: None,
            link_target: None,
            caps: None,
            ima_signature: None,
            device: 1,
            inode: 1,
            rdev: 0,
            nlink: Some(1),
        }
    }

    #[test]
    fn exact_rpm_root_triplet_is_a_source_ownership_anchor() {
        assert_eq!(
            rpm_header_path("/", "").unwrap(),
            ("/".to_string(), HeaderPathKind::RootAnchor)
        );
        assert!(rpm_header_path("", "/").is_err());
        assert!(rpm_header_path("/", ".").is_err());
        assert!(rpm_header_path("//", "usr").is_err());
        assert_eq!(
            rpm_header_path("/", "usr").unwrap(),
            ("/usr".to_string(), HeaderPathKind::Deployable)
        );
        assert_eq!(
            rpm_header_path("/usr/share/", "fixture").unwrap(),
            ("/usr/share/fixture".to_string(), HeaderPathKind::Deployable)
        );
    }

    #[test]
    fn root_anchor_rejects_mutating_or_ambiguous_metadata() {
        let record = root_record();
        validate_root_anchor(&record, FileFlags::empty()).unwrap();

        let mut deployable_root = record.clone();
        deployable_root.path_kind = HeaderPathKind::Deployable;
        assert!(
            validate_root_anchor(&deployable_root, FileFlags::empty())
                .unwrap_err()
                .to_string()
                .contains("cannot be classified as a deployable path")
        );

        let mut regular = record.clone();
        regular.mode = libc::S_IFREG | 0o555;
        assert!(
            validate_root_anchor(&regular, FileFlags::empty())
                .unwrap_err()
                .to_string()
                .contains("must be a directory")
        );

        assert!(
            validate_root_anchor(&record, FileFlags::GHOST)
                .unwrap_err()
                .to_string()
                .contains("unsupported file flags")
        );

        let mut linked = record;
        linked.link_target = Some("usr".to_string());
        assert!(
            validate_root_anchor(&linked, FileFlags::empty())
                .unwrap_err()
                .to_string()
                .contains("non-directory payload metadata")
        );

        let mut sized = root_record();
        sized.size = 1;
        assert!(
            validate_root_anchor(&sized, FileFlags::empty())
                .unwrap_err()
                .to_string()
                .contains("non-directory payload metadata")
        );
    }
}
