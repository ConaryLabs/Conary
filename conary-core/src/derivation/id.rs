// conary-core/src/derivation/id.rs

//! Derivation ID computation via canonical serialization.
//!
//! `DerivationId` is the SHA-256 of a canonical byte string built from all
//! build inputs. `SourceDerivationId` excludes `build_env_hash` so that the
//! same source+script+deps combination can be verified across different build
//! environments.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt;

/// Version prefix for the canonical derivation format.
const CANONICAL_PREFIX: &str = "CONARY-DERIVATION-V1";

/// Content-addressed derivation identifier (SHA-256, 64-char hex).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DerivationId(String);

/// Source-only derivation identifier that excludes `build_env_hash`.
///
/// Used for cross-seed verification: two builds from the same source, script,
/// and dependencies should share a `SourceDerivationId` even if built in
/// different environments.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceDerivationId(String);

/// All inputs that feed into a derivation ID computation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivationInputs {
    /// SHA-256 of the source archive or tree.
    pub source_hash: String,
    /// SHA-256 of the build script.
    pub build_script_hash: String,
    /// Map of dependency name to its `DerivationId`.
    pub dependency_ids: BTreeMap<String, DerivationId>,
    /// SHA-256 of the build environment EROFS image.
    pub build_env_hash: String,
    /// Target triple (e.g. `x86_64-unknown-linux-gnu`).
    pub target_triple: String,
    /// Arbitrary key-value build options.
    pub build_options: BTreeMap<String, String>,
}

impl DerivationId {
    /// Compute a `DerivationId` from the given inputs.
    #[must_use]
    pub fn compute(inputs: &DerivationInputs) -> Self {
        let canonical = Self::canonical_string(inputs);
        let hash = Sha256::digest(canonical.as_bytes());
        Self(hex::encode(hash))
    }

    /// Produce the canonical serialization of the derivation inputs.
    ///
    /// Format (newline-terminated lines):
    /// ```text
    /// CONARY-DERIVATION-V1
    /// source:<source_sha256>
    /// script:<build_script_sha256>
    /// dep:<dep_name>:<dep_derivation_id>   (sorted by name)
    /// env:<build_env_erofs_sha256>
    /// target:<target_triple>
    /// opt:<key>:<value>                    (sorted by key)
    /// ```
    #[must_use]
    pub fn canonical_string(inputs: &DerivationInputs) -> String {
        let mut lines = Vec::new();

        lines.push(CANONICAL_PREFIX.to_owned());
        lines.push(format!("source:{}", inputs.source_hash));
        lines.push(format!("script:{}", inputs.build_script_hash));

        // BTreeMap iterates in sorted key order.
        for (name, id) in &inputs.dependency_ids {
            lines.push(format!("dep:{name}:{id}"));
        }

        lines.push(format!("env:{}", inputs.build_env_hash));
        lines.push(format!("target:{}", inputs.target_triple));

        for (key, value) in &inputs.build_options {
            lines.push(format!("opt:{key}:{value}"));
        }

        // Each line is terminated by newline, including the last.
        let mut out = String::new();
        for line in lines {
            out.push_str(&line);
            out.push('\n');
        }
        out
    }

    /// Return the raw 64-char hex string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DerivationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl SourceDerivationId {
    /// Compute a `SourceDerivationId` from the given inputs, excluding
    /// `build_env_hash`.
    #[must_use]
    pub fn compute(inputs: &DerivationInputs) -> Self {
        let canonical = Self::canonical_string(inputs);
        let hash = Sha256::digest(canonical.as_bytes());
        Self(hex::encode(hash))
    }

    /// Produce the canonical serialization excluding `build_env_hash`.
    ///
    /// Same format as `DerivationId::canonical_string` but without the
    /// `env:` line.
    #[must_use]
    pub fn canonical_string(inputs: &DerivationInputs) -> String {
        let mut lines = Vec::new();

        lines.push(CANONICAL_PREFIX.to_owned());
        lines.push(format!("source:{}", inputs.source_hash));
        lines.push(format!("script:{}", inputs.build_script_hash));

        for (name, id) in &inputs.dependency_ids {
            lines.push(format!("dep:{name}:{id}"));
        }

        // env line intentionally omitted.
        lines.push(format!("target:{}", inputs.target_triple));

        for (key, value) in &inputs.build_options {
            lines.push(format!("opt:{key}:{value}"));
        }

        let mut out = String::new();
        for line in lines {
            out.push_str(&line);
            out.push('\n');
        }
        out
    }

