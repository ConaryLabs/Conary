// conary-core/src/filesystem/source_path.rs

//! Byte-exact path authority for untrusted source-package archives.
//!
//! Archive paths are byte strings in a format-defined grammar. They are not
//! `std::path::Path` values, and asking `Path` to parse them imports host
//! filesystem conventions that the source format never promised. This module
//! keeps the two apart: [`SourcePathBytes`] preserves exactly what the archive
//! declared, and [`DeploymentPath`] is the validated relative form that may be
//! materialized under a root.
//!
//! Traversal safety is decided by exact component comparison on bytes, never by
//! how a path *looks*. The previous implementation rejected every non-ASCII
//! path as traversal, which is not a rule of ALPM, RPM, Debian, tar, or Linux.
//! It rejected valid repository packages — `aisleriot`, `aspell-nb`, `azure-cli`
//! on Arch, and equally any RPM or Debian package with a non-ASCII path, since
//! all three formats route through this authority. It also converted with
//! `to_string_lossy` before deciding, destroying the very bytes a rejection
//! would need to be diagnosed.
//!
//! # Scope boundary
//!
//! [`SourcePathBytes`] can hold any non-NUL byte sequence, but Conary persists
//! paths as `TEXT`/`String` (`ccs/v3/schema.rs`, `packages/mod.rs`, the `files`
//! table). Storing a non-UTF-8 path therefore requires a CCS schema and
//! database decision that this module deliberately does not make. Until that
//! decision exists, [`DeploymentPath::to_utf8`] is the persistence boundary and
//! returns a typed error for non-UTF-8 input rather than lossily converting it.
//! That keeps the failure explicit and attributable instead of silently
//! corrupting a path.

use crate::error::{Error, Result};
use std::path::{Path, PathBuf};

/// The path separator byte. Archive grammars define this exactly; it is not a
/// host filesystem convention and does not vary by platform.
const SEPARATOR: u8 = b'/';

const CURRENT_DIR: &[u8] = b".";
const PARENT_DIR: &[u8] = b"..";

/// A path exactly as an archive declared it.
///
/// Lossless: no normalization, no case folding, no Unicode normalization, no
/// lossy character replacement. Two paths that differ by a single byte remain
/// distinguishable, which matters because a package may legitimately ship paths
/// that are canonically equivalent under NFC/NFD but distinct as bytes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourcePathBytes(Vec<u8>);

impl SourcePathBytes {
    /// Record a declared path verbatim.
    pub fn new(bytes: impl Into<Vec<u8>>) -> Self {
        Self(bytes.into())
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Lossy rendering for diagnostics only.
    ///
    /// Never use this to make a decision. It exists so an error message can
    /// show something readable for a path that is not valid UTF-8.
    pub fn to_diagnostic_string(&self) -> String {
        String::from_utf8_lossy(&self.0).into_owned()
    }

    /// Validate for materialization under a root.
    ///
    /// Splits on the separator byte only, then judges each component by exact
    /// byte comparison:
    ///
    /// - a NUL byte anywhere is rejected, since it truncates the path at C API
    ///   boundaries and would make the materialized path differ from the
    ///   validated one
    /// - empty components, produced by leading, trailing, or repeated
    ///   separators, carry no meaning and are dropped
    /// - `.` is dropped
    /// - `..` is rejected outright rather than resolved, because resolving it
    ///   against a root that contains symlinks does not mean what it appears to
    /// - a path with no components left is rejected as undeployable
    ///
    /// Character class, script, and encoding are deliberately not consulted.
    pub fn to_deployment_path(&self) -> Result<DeploymentPath> {
        if self.0.contains(&0) {
            return Err(Error::PathTraversal(format!(
                "path contains a NUL byte: {}",
                self.to_diagnostic_string()
            )));
        }

        let mut components: Vec<Vec<u8>> = Vec::new();
        for component in self.0.split(|byte| *byte == SEPARATOR) {
            match component {
                b"" | CURRENT_DIR => {}
                PARENT_DIR => {
                    return Err(Error::PathTraversal(format!(
                        "path contains a parent-directory component: {}",
                        self.to_diagnostic_string()
                    )));
                }
                other => components.push(other.to_vec()),
            }
        }

        if components.is_empty() {
            return Err(Error::InvalidPath(format!(
                "path has no deployable components: {}",
                self.to_diagnostic_string()
            )));
        }

        Ok(DeploymentPath { components })
    }
}

impl From<&str> for SourcePathBytes {
    fn from(value: &str) -> Self {
        Self(value.as_bytes().to_vec())
    }
}

/// A validated, relative path safe to join beneath a root.
///
/// Holding one of these means traversal validation already happened. It cannot
/// be constructed except through [`SourcePathBytes::to_deployment_path`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeploymentPath {
    components: Vec<Vec<u8>>,
}

