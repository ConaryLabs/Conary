// apps/conary/src/commands/remi_publish.rs
//! Client-side Remi release publish transport.

use std::path::Path;

use anyhow::{Context, Result, bail};
use conary_core::ccs::CcsPackage;
use conary_core::packages::traits::PackageFormat;

pub const REMI_ADMIN_TOKEN_ENV: &str = "REMI_ADMIN_TOKEN";
pub const CONARY_REMI_ADMIN_TOKEN_ENV: &str = "CONARY_REMI_ADMIN_TOKEN";

pub struct RemiPublishOptions<'a> {
    pub artifact_path: &'a Path,
    pub target_url: &'a str,
    pub bearer_token: &'a str,
}

pub fn resolve_remi_publish_bearer_token() -> Result<String> {
    for key in [REMI_ADMIN_TOKEN_ENV, CONARY_REMI_ADMIN_TOKEN_ENV] {
        if let Some(value) = std::env::var_os(key) {
            let token = value.to_string_lossy().trim().to_string();
            if !token.is_empty() {
                return Ok(token);
            }
        }
    }

    bail!("Remi release publish requires {REMI_ADMIN_TOKEN_ENV} or {CONARY_REMI_ADMIN_TOKEN_ENV}")
}

pub async fn publish_to_remi(options: RemiPublishOptions<'_>) -> Result<()> {
    preflight_release_artifact(options.artifact_path)?;
    let bytes = tokio::fs::read(options.artifact_path)
        .await
        .with_context(|| format!("read artifact {}", options.artifact_path.display()))?;
    if bytes.is_empty() {
        bail!("artifact {} is empty", options.artifact_path.display());
    }

    let client = reqwest::Client::new();
    let response = client
        .post(options.target_url)
        .bearer_auth(options.bearer_token)
        .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
        .body(bytes)
        .send()
        .await
        .context("send Remi release upload")?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        bail!("Remi release upload failed with {status}: {body}");
    }

    Ok(())
}

fn preflight_release_artifact(artifact_path: &Path) -> Result<()> {
    let path = artifact_path
        .to_str()
        .context("Remi release artifact path must be valid UTF-8")?;
    CcsPackage::parse(path)
        .map(|_| ())
        .map_err(anyhow::Error::from)
        .with_context(|| format!("preflight CCS artifact {}", artifact_path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::Mutex;

    #[test]
    fn resolve_remi_publish_bearer_token_uses_remi_admin_token() {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let _conary_guard = EnvVarGuard::remove(CONARY_REMI_ADMIN_TOKEN_ENV);
        let _remi_guard = EnvVarGuard::set(REMI_ADMIN_TOKEN_ENV, "admin-token");

        assert_eq!(resolve_remi_publish_bearer_token().unwrap(), "admin-token");
    }

    #[test]
    fn resolve_remi_publish_bearer_token_rejects_missing_token() {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let _remi_guard = EnvVarGuard::remove(REMI_ADMIN_TOKEN_ENV);
        let _conary_guard = EnvVarGuard::remove(CONARY_REMI_ADMIN_TOKEN_ENV);

        let error = resolve_remi_publish_bearer_token().unwrap_err();

        assert!(error.to_string().contains(REMI_ADMIN_TOKEN_ENV));
    }

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let previous = std::env::var_os(key);
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, previous }
        }

        fn remove(key: &'static str) -> Self {
            let previous = std::env::var_os(key);
            unsafe {
                std::env::remove_var(key);
            }
            Self { key, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            unsafe {
                if let Some(previous) = &self.previous {
                    std::env::set_var(self.key, previous);
                } else {
                    std::env::remove_var(self.key);
                }
            }
        }
    }

    static ENV_LOCK: Mutex<()> = Mutex::new(());
}