    /// Return the raw 64-char hex string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SourceDerivationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_inputs() -> DerivationInputs {
        let mut deps = BTreeMap::new();
        deps.insert(
            "glibc".to_owned(),
            DerivationId("a".repeat(64)),
        );
        deps.insert(
            "zlib".to_owned(),
            DerivationId("b".repeat(64)),
        );

        DerivationInputs {
            source_hash: "abc123".to_owned(),
            build_script_hash: "def456".to_owned(),
            dependency_ids: deps,
            build_env_hash: "env789".to_owned(),
            target_triple: "x86_64-unknown-linux-gnu".to_owned(),
            build_options: BTreeMap::from([
                ("optimize".to_owned(), "2".to_owned()),
                ("debug".to_owned(), "false".to_owned()),
            ]),
        }
    }

    #[test]
    fn derivation_id_is_deterministic() {
        let inputs = sample_inputs();
        let id1 = DerivationId::compute(&inputs);
        let id2 = DerivationId::compute(&inputs);
        assert_eq!(id1, id2);
    }

    #[test]
    fn different_inputs_produce_different_ids() {
        let inputs1 = sample_inputs();
        let mut inputs2 = sample_inputs();
        inputs2.source_hash = "different_hash".to_owned();

        let id1 = DerivationId::compute(&inputs1);
        let id2 = DerivationId::compute(&inputs2);
        assert_ne!(id1, id2);
    }

    #[test]
    fn canonical_format_matches_spec() {
        let inputs = sample_inputs();
        let canonical = DerivationId::canonical_string(&inputs);

        assert!(canonical.starts_with("CONARY-DERIVATION-V1\n"));
        assert!(canonical.contains("source:abc123\n"));
        assert!(canonical.contains("script:def456\n"));
        assert!(canonical.contains("env:env789\n"));
        assert!(canonical.contains("target:x86_64-unknown-linux-gnu\n"));
        assert!(canonical.contains(&format!("dep:glibc:{}\n", "a".repeat(64))));
        assert!(canonical.contains(&format!("dep:zlib:{}\n", "b".repeat(64))));
        assert!(canonical.contains("opt:debug:false\n"));
        assert!(canonical.contains("opt:optimize:2\n"));
    }

    #[test]
    fn dependencies_are_sorted_by_name() {
        let inputs = sample_inputs();
        let canonical = DerivationId::canonical_string(&inputs);

        let glibc_pos = canonical.find("dep:glibc:").expect("glibc dep missing");
        let zlib_pos = canonical.find("dep:zlib:").expect("zlib dep missing");
        assert!(
            glibc_pos < zlib_pos,
            "dependencies must be sorted by name: glibc before zlib"
        );
    }

    #[test]
    fn source_derivation_id_excludes_env_hash() {
        let mut inputs1 = sample_inputs();
        let mut inputs2 = sample_inputs();
        inputs1.build_env_hash = "env_aaa".to_owned();
        inputs2.build_env_hash = "env_zzz".to_owned();

        let source_id1 = SourceDerivationId::compute(&inputs1);
        let source_id2 = SourceDerivationId::compute(&inputs2);
        assert_eq!(source_id1, source_id2, "SourceDerivationId must ignore env hash");

        // But the full DerivationId should differ.
        let full_id1 = DerivationId::compute(&inputs1);
        let full_id2 = DerivationId::compute(&inputs2);
        assert_ne!(full_id1, full_id2, "DerivationId must include env hash");
    }

    #[test]
    fn derivation_id_is_64_char_hex() {
        let inputs = sample_inputs();
        let id = DerivationId::compute(&inputs);
        let s = id.as_str();

        assert_eq!(s.len(), 64, "SHA-256 hex is 64 chars");
        assert!(
            s.chars().all(|c| c.is_ascii_hexdigit()),
            "must be valid hex"
        );
    }

    #[test]
    fn display_returns_hex_string() {
        let inputs = sample_inputs();
        let id = DerivationId::compute(&inputs);
        assert_eq!(format!("{id}"), id.as_str());

        let source_id = SourceDerivationId::compute(&inputs);
        assert_eq!(format!("{source_id}"), source_id.as_str());
    }

    #[test]
    fn source_canonical_string_omits_env_line() {
        let inputs = sample_inputs();
        let canonical = SourceDerivationId::canonical_string(&inputs);
        assert!(!canonical.contains("env:"), "source canonical must not contain env line");
    }
}