impl DeploymentPath {
    /// Components in order, each a non-empty byte string that is not `.` or `..`.
    pub fn components(&self) -> impl Iterator<Item = &[u8]> {
        self.components.iter().map(|component| component.as_slice())
    }

    pub fn component_count(&self) -> usize {
        self.components.len()
    }

    /// Rejoin with the separator byte, byte-exact.
    pub fn to_bytes(&self) -> Vec<u8> {
        self.components.join(&SEPARATOR)
    }

    /// The persistence boundary.
    ///
    /// Errors rather than converting lossily, because Conary persists paths as
    /// text and a lossy conversion here would write a path that does not match
    /// the artifact. See this module's scope note.
    pub fn to_utf8(&self) -> Result<String> {
        String::from_utf8(self.to_bytes()).map_err(|error| {
            Error::InvalidPath(format!(
                "path is not valid UTF-8 and cannot be persisted: {error}"
            ))
        })
    }

    /// Convert to a relative `PathBuf` for filesystem calls.
    ///
    /// Safe only after validation, which is why it lives on this type and not
    /// on [`SourcePathBytes`].
    #[cfg(unix)]
    pub fn to_path_buf(&self) -> PathBuf {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let mut path = PathBuf::new();
        for component in &self.components {
            path.push(OsString::from_vec(component.clone()));
        }
        path
    }

    /// Non-Unix hosts have no byte-exact path representation, so this requires
    /// UTF-8 rather than guessing an encoding.
    #[cfg(not(unix))]
    pub fn to_path_buf(&self) -> PathBuf {
        PathBuf::from(
            self.to_utf8()
                .unwrap_or_else(|_| String::from_utf8_lossy(&self.to_bytes()).into_owned()),
        )
    }

