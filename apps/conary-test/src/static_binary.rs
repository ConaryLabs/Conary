// conary-test/src/static_binary.rs

//! The statically linked `conary` artifact staged into foreign userlands.
//!
//! A distro test image is a frozen userland that rarely matches the build
//! host. Staging the host-built `conary` couples the image to the host's
//! glibc and to a matching `libseccomp.so.2`, which is why the Ubuntu image
//! used to compile Conary from source inside the container instead. A static
//! musl build removes both couplings, so one artifact runs in every image.
//!
//! `scripts/build-static-conary.sh` owns producing that artifact. This module
//! owns finding it and refusing anything that is not actually static.

use anyhow::{Context, Result, bail};
use std::ffi::OsString;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Target triple `scripts/build-static-conary.sh` builds.
pub(crate) const STATIC_TARGET_TRIPLE: &str = "x86_64-unknown-linux-musl";

/// Cargo profile directory `scripts/build-static-conary.sh` builds into.
pub(crate) const STATIC_PROFILE_DIR: &str = "debug";

/// Builder for the static artifact, relative to the workspace root.
pub(crate) const STATIC_BUILD_SCRIPT: &str = "scripts/build-static-conary.sh";

const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];
const ELF_CLASS_64: u8 = 2;
const ELF_DATA_LSB: u8 = 1;
const ELF_HEADER_LEN: usize = 64;
const E_PHOFF_OFFSET: usize = 0x20;
const E_PHENTSIZE_OFFSET: usize = 0x36;
const E_PHNUM_OFFSET: usize = 0x38;
const PT_INTERP: u32 = 3;
const PN_XNUM: u16 = 0xffff;

/// Resolve the static `conary` artifact under `target_dir`.
///
/// Fails closed: a missing artifact names the builder rather than falling back
/// to the host binary, because a silent fallback reintroduces exactly the
/// glibc coupling this staging path exists to remove.
pub(crate) fn static_conary_binary_in(target_dir: &Path) -> Result<PathBuf> {
    let binary = static_conary_binary_path_in(target_dir);
    if !binary.is_file() {
        bail!(
            "static conary artifact not found at {}\n\
             Build it with: bash {}",
            binary.display(),
            STATIC_BUILD_SCRIPT
        );
    }
    ensure_statically_linked(&binary)?;
    Ok(binary)
}

/// Where `scripts/build-static-conary.sh` leaves the artifact.
pub(crate) fn static_conary_binary_path_in(target_dir: &Path) -> PathBuf {
    target_dir
        .join(STATIC_TARGET_TRIPLE)
        .join(STATIC_PROFILE_DIR)
        .join("conary")
}

/// The Cargo target directory the build script writes into.
///
/// The script resolves it as `${CARGO_TARGET_DIR:-$repo_root/target}`; keep
/// both in agreement, or staging looks in a directory nothing builds into.
pub(crate) fn static_target_dir(project_root: &Path) -> PathBuf {
    target_dir_from(project_root, std::env::var_os("CARGO_TARGET_DIR"))
}

fn target_dir_from(project_root: &Path, cargo_target_dir: Option<OsString>) -> PathBuf {
    match cargo_target_dir {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => project_root.join("target"),
    }
}

/// Reject any ELF that asks a dynamic loader for its libraries.
///
/// A fully static binary carries no `PT_INTERP` segment, whether it is linked
/// `ET_EXEC` or as a static PIE. Checking the program headers directly proves
/// the property that matters inside a foreign userland, without shelling out
/// to `file` or `ldd`.
fn ensure_statically_linked(path: &Path) -> Result<()> {
    let mut file =
        File::open(path).with_context(|| format!("failed to open {}", path.display()))?;

    let mut header = [0u8; ELF_HEADER_LEN];
    file.read_exact(&mut header)
        .with_context(|| format!("{} is too small to be a 64-bit ELF binary", path.display()))?;

    if header[..4] != ELF_MAGIC {
        bail!("{} is not an ELF binary", path.display());
    }
    if header[4] != ELF_CLASS_64 || header[5] != ELF_DATA_LSB {
        bail!(
            "{} is not a little-endian 64-bit ELF binary",
            path.display()
        );
    }

    let e_phoff = u64::from_le_bytes(
        header[E_PHOFF_OFFSET..E_PHOFF_OFFSET + 8]
            .try_into()
            .expect("8 bytes"),
    );
    let e_phentsize = u16::from_le_bytes(
        header[E_PHENTSIZE_OFFSET..E_PHENTSIZE_OFFSET + 2]
            .try_into()
            .expect("2 bytes"),
    );
    let e_phnum = u16::from_le_bytes(
        header[E_PHNUM_OFFSET..E_PHNUM_OFFSET + 2]
            .try_into()
            .expect("2 bytes"),
    );

    if e_phnum == PN_XNUM {
        bail!(
            "{} stores its program header count in the section table (PN_XNUM); \
             static linkage cannot be proven",
            path.display()
        );
    }
    if e_phnum == 0 || u64::from(e_phentsize) < 4 {
        bail!(
            "{} has no usable program headers; static linkage cannot be proven",
            path.display()
        );
    }

    for index in 0..u64::from(e_phnum) {
        let offset = e_phoff + index * u64::from(e_phentsize);
        file.seek(SeekFrom::Start(offset)).with_context(|| {
            format!(
                "failed to seek to program header {index} of {}",
                path.display()
            )
        })?;
        let mut p_type = [0u8; 4];
        file.read_exact(&mut p_type).with_context(|| {
            format!(
                "failed to read program header {index} of {}",
                path.display()
            )
        })?;
        if u32::from_le_bytes(p_type) == PT_INTERP {
            bail!(
                "{} is dynamically linked (it requests a program interpreter)\n\
                 A dynamically linked binary reintroduces the host glibc coupling that \
                 static staging exists to remove; rebuild it with: bash {}",
                path.display(),
                STATIC_BUILD_SCRIPT
            );
        }
    }

    Ok(())
}

