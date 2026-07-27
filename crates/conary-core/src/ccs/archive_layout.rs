// conary-core/src/ccs/archive_layout.rs

//! The directory layout a CCS v2 archive is allowed to contain.
//!
//! One owner for the layout vocabulary. The writer emits these names, and both
//! the streaming verifier and the archive reader admit exactly this set, so a
//! package the writer produces is reader-admissible by construction rather than
//! by separate hand-maintained lists happening to agree.
//!
//! They did not agree. Two copies of this rule previously existed, and when the
//! structural budget work added a second reader it dropped `components` from
//! one of them, so any package carrying a component directory failed
//! verification while its own writer had just emitted it. That is the exact
//! writer-versus-reader disagreement the budget owner was introduced to
//! prevent, reproduced one layer up. Hence: one owner here too.
//!
//! Membership is returned as a `bool` rather than a `Result` because the two
//! readers carry different error types. Each caller states its own failure.

/// Component payload directory.
pub const COMPONENTS_DIR: &str = "components";

/// Content-addressed object directory.
pub const OBJECTS_DIR: &str = "objects";

/// Whether `path` is a directory entry the CCS v2 layout defines.
///
/// Accepts the two top-level directories and the two-hex-character fan-out
/// directories beneath `objects/`. Anything else is rejected: an unrecognized
/// directory in a signed archive is unaccounted-for structure, not a harmless
/// extra.
pub fn is_known_directory(path: &str) -> bool {
    if path == COMPONENTS_DIR || path == OBJECTS_DIR {
        return true;
    }

    matches!(
        path.strip_prefix(concat!("objects", "/")),
        Some(prefix) if prefix.len() == 2 && is_lower_hex(prefix)
    )
}

/// Whether every byte is a lowercase hexadecimal digit.
///
/// Case matters: object fan-out directories are emitted lowercase, so accepting
/// uppercase would admit two spellings of one address.
pub fn is_lower_hex(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The component name a `components/<name>.json` entry declares, if the path is
/// a well-formed component entry.
///
/// Returns `None` for anything else so callers can use it as a match guard. A
/// name may not be empty or contain a separator: `components/a/b.json` names no
/// component and must not be admitted as one.
pub fn component_entry_name(path: &str) -> Option<&str> {
    let name = path
        .strip_prefix(COMPONENTS_DIR)?
        .strip_prefix('/')?
        .strip_suffix(".json")?;

    (!name.is_empty() && !name.contains('/')).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_top_level_directories_are_known() {
        assert!(is_known_directory(COMPONENTS_DIR));
        assert!(is_known_directory(OBJECTS_DIR));
    }

    /// The regression that motivated this module: `components` was dropped from
    /// one of two copies of this rule, so writer output failed its own reader.
    #[test]
    fn components_is_admitted_because_the_writer_emits_it() {
        assert!(
            is_known_directory("components"),
            "the writer emits a components directory; the reader must admit it"
        );
    }

    #[test]
    fn object_fanout_directories_are_known() {
        for path in ["objects/00", "objects/ab", "objects/9f", "objects/ff"] {
            assert!(is_known_directory(path), "{path} should be known");
        }
    }

    #[test]
    fn malformed_object_fanout_is_rejected() {
        for path in [
            "objects/A0",
            "objects/abc",
            "objects/a",
            "objects/",
            "objects/zz",
            "objects/ab/cd",
        ] {
            assert!(!is_known_directory(path), "{path} should be rejected");
        }
    }

    #[test]
    fn unrelated_directories_are_rejected() {
        for path in ["", "manifest", "component", "objects2", "../objects"] {
            assert!(!is_known_directory(path), "{path} should be rejected");
        }
    }

    /// Closes the gap that let the regression through: #103 proved writer and
    /// reader share one *budget* owner, but nothing proved they share one
    /// *layout* owner. Every directory the writer can emit must be admitted.
    #[test]
    fn every_directory_the_writer_emits_is_reader_admissible() {
        let writer_emits = [
            COMPONENTS_DIR,
            OBJECTS_DIR,
            "objects/00",
            "objects/0f",
            "objects/ff",
        ];

        for path in writer_emits {
            assert!(
                is_known_directory(path),
                "the writer emits {path:?}; a reader that rejects it makes its own output unreadable"
            );
        }
    }

    #[test]
    fn component_entries_yield_their_declared_name() {
        assert_eq!(
            component_entry_name("components/runtime.json"),
            Some("runtime")
        );
        assert_eq!(component_entry_name("components/dev.json"), Some("dev"));
    }

    #[test]
    fn malformed_component_entries_name_nothing() {
        for path in [
            "components/.json",
            "components/a/b.json",
            "components/runtime.txt",
            "components/runtime",
            "components.json",
            "objects/runtime.json",
            "components/",
        ] {
            assert_eq!(
                component_entry_name(path),
                None,
                "{path} names no component"
            );
        }
    }

    #[test]
    fn lower_hex_rejects_uppercase_and_empty() {
        assert!(is_lower_hex("0f"));
        assert!(!is_lower_hex("0F"));
        assert!(!is_lower_hex(""));
        assert!(!is_lower_hex("g0"));
    }
}
