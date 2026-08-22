// apps/remi/src/server/admin_service/repository_policy.rs

use conary_core::db::models::{
    AuthenticatedSnapshotIdentity, NativeSourceEcosystem, NativeSourceStream, Repository,
    RepositoryPolicyScope, RepositorySourcePolicy, RepositoryUpdateMode,
};
use conary_core::repository::RepositoryFormat;

#[derive(Debug, Clone)]
pub struct NativeSourcePolicyInput {
    pub source_identity: String,
    pub repository_identity: String,
    pub stream_kind: String,
    pub stream_identity: String,
    pub policy_group: Option<String>,
    pub update_mode: String,
    pub pinned_snapshot_sha256: Option<String>,
}

pub(super) fn apply_native_source_contract(
    repository: &mut Repository,
    input: Option<NativeSourcePolicyInput>,
) -> conary_core::Result<()> {
    let native_format = matches!(
        repository.package_format,
        RepositoryFormat::Arch | RepositoryFormat::Debian | RepositoryFormat::Fedora
    );
    if !native_format {
        if input.is_some() {
            return Err(conary_core::Error::ConfigError(
                "native source policy is valid only for rpm, deb, or arch repositories".to_string(),
            ));
        }
        repository.source_policy = None;
        repository.repository_identity = None;
        repository.stream_binding_sha256 = None;
        repository.pinned_snapshot = None;
        return Ok(());
    }

    let input = input.ok_or_else(|| {
        conary_core::Error::ConfigError(
            "native repository requires exact source identity, repository identity, stream, and follow-or-pin policy"
                .to_string(),
        )
    })?;
    let ecosystem = NativeSourceEcosystem::from_repository_format(repository.package_format)?;
    let stream = match input.stream_kind.as_str() {
        "release" => NativeSourceStream::release(input.stream_identity)?,
        "channel" => NativeSourceStream::channel(input.stream_identity)?,
        "rolling" => NativeSourceStream::rolling(input.stream_identity)?,
        other => {
            return Err(conary_core::Error::ConfigError(format!(
                "unknown native source stream kind '{other}'"
            )));
        }
    };
    let scope = match input.policy_group {
        Some(group) => RepositoryPolicyScope::group(group)?,
        None => RepositoryPolicyScope::repository(input.repository_identity.clone())?,
    };
    let (update_mode, pinned_snapshot) =
        match (input.update_mode.as_str(), input.pinned_snapshot_sha256) {
            ("follow", None) => (RepositoryUpdateMode::Follow, None),
            ("pin", Some(snapshot)) => (
                RepositoryUpdateMode::Pin,
                Some(AuthenticatedSnapshotIdentity::from_sha256(snapshot)?),
            ),
            ("follow", Some(_)) => {
                return Err(conary_core::Error::ConfigError(
                    "follow policy cannot declare a pinned snapshot".to_string(),
                ));
            }
            ("pin", None) => {
                return Err(conary_core::Error::ConfigError(
                    "pin policy requires an authenticated snapshot SHA-256".to_string(),
                ));
            }
            (other, _) => {
                return Err(conary_core::Error::ConfigError(format!(
                    "unknown native repository update mode '{other}'"
                )));
            }
        };
    let policy =
        RepositorySourcePolicy::new(input.source_identity, scope, ecosystem, stream, update_mode)?;
    repository.set_native_source_policy(policy, input.repository_identity, pinned_snapshot)?;
    Ok(())
}
