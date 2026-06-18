// apps/conary/src/commands/ccs/local_dev.rs

use anyhow::{Context, Result};
use conary_core::ccs::signing::SigningKeyPair;
use std::path::{Path, PathBuf};

pub struct LocalDevKeyPaths {
    pub private: PathBuf,
    pub public: PathBuf,
}

pub fn local_dev_key_paths() -> Result<LocalDevKeyPaths> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .context("HOME or XDG_DATA_HOME is required for local-dev CCS keys")?
        .join("conary")
        .join("ccs")
        .join("local-dev");
    Ok(LocalDevKeyPaths {
        private: base.join("local-dev-key.private.toml"),
        public: base.join("local-dev-key.public.toml"),
    })
}

pub fn load_or_create_local_dev_key() -> Result<SigningKeyPair> {
    let paths = local_dev_key_paths()?;
    if paths.private.exists() {
        return SigningKeyPair::load_from_file(&paths.private)
            .map_err(anyhow::Error::from)
            .with_context(|| format!("load local-dev key {}", paths.private.display()));
    }
    let key = SigningKeyPair::generate().with_key_id("local-dev");
    key.save_to_files(&paths.private, &paths.public)
        .map_err(anyhow::Error::from)
        .with_context(|| format!("write local-dev key {}", paths.private.display()))?;
    Ok(key)
}

pub fn write_local_dev_policy(path: &Path, key: &SigningKeyPair) -> Result<()> {
    std::fs::write(
        path,
        format!(
            "trusted_keys = [\"{}\"]\nallow_unsigned = false\nrequire_timestamp = false\n",
            key.public_key_base64()
        ),
    )
    .with_context(|| format!("write local-dev trust policy {}", path.display()))
}

pub fn local_dev_trust_policy() -> Result<Option<conary_core::ccs::verify::TrustPolicy>> {
    let paths = local_dev_key_paths()?;
    if !paths.public.exists() {
        return Ok(None);
    }

    #[derive(serde::Deserialize)]
    struct PublicKeyFile {
        key: String,
    }

    let public_text = std::fs::read_to_string(&paths.public)
        .with_context(|| format!("read local-dev public key {}", paths.public.display()))?;
    let public_key: PublicKeyFile = toml::from_str(&public_text)
        .with_context(|| format!("parse local-dev public key {}", paths.public.display()))?;
    Ok(Some(conary_core::ccs::verify::TrustPolicy::strict(vec![
        public_key.key,
    ])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_dev_policy_trusts_generated_public_key() {
        let temp = tempfile::tempdir().unwrap();
        let key = conary_core::ccs::signing::SigningKeyPair::generate().with_key_id("local-dev");
        let policy_path = temp.path().join("policy.toml");

        write_local_dev_policy(&policy_path, &key).unwrap();
        let policy = conary_core::ccs::verify::TrustPolicy::from_file(&policy_path).unwrap();

        assert_eq!(policy.trusted_keys, vec![key.public_key_base64()]);
        assert!(!policy.allow_unsigned);
    }
}