/// Minimal ELF64 image carrying program headers of the given types.
///
/// Shared with the container staging tests so they can exercise the artifact
/// contract without a real musl build.
#[cfg(test)]
pub(crate) fn synthetic_elf(program_header_types: &[u32]) -> Vec<u8> {
    let entry_size: u16 = 56;
    let mut image = vec![0u8; ELF_HEADER_LEN];
    image[..4].copy_from_slice(&ELF_MAGIC);
    image[4] = ELF_CLASS_64;
    image[5] = ELF_DATA_LSB;
    image[E_PHOFF_OFFSET..E_PHOFF_OFFSET + 8]
        .copy_from_slice(&(ELF_HEADER_LEN as u64).to_le_bytes());
    image[E_PHENTSIZE_OFFSET..E_PHENTSIZE_OFFSET + 2].copy_from_slice(&entry_size.to_le_bytes());
    image[E_PHNUM_OFFSET..E_PHNUM_OFFSET + 2]
        .copy_from_slice(&(program_header_types.len() as u16).to_le_bytes());

    for p_type in program_header_types {
        let mut entry = vec![0u8; entry_size as usize];
        entry[..4].copy_from_slice(&p_type.to_le_bytes());
        image.extend_from_slice(&entry);
    }
    image
}

/// `PT_LOAD`, the program header type a static binary does carry.
#[cfg(test)]
pub(crate) const PT_LOAD: u32 = 1;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn workspace_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("canonicalize workspace root")
    }

    /// The script and this module both encode where the artifact lands. Prove
    /// they agree instead of discovering a rename at image-build time.
    #[test]
    fn static_artifact_path_matches_the_build_script_convention() {
        let script = workspace_root().join(STATIC_BUILD_SCRIPT);
        let contents = fs::read_to_string(&script).expect("read static build script");

        assert!(
            contents.contains(&format!("TARGET=\"{STATIC_TARGET_TRIPLE}\"")),
            "{} must build {STATIC_TARGET_TRIPLE}",
            script.display()
        );
        assert!(
            contents.contains("target_dir=\"${CARGO_TARGET_DIR:-$repo_root/target}\""),
            "{} must resolve its target directory the way this module does",
            script.display()
        );
        assert!(
            contents.contains(&format!(
                "binary=\"$target_dir/$TARGET/{STATIC_PROFILE_DIR}/conary\""
            )),
            "{} must leave the artifact in the {STATIC_PROFILE_DIR} profile directory",
            script.display()
        );
    }

    #[test]
    fn static_artifact_path_follows_the_target_directory() {
        let target_dir = Path::new("/workspace/conary/target");
        assert_eq!(
            static_conary_binary_path_in(target_dir),
            target_dir
                .join(STATIC_TARGET_TRIPLE)
                .join(STATIC_PROFILE_DIR)
                .join("conary")
        );
    }

    #[test]
    fn target_directory_honours_the_cargo_override() {
        let project_root = Path::new("/workspace/conary");
        assert_eq!(
            target_dir_from(project_root, None),
            project_root.join("target"),
            "an unset override must use the workspace target directory"
        );
        assert_eq!(
            target_dir_from(project_root, Some(OsString::from("/shared/target"))),
            Path::new("/shared/target"),
            "CARGO_TARGET_DIR must win, as it does for the build script"
        );
        assert_eq!(
            target_dir_from(project_root, Some(OsString::new())),
            project_root.join("target"),
            "an empty override is not a target directory"
        );
    }

    #[test]
    fn missing_static_artifact_names_the_builder() {
        let directory = tempfile::tempdir().expect("create temp directory");
        let error = static_conary_binary_in(directory.path())
            .expect_err("a missing static artifact must fail closed");
        let message = error.to_string();

        assert!(
            message.contains("static conary artifact not found"),
            "unexpected message: {message}"
        );
        assert!(
            message.contains(STATIC_BUILD_SCRIPT),
            "the failure must name the builder: {message}"
        );
    }

    #[test]
    fn dynamically_linked_artifact_is_rejected() {
        let directory = tempfile::tempdir().expect("create temp directory");
        let binary = directory.path().join("conary");
        fs::write(&binary, synthetic_elf(&[PT_LOAD, PT_INTERP, 2])).expect("write dynamic elf");

        let error =
            ensure_statically_linked(&binary).expect_err("a PT_INTERP binary must be rejected");
        assert!(
            error.to_string().contains("dynamically linked"),
            "unexpected message: {error}"
        );
    }

    #[test]
    fn statically_linked_artifact_is_accepted() {
        let directory = tempfile::tempdir().expect("create temp directory");
        let binary = directory.path().join("conary");
        fs::write(&binary, synthetic_elf(&[PT_LOAD, 6, 2])).expect("write static elf");

        ensure_statically_linked(&binary).expect("a binary without PT_INTERP must be accepted");
    }

    #[test]
    fn non_elf_artifact_is_rejected() {
        let directory = tempfile::tempdir().expect("create temp directory");
        let binary = directory.path().join("conary");
        fs::write(&binary, vec![b'#'; ELF_HEADER_LEN]).expect("write script");

        let error = ensure_statically_linked(&binary).expect_err("a non-ELF file must be rejected");
        assert!(
            error.to_string().contains("is not an ELF binary"),
            "unexpected message: {error}"
        );
    }
}