    /// Join beneath `root`. The result cannot escape by construction: every
    /// component is a plain name, and `..` was rejected at validation.
    pub fn join_under(&self, root: impl AsRef<Path>) -> PathBuf {
        root.as_ref().join(self.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deploy(input: &str) -> Result<DeploymentPath> {
        SourcePathBytes::from(input).to_deployment_path()
    }

    #[test]
    fn ordinary_paths_keep_their_components_in_order() {
        let path = deploy("usr/bin/foo").expect("valid path");

        assert_eq!(path.component_count(), 3);
        assert_eq!(path.to_bytes(), b"usr/bin/foo");
        assert_eq!(path.to_utf8().unwrap(), "usr/bin/foo");
    }

    #[test]
    fn leading_trailing_and_repeated_separators_are_dropped() {
        for input in ["/usr/bin/foo", "usr/bin/foo/", "//usr//bin//foo//"] {
            let path = deploy(input).unwrap_or_else(|_| panic!("{input} should be valid"));
            assert_eq!(path.to_bytes(), b"usr/bin/foo", "input was {input}");
        }
    }

    #[test]
    fn current_directory_components_are_dropped() {
        assert_eq!(
            deploy("./usr/./bin/foo").unwrap().to_bytes(),
            b"usr/bin/foo"
        );
    }

    // -- Traversal safety, decided on exact components ----------------------

    #[test]
    fn parent_directory_components_are_rejected() {
        for input in ["../etc/passwd", "usr/../../etc/passwd", "usr/bin/..", ".."] {
            assert!(
                deploy(input).is_err(),
                "{input} must be rejected as traversal"
            );
        }
    }

    #[test]
    fn nul_bytes_are_rejected() {
        let path = SourcePathBytes::new(b"usr/bin/foo\0.txt".to_vec());

        assert!(
            path.to_deployment_path().is_err(),
            "a NUL byte truncates the path at C API boundaries"
        );
    }

    #[test]
    fn paths_with_no_deployable_components_are_rejected() {
        for input in ["", "/", "///", ".", "./."] {
            assert!(deploy(input).is_err(), "{input} has nothing to deploy");
        }
    }

    /// Names that merely *contain* dots are ordinary names. Only an exact `..`
    /// component is traversal.
    #[test]
    fn names_containing_dots_are_not_traversal() {
        for input in ["usr/lib/..foo", "usr/lib/foo..", "usr/lib/a..b", "usr/..."] {
            assert!(
                deploy(input).is_ok(),
                "{input} is an ordinary name, not traversal"
            );
        }
    }

    // -- The defect this module replaces ------------------------------------

    /// The previous authority rejected every non-ASCII path as traversal. These
    /// are real repository packages named in issue #99.
    #[test]
    fn valid_non_ascii_paths_are_accepted_and_round_trip_byte_exactly() {
        let cases = [
            "usr/share/aisleriot/cards/fran\u{e7}ais.svg",
            "usr/share/doc/aspell-nb/n\u{f8}rsk.txt",
            "usr/lib/azure-cli/\u{6587}\u{4ef6}.py",
            "usr/share/icons/\u{1f4e6}.png",
        ];

        for input in cases {
            let path = deploy(input).unwrap_or_else(|error| {
                panic!("{input} is a valid source path but was rejected: {error:?}")
            });
            assert_eq!(
                path.to_utf8().unwrap(),
                input.trim_start_matches('/'),
                "{input} must round-trip byte-exactly"
            );
        }
    }

    /// Non-ASCII is orthogonal to traversal: a non-ASCII path with `..` is
    /// still rejected, and an ASCII path without it is still accepted.
    #[test]
    fn character_class_does_not_decide_traversal() {
        assert!(deploy("usr/\u{e9}t\u{e9}/../../etc/passwd").is_err());
        assert!(deploy("usr/\u{e9}t\u{e9}/file").is_ok());
        assert!(deploy("usr/ascii/../../etc/passwd").is_err());
        assert!(deploy("usr/ascii/file").is_ok());
    }

    /// Canonically equivalent Unicode forms are distinct byte sequences and
    /// must stay distinct. Normalizing here would merge two paths a package
    /// may legitimately ship separately.
    #[test]
    fn unicode_normalization_is_not_applied() {
        let precomposed = deploy("usr/share/\u{e9}").unwrap();
        let decomposed = deploy("usr/share/e\u{301}").unwrap();

        assert_ne!(
            precomposed.to_bytes(),
            decomposed.to_bytes(),
            "NFC and NFD forms must remain distinct byte sequences"
        );
    }

    // -- Byte-level authority beyond UTF-8 ----------------------------------

    /// Structural validation is defined on bytes, so a non-UTF-8 path is
    /// judged for traversal without needing to be decodable.
    #[test]
    fn non_utf8_paths_are_structurally_validated_without_decoding() {
        let valid = SourcePathBytes::new(vec![b'u', b's', b'r', b'/', 0xff, 0xfe]);
        let traversal = SourcePathBytes::new(vec![0xff, b'/', b'.', b'.', b'/', b'x']);

        let deployed = valid.to_deployment_path().expect("structurally valid");
        assert_eq!(deployed.component_count(), 2);
        assert!(
            traversal.to_deployment_path().is_err(),
            "traversal is caught on bytes, not on decoded text"
        );
    }

    /// The persistence boundary is explicit: non-UTF-8 fails loudly here rather
    /// than being lossily written into a TEXT column.
    #[test]
    fn non_utf8_paths_fail_at_the_persistence_boundary_rather_than_corrupting() {
        let path = SourcePathBytes::new(vec![b'u', b's', b'r', b'/', 0xff])
            .to_deployment_path()
            .expect("structurally valid");

        let error = path.to_utf8().expect_err("must not persist lossily");
        assert!(
            format!("{error:?}").contains("UTF-8"),
            "the error must name the reason, got {error:?}"
        );
        assert_eq!(
            path.to_bytes(),
            vec![b'u', b's', b'r', b'/', 0xff],
            "the lossless bytes survive the failed projection"
        );
    }

    #[test]
    fn diagnostics_never_decide_anything_but_stay_readable() {
        let path = SourcePathBytes::new(vec![b'a', 0xff, b'b']);

        let rendered = path.to_diagnostic_string();

        assert!(rendered.contains('a') && rendered.contains('b'));
        assert_eq!(
            path.as_bytes(),
            &[b'a', 0xff, b'b'],
            "rendering must not mutate the authority"
        );
    }

    // -- Joining ------------------------------------------------------------

    #[test]
    fn joining_stays_under_the_root() {
        let root = Path::new("/target/root");
        let joined = deploy("/usr/bin/foo").unwrap().join_under(root);

        assert_eq!(joined, Path::new("/target/root/usr/bin/foo"));
        assert!(joined.starts_with(root));
    }

    #[cfg(unix)]
    #[test]
    fn joining_preserves_non_utf8_component_bytes() {
        use std::os::unix::ffi::OsStrExt;

        let path = SourcePathBytes::new(vec![b'u', b's', b'r', b'/', 0xff])
            .to_deployment_path()
            .unwrap();

        let joined = path.join_under("/root");

        assert!(
            joined.as_os_str().as_bytes().ends_with(&[0xff]),
            "the raw byte must survive into the filesystem path"
        );
    }
}
