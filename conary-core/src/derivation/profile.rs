// conary-core/src/derivation/profile.rs

//! Build profile type for deterministic build plans.
//!
//! A `BuildProfile` captures the complete, ordered plan for a bootstrap or
//! rebuild: which seed to start from, which stages to execute, and which
//! derivations each stage contains. The profile hash is a SHA-256 digest of
//! all this information, making it trivial to detect when a plan has changed.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

/// Version prefix for the canonical profile hash format.
const PROFILE_HASH_PREFIX: &str = "CONARY-PROFILE-V1";

/// A complete, deterministic build plan.
///
/// Captures the seed environment, the ordered stages, and every derivation
/// within each stage. Two profiles with identical contents will always produce
/// the same `profile_hash`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildProfile {
    /// Top-level metadata about this profile.
    pub profile: ProfileMetadata,
    /// Reference to the seed environment this profile starts from.
    pub seed: ProfileSeedRef,
    /// Ordered build stages.
    pub stages: Vec<ProfileStage>,
}

/// Metadata about the profile itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileMetadata {
    /// Name of the manifest that generated this profile.
    pub manifest: String,
    /// SHA-256 hash of the profile contents (computed, not stored on input).
    pub profile_hash: String,
    /// ISO 8601 timestamp when this profile was generated.
    pub generated_at: String,
    /// Target triple (e.g. `x86_64-unknown-linux-gnu`).
    pub target: String,
}

/// Reference to the seed (bootstrap) environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSeedRef {
    /// Seed identifier (e.g. derivation ID or image hash).
    pub id: String,
    /// Where the seed came from (e.g. `"local"`, `"remi"`, a URL).
    pub source: String,
}

/// A single stage within a build profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileStage {
    /// Human-readable stage name (e.g. `"phase-1"`, `"cross-tools"`).
    pub name: String,
    /// Identifier for the build environment used by this stage.
    pub build_env: String,
    /// Derivations to build in this stage, in order.
    pub derivations: Vec<ProfileDerivation>,
}

/// A single derivation within a stage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileDerivation {
    /// Package name.
    pub package: String,
    /// Package version string.
    pub version: String,
    /// Content-addressed derivation ID.
    pub derivation_id: String,
}

/// Diff between two build profiles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileDiff {
    /// Packages present in `other` but not in `self`.
    pub added: Vec<String>,
    /// Packages present in `self` but not in `other`.
    pub removed: Vec<String>,
    /// Packages present in both but with different derivation IDs or versions.
    pub changed: Vec<String>,
}

impl BuildProfile {
    /// Compute a deterministic SHA-256 hash of the profile contents.
    ///
    /// The hash covers the seed reference and every stage/derivation in order,
    /// using a canonical line-oriented format. The `profile_hash` and
    /// `generated_at` metadata fields are intentionally excluded so that
    /// regenerating the same logical plan always yields the same hash.
    #[must_use]
    pub fn compute_hash(&self) -> String {
        let canonical = self.canonical_string();
        let hash = Sha256::digest(canonical.as_bytes());
        hex::encode(hash)
    }

    /// Produce the canonical serialization used for hashing.
    ///
    /// Format (newline-terminated lines):
    /// ```text
    /// CONARY-PROFILE-V1
    /// manifest:<manifest>
    /// target:<target>
    /// seed:<id>:<source>
    /// stage:<name>:<build_env>
    /// drv:<package>:<version>:<derivation_id>
    /// ...
    /// ```
    #[must_use]
    fn canonical_string(&self) -> String {
        let mut lines = Vec::new();

        lines.push(PROFILE_HASH_PREFIX.to_owned());
        lines.push(format!("manifest:{}", self.profile.manifest));
        lines.push(format!("target:{}", self.profile.target));
        lines.push(format!("seed:{}:{}", self.seed.id, self.seed.source));

        for stage in &self.stages {
            lines.push(format!("stage:{}:{}", stage.name, stage.build_env));
            for drv in &stage.derivations {
                lines.push(format!(
                    "drv:{}:{}:{}",
                    drv.package, drv.version, drv.derivation_id
                ));
            }
        }

        let mut out = String::new();
        for line in lines {
            out.push_str(&line);
            out.push('\n');
        }
        out
    }

