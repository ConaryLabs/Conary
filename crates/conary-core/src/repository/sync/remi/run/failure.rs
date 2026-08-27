// conary-core/src/repository/sync/remi/run/failure.rs

//! Typed durable failure evidence for one profile refresh run.

use crate::error::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileSyncFailureStage {
    FetchingObjects,
    Ingesting,
    Publishing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileSyncFailureCategory {
    Transport,
    WireContract,
    Database,
    Fenced,
    Internal,
}

pub type RemiSyncFailureStage = ProfileSyncFailureStage;
pub type RemiSyncFailureCategory = ProfileSyncFailureCategory;

impl ProfileSyncFailureCategory {
    pub fn from_error(error: &Error) -> Self {
        match error {
            Error::DownloadError(_)
            | Error::RepositoryResponseBody { .. }
            | Error::HttpStatus { .. }
            | Error::TimeoutError(_) => Self::Transport,
            Error::ParseError(_) | Error::ConfigError(_) | Error::Json(_) => Self::WireContract,
            Error::Database(_) => Self::Database,
            Error::ConflictError(_) => Self::Fenced,
            _ => Self::Internal,
        }
    }

    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Transport => "transport",
            Self::WireContract => "wire_contract",
            Self::Database => "database",
            Self::Fenced => "fenced",
            Self::Internal => "internal",
        }
    }
}

impl ProfileSyncFailureStage {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::FetchingObjects => "fetching_objects",
            Self::Ingesting => "ingesting",
            Self::Publishing => "publishing",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupted_repository_bodies_are_transport_failures() {
        let error = Error::RepositoryResponseBody {
            url: "https://fixture.invalid/InRelease".to_string(),
            detail: "body ended early".to_string(),
        };

        assert_eq!(
            ProfileSyncFailureCategory::from_error(&error),
            ProfileSyncFailureCategory::Transport
        );
    }
}
