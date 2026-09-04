// crates/conary-core/src/repository/sync/remi/run/contract.rs

//! Typed validation for durable profile-refresh run identities and members.

use super::ProfileSyncRunMember;
use crate::error::{Error, Result};

pub(super) fn validate_member(member: &ProfileSyncRunMember) -> Result<()> {
    if member.ordinal < 0 {
        return Err(Error::ConfigError(
            "sync run member ordinal must not be negative".to_string(),
        ));
    }
    if member.repository_id <= 0 {
        return Err(Error::ConfigError(
            "sync run member repository ID must be positive".to_string(),
        ));
    }
    validate_identity(&member.source_identity, "sync run member source identity")?;
    validate_identity(
        &member.repository_identity,
        "sync run member repository identity",
    )?;
    validate_identity(&member.stream_identity, "sync run member stream identity")?;
    if !matches!(
        member.stream_kind.as_str(),
        "release" | "channel" | "rolling"
    ) {
        return Err(Error::ConfigError(format!(
            "sync run member stream kind '{}' is unsupported",
            member.stream_kind
        )));
    }
    if let Some(digest) = member.candidate_source_snapshot_sha256.as_deref() {
        validate_digest(digest, "sync run candidate source snapshot digest")?;
    }
    if let Some(digest) = member.input_source_snapshot_sha256.as_deref() {
        validate_digest(digest, "sync run input source snapshot digest")?;
    }
    Ok(())
}

pub(super) fn validate_profile(value: &str) -> Result<()> {
    validate_identity(value, "sync run source profile")
}

pub(super) fn validate_identity(value: &str, label: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 255
        || value.trim() != value
        || !value.bytes().all(|byte| (0x20..=0x7e).contains(&byte))
    {
        return Err(Error::ConfigError(format!(
            "{label} must contain 1 to 255 printable ASCII characters without surrounding whitespace"
        )));
    }
    Ok(())
}

pub(super) fn validate_digest(value: &str, label: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::ConfigError(format!(
            "{label} must be exactly 64 lowercase hexadecimal characters"
        )));
    }
    Ok(())
}

pub(super) fn validate_uuid(value: &str, label: &str) -> Result<()> {
    if value.len() != 36 || uuid::Uuid::parse_str(value).is_err() {
        return Err(Error::ConfigError(format!(
            "{label} must be a canonical 36-character UUID"
        )));
    }
    Ok(())
}

/// Persisted profile-refresh lifecycle. Unknown values cannot authorize work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileSyncRunState {
    Created,
    FetchingObjects,
    ReadyToPublish,
    Candidate,
    Published,
    Failed,
    Abandoned,
}

impl ProfileSyncRunState {
    pub const TERMINAL_STATES: &'static [Self] = &[
        Self::Candidate,
        Self::Published,
        Self::Failed,
        Self::Abandoned,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::FetchingObjects => "fetching_objects",
            Self::ReadyToPublish => "ready_to_publish",
            Self::Candidate => "candidate",
            Self::Published => "published",
            Self::Failed => "failed",
            Self::Abandoned => "abandoned",
        }
    }

    pub fn is_terminal(self) -> bool {
        Self::TERMINAL_STATES.contains(&self)
    }

    pub(super) fn terminal_sql() -> &'static str {
        static SQL: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
            ProfileSyncRunState::TERMINAL_STATES
                .iter()
                .map(|state| format!("'{}'", state.as_str()))
                .collect::<Vec<_>>()
                .join(", ")
        });
        &SQL
    }
}

impl TryFrom<&str> for ProfileSyncRunState {
    type Error = crate::db::models::InvalidPersistedValue;

    fn try_from(value: &str) -> std::result::Result<Self, Self::Error> {
        match value {
            "created" => Ok(Self::Created),
            "fetching_objects" => Ok(Self::FetchingObjects),
            "ready_to_publish" => Ok(Self::ReadyToPublish),
            "candidate" => Ok(Self::Candidate),
            "published" => Ok(Self::Published),
            "failed" => Ok(Self::Failed),
            "abandoned" => Ok(Self::Abandoned),
            other => Err(Self::Error::new(
                "profile sync run state",
                other,
                "a current lifecycle state; fence this run or rebuild the database",
            )),
        }
    }
}