    /// Serialize this profile to TOML.
    ///
    /// # Errors
    ///
    /// Returns an error if serialization fails.
    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    /// Deserialize a profile from TOML.
    ///
    /// # Errors
    ///
    /// Returns an error if the input is not valid TOML or does not match the
    /// `BuildProfile` schema.
    pub fn from_toml(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    /// Compute the diff between `self` and `other`.
    ///
    /// A package is identified by its name. If a package appears in both
    /// profiles but with a different version or derivation ID, it is reported
    /// as "changed".
    #[must_use]
    pub fn diff(&self, other: &BuildProfile) -> ProfileDiff {
        let self_pkgs = self.collect_packages();
        let other_pkgs = other.collect_packages();

        let self_names: BTreeSet<&str> = self_pkgs.iter().map(|(name, _, _)| name.as_str()).collect();
        let other_names: BTreeSet<&str> =
            other_pkgs.iter().map(|(name, _, _)| name.as_str()).collect();

        let added: Vec<String> = other_names
            .difference(&self_names)
            .map(|s| (*s).to_owned())
            .collect();

        let removed: Vec<String> = self_names
            .difference(&other_names)
            .map(|s| (*s).to_owned())
            .collect();

        // For packages in both, check if version or derivation_id changed.
        let mut changed = Vec::new();
        for name in self_names.intersection(&other_names) {
            let self_entry = self_pkgs
                .iter()
                .find(|(n, _, _)| n == name)
                .expect("name came from self_pkgs");
            let other_entry = other_pkgs
                .iter()
                .find(|(n, _, _)| n == name)
                .expect("name came from other_pkgs");

            if self_entry.1 != other_entry.1 || self_entry.2 != other_entry.2 {
                changed.push((*name).to_owned());
            }
        }

        ProfileDiff {
            added,
            removed,
            changed,
        }
    }

    /// Collect all (package, version, derivation_id) tuples across all stages.
    fn collect_packages(&self) -> Vec<(String, String, String)> {
        self.stages
            .iter()
            .flat_map(|stage| {
                stage.derivations.iter().map(|drv| {
                    (
                        drv.package.clone(),
                        drv.version.clone(),
                        drv.derivation_id.clone(),
                    )
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_profile() -> BuildProfile {
        BuildProfile {
            profile: ProfileMetadata {
                manifest: "bootstrap-v2".to_owned(),
                profile_hash: String::new(),
                generated_at: "2026-03-19T00:00:00Z".to_owned(),
                target: "x86_64-unknown-linux-gnu".to_owned(),
            },
            seed: ProfileSeedRef {
                id: "seed-abc123".to_owned(),
                source: "local".to_owned(),
            },
            stages: vec![
                ProfileStage {
                    name: "phase-1".to_owned(),
                    build_env: "env-aaa".to_owned(),
                    derivations: vec![
                        ProfileDerivation {
                            package: "binutils".to_owned(),
                            version: "2.42".to_owned(),
                            derivation_id: "d".repeat(64),
                        },
                        ProfileDerivation {
                            package: "gcc".to_owned(),
                            version: "14.1".to_owned(),
                            derivation_id: "e".repeat(64),
                        },
                    ],
                },
                ProfileStage {
                    name: "phase-2".to_owned(),
                    build_env: "env-bbb".to_owned(),
                    derivations: vec![ProfileDerivation {
                        package: "glibc".to_owned(),
                        version: "2.39".to_owned(),
                        derivation_id: "f".repeat(64),
                    }],
                },
            ],
        }
    }

    #[test]
    fn profile_hash_is_deterministic() {
        let p1 = sample_profile();
        let p2 = sample_profile();

        let h1 = p1.compute_hash();
        let h2 = p2.compute_hash();

        assert_eq!(h1, h2, "identical profiles must produce the same hash");
        assert_eq!(h1.len(), 64, "SHA-256 hex is 64 chars");
        assert!(
            h1.chars().all(|c| c.is_ascii_hexdigit()),
            "must be valid hex"
        );
    }

    #[test]
    fn different_seeds_produce_different_hashes() {
        let p1 = sample_profile();
        let mut p2 = sample_profile();
        p2.seed.id = "seed-different".to_owned();

        let h1 = p1.compute_hash();
        let h2 = p2.compute_hash();

        assert_ne!(h1, h2, "different seeds must produce different hashes");
    }

    #[test]
    fn different_derivations_produce_different_hashes() {
        let p1 = sample_profile();
        let mut p2 = sample_profile();
        p2.stages[0].derivations[0].version = "2.43".to_owned();

        let h1 = p1.compute_hash();
        let h2 = p2.compute_hash();

        assert_ne!(
            h1, h2,
            "changing a derivation version must change the hash"
        );
    }

    #[test]
    fn hash_excludes_generated_at_and_profile_hash() {
        let mut p1 = sample_profile();
        let mut p2 = sample_profile();
        p1.profile.generated_at = "2026-01-01T00:00:00Z".to_owned();
        p2.profile.generated_at = "2026-12-31T23:59:59Z".to_owned();
        p1.profile.profile_hash = "old-hash".to_owned();
        p2.profile.profile_hash = "new-hash".to_owned();

        let h1 = p1.compute_hash();
        let h2 = p2.compute_hash();

        assert_eq!(
            h1, h2,
            "generated_at and profile_hash must not affect the hash"
        );
    }

    #[test]
    fn toml_roundtrip() {
        let original = sample_profile();
        let toml_str = original.to_toml().expect("serialization should succeed");
        let restored =
            BuildProfile::from_toml(&toml_str).expect("deserialization should succeed");

        assert_eq!(original, restored, "TOML roundtrip must be lossless");
    }

    #[test]
    fn toml_output_is_valid() {
        let profile = sample_profile();
        let toml_str = profile.to_toml().expect("serialization should succeed");

        // Basic structure checks.
        assert!(toml_str.contains("[profile]"), "must contain [profile] section");
        assert!(toml_str.contains("[seed]"), "must contain [seed] section");
        assert!(
            toml_str.contains("[[stages]]"),
            "must contain [[stages]] array"
        );
    }

    #[test]
    fn diff_detects_added_packages() {
        let p1 = sample_profile();
        let mut p2 = sample_profile();
        p2.stages[1].derivations.push(ProfileDerivation {
            package: "zlib".to_owned(),
            version: "1.3.1".to_owned(),
            derivation_id: "a".repeat(64),
        });

        let diff = p1.diff(&p2);

        assert_eq!(diff.added, vec!["zlib"]);
        assert!(diff.removed.is_empty());
        assert!(diff.changed.is_empty());
    }

    #[test]
    fn diff_detects_removed_packages() {
        let p1 = sample_profile();
        let mut p2 = sample_profile();
        // Remove glibc from phase-2.
        p2.stages[1].derivations.clear();

        let diff = p1.diff(&p2);

        assert!(diff.added.is_empty());
        assert_eq!(diff.removed, vec!["glibc"]);
        assert!(diff.changed.is_empty());
    }

    #[test]
    fn diff_detects_changed_packages() {
        let p1 = sample_profile();
        let mut p2 = sample_profile();
        // Change gcc version.
        p2.stages[0].derivations[1].version = "14.2".to_owned();
        p2.stages[0].derivations[1].derivation_id = "x".repeat(64);

        let diff = p1.diff(&p2);

        assert!(diff.added.is_empty());
        assert!(diff.removed.is_empty());
        assert_eq!(diff.changed, vec!["gcc"]);
    }

    #[test]
    fn diff_handles_multiple_changes() {
        let p1 = sample_profile();
        let mut p2 = sample_profile();

        // Remove binutils.
        p2.stages[0].derivations.remove(0);
        // Change glibc version.
        p2.stages[1].derivations[0].version = "2.40".to_owned();
        // Add zlib.
        p2.stages[1].derivations.push(ProfileDerivation {
            package: "zlib".to_owned(),
            version: "1.3.1".to_owned(),
            derivation_id: "a".repeat(64),
        });

        let diff = p1.diff(&p2);

        assert_eq!(diff.added, vec!["zlib"]);
        assert_eq!(diff.removed, vec!["binutils"]);
        assert_eq!(diff.changed, vec!["glibc"]);
    }

    #[test]
    fn diff_identical_profiles_is_empty() {
        let p1 = sample_profile();
        let p2 = sample_profile();

        let diff = p1.diff(&p2);

        assert!(diff.added.is_empty());
        assert!(diff.removed.is_empty());
        assert!(diff.changed.is_empty());
    }
}
