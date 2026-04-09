// conary-test/src/build_info.rs

use serde::Serialize;
use std::borrow::ToOwned;

/// Stable build metadata captured at compile time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BuildInfo {
    pub version: String,
    pub git_commit: String,
    pub commit_timestamp: String,
    pub build_timestamp: Option<String>,
}

impl BuildInfo {
    /// Construct build metadata from stable string inputs.
    pub fn new(
        version: impl Into<String>,
        git_commit: impl Into<String>,
        commit_timestamp: impl Into<String>,
        build_timestamp: Option<impl Into<String>>,
    ) -> Self {
        Self {
            version: version.into(),
            git_commit: git_commit.into(),
            commit_timestamp: commit_timestamp.into(),
            build_timestamp: build_timestamp.map(Into::into),
        }
    }

    /// Capture the build metadata for this `conary-test` binary.
    pub fn current() -> Self {
        Self::new(
            env!("CARGO_PKG_VERSION"),
            env!("CONARY_TEST_GIT_COMMIT"),
            env!("CONARY_TEST_COMMIT_TIMESTAMP"),
            option_env!("CONARY_TEST_BUILD_TIMESTAMP").map(ToOwned::to_owned),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::BuildInfo;
    use serde_json::json;

    #[test]
    fn new_preserves_required_metadata_fields() {
        let info = BuildInfo::new("0.3.0", "abc123", "2026-04-09T00:00:00Z", Some("ci-build"));

        assert_eq!(info.version, "0.3.0");
        assert_eq!(info.git_commit, "abc123");
        assert_eq!(info.commit_timestamp, "2026-04-09T00:00:00Z");
        assert_eq!(info.build_timestamp.as_deref(), Some("ci-build"));
    }

    #[test]
    fn new_leaves_optional_build_timestamp_absent() {
        let info = BuildInfo::new(
            "0.3.0",
            "abc123",
            "2026-04-09T00:00:00Z",
            Option::<String>::None,
        );

        assert!(info.build_timestamp.is_none());
    }

    #[test]
    fn serializes_to_predictable_json_values() {
        let info = BuildInfo::new("0.3.0", "abc123", "2026-04-09T00:00:00Z", Some("ci-build"));

        assert_eq!(
            serde_json::to_value(info).unwrap(),
            json!({
                "version": "0.3.0",
                "git_commit": "abc123",
                "commit_timestamp": "2026-04-09T00:00:00Z",
                "build_timestamp": "ci-build"
            })
        );
    }

    #[test]
    fn current_exposes_embedded_metadata() {
        let info = BuildInfo::current();

        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        assert!(!info.git_commit.is_empty());
        assert!(!info.commit_timestamp.is_empty());
        assert_eq!(
            info.build_timestamp.as_deref(),
            option_env!("CONARY_TEST_BUILD_TIMESTAMP")
        );
    }
}
