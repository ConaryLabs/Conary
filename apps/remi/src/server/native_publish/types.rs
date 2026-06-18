// apps/remi/src/server/native_publish/types.rs

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use conary_core::ccs::CcsPackage;
use conary_core::repository::static_repo::publish_gate::PublishLintReport;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NativePublishErrorCode {
    InvalidCcs,
    UnsupportedCcsFormat,
    PackageSignatureFailed,
    PublishGateFailed,
    UntrustedBuildAttestationSigner,
    OutputIdentityMismatch,
    LocalDevArtifactRefused,
    UnsupportedDistro,
    MetadataCommitFailed,
    IoError,
}

#[derive(Debug)]
pub struct NativePublishError {
    pub status: StatusCode,
    pub code: NativePublishErrorCode,
    pub message: String,
}

impl NativePublishError {
    pub fn unprocessable(code: NativePublishErrorCode, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code,
            message: message.into(),
        }
    }

    pub fn internal(code: NativePublishErrorCode, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code,
            message: message.into(),
        }
    }

    pub fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({
                "code": self.code,
                "error": self.message,
            })),
        )
            .into_response()
    }
}

#[derive(Debug)]
pub struct VerifiedNativeArtifact {
    pub package: CcsPackage,
    pub lint: PublishLintReport,
    pub name: String,
    pub version: String,
    pub package_release: String,
    pub architecture: String,
    pub package_kind: String,
    pub authority_format_version: i64,
    pub content_hash: String,
    pub total_size: u64,
}

#[derive(Debug, Clone)]
pub struct NativePublishResult {
    pub distro: String,
    pub package: String,
    pub version: String,
    pub package_release: String,
    pub architecture: String,
    pub path: PathBuf,
    pub size: u64,
    pub content_hash: String,
}
