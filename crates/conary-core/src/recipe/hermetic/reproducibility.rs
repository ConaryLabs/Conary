// conary-core/src/recipe/hermetic/reproducibility.rs

use crate::recipe::hermetic::evidence::ReproducibilityRecord;
use crate::{Error, Result};
use std::path::{Path, PathBuf};

const SOURCE_DATE_EPOCH: &str = "SOURCE_DATE_EPOCH";
const RUSTFLAGS: &str = "RUSTFLAGS";
const CFLAGS: &str = "CFLAGS";
const CXXFLAGS: &str = "CXXFLAGS";
const PATH_REMAP_COUNT: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReproducibilityConfig {
    pub source_date_epoch: i64,
    pub source_root: PathBuf,
    pub build_root: PathBuf,
}

impl Default for ReproducibilityConfig {
    fn default() -> Self {
        Self {
            source_date_epoch: 0,
            source_root: PathBuf::new(),
            build_root: PathBuf::new(),
        }
    }
}

impl ReproducibilityConfig {
    pub fn new(source_date_epoch: i64, source_root: &Path, build_root: &Path) -> Self {
        Self {
            source_date_epoch,
            source_root: source_root.to_path_buf(),
            build_root: build_root.to_path_buf(),
        }
    }

    pub fn with_roots(&self, source_root: &Path, build_root: &Path) -> Self {
        Self {
            source_date_epoch: self.source_date_epoch,
            source_root: source_root.to_path_buf(),
            build_root: build_root.to_path_buf(),
        }
    }

    pub fn env_vars(&self) -> Vec<(String, String)> {
        vec![
            (
                SOURCE_DATE_EPOCH.to_string(),
                self.source_date_epoch.to_string(),
            ),
            (RUSTFLAGS.to_string(), self.rust_remaps().join(" ")),
            (CFLAGS.to_string(), self.file_prefix_remaps().join(" ")),
            (CXXFLAGS.to_string(), self.file_prefix_remaps().join(" ")),
        ]
    }

    pub fn merge_env(&self, recipe_env: Vec<(String, String)>) -> Result<Vec<(String, String)>> {
        if recipe_env.iter().any(|(key, _)| key == SOURCE_DATE_EPOCH) {
            return Err(Error::ConfigError(
                "hermetic reproducibility controls SOURCE_DATE_EPOCH; recipe or extra environment cannot override it"
                    .to_string(),
            ));
        }

        let mut merged = recipe_env;
        merged.push((
            SOURCE_DATE_EPOCH.to_string(),
            self.source_date_epoch.to_string(),
        ));
        self.append_or_insert_flags(&mut merged, RUSTFLAGS, &self.rust_remaps());
        self.append_or_insert_flags(&mut merged, CFLAGS, &self.file_prefix_remaps());
        self.append_or_insert_flags(&mut merged, CXXFLAGS, &self.file_prefix_remaps());
        Ok(merged)
    }

    pub fn validate_final_env(&self, env: &[(String, String)]) -> Result<()> {
        let expected_epoch = self.source_date_epoch.to_string();
        match effective_env_value(env, SOURCE_DATE_EPOCH) {
            Some(value) if value == expected_epoch => {}
            Some(value) => {
                return Err(Error::ConfigError(format!(
                    "hermetic reproducibility requires SOURCE_DATE_EPOCH={expected_epoch}, got {value}"
                )));
            }
            None => {
                return Err(Error::ConfigError(
                    "hermetic reproducibility requires SOURCE_DATE_EPOCH".to_string(),
                ));
            }
        }

        self.validate_required_tokens(env, RUSTFLAGS, &self.rust_remaps())?;
        self.validate_required_tokens(env, CFLAGS, &self.file_prefix_remaps())?;
        self.validate_required_tokens(env, CXXFLAGS, &self.file_prefix_remaps())?;
        Ok(())
    }

    pub fn record(&self) -> ReproducibilityRecord {
        let mut env_keys: Vec<String> = self.env_vars().into_iter().map(|(key, _)| key).collect();
        env_keys.sort();

        ReproducibilityRecord {
            source_date_epoch: Some(self.source_date_epoch),
            path_remap_count: PATH_REMAP_COUNT,
            env_keys,
        }
    }

    pub(crate) fn controlled_env_keys() -> &'static [&'static str] {
        &[SOURCE_DATE_EPOCH, RUSTFLAGS, CFLAGS, CXXFLAGS]
    }

    pub(crate) fn command_local_assignment_allowed(&self, key: &str, value: &str) -> bool {
        match key {
            SOURCE_DATE_EPOCH => false,
            RUSTFLAGS => self
                .rust_remaps()
                .iter()
                .all(|required| value.contains(required)),
            CFLAGS | CXXFLAGS => self
                .file_prefix_remaps()
                .iter()
                .all(|required| value.contains(required)),
            _ => true,
        }
    }

    fn append_or_insert_flags(
        &self,
        env: &mut Vec<(String, String)>,
        key: &str,
        required_flags: &[String],
    ) {
        let required = required_flags.join(" ");
        let existing = effective_env_value(env, key).map(str::to_string);
        env.retain(|(candidate, _)| candidate != key);
        let value = match existing.as_deref() {
            Some(existing) if !existing.trim().is_empty() => format!("{} {}", existing, required),
            _ => required,
        };
        env.push((key.to_string(), value));
    }

    fn validate_required_tokens(
        &self,
        env: &[(String, String)],
        key: &str,
        required: &[String],
    ) -> Result<()> {
        let Some(value) = effective_env_value(env, key) else {
            return Err(Error::ConfigError(format!(
                "hermetic reproducibility requires {key} path remaps"
            )));
        };
        for token in required {
            if !value.contains(token) {
                return Err(Error::ConfigError(format!(
                    "hermetic reproducibility requires {key} to include required remap flag {token}"
                )));
            }
        }
        Ok(())
    }

    fn rust_remaps(&self) -> Vec<String> {
        vec![
            format!(
                "--remap-path-prefix={}=/build/source",
                self.source_root.display()
            ),
            format!("--remap-path-prefix={}=/build", self.build_root.display()),
        ]
    }

    fn file_prefix_remaps(&self) -> Vec<String> {
        vec![
            format!(
                "-ffile-prefix-map={}=/build/source",
                self.source_root.display()
            ),
            format!("-ffile-prefix-map={}=/build", self.build_root.display()),
        ]
    }
}

fn effective_env_value<'a>(env: &'a [(String, String)], key: &str) -> Option<&'a str> {
    env.iter()
        .rev()
        .find(|(candidate, _)| candidate == key)
        .map(|(_, value)| value.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn reproducibility_env_sets_source_date_epoch_and_path_maps() {
        let source = Path::new("/tmp/conary/source");
        let build = Path::new("/tmp/conary/build");
        let config = ReproducibilityConfig::new(123, source, build);

        let env = config.env_vars();

        assert!(
            env.iter()
                .any(|(k, v)| k == "SOURCE_DATE_EPOCH" && v == "123")
        );
        assert!(env.iter().any(|(k, v)| {
            k == "RUSTFLAGS"
                && v.contains("--remap-path-prefix=/tmp/conary/source=/build/source")
                && v.contains("--remap-path-prefix=/tmp/conary/build=/build")
        }));
        assert!(env.iter().any(|(k, v)| {
            k == "CFLAGS"
                && v.contains("-ffile-prefix-map=/tmp/conary/source=/build/source")
                && v.contains("-ffile-prefix-map=/tmp/conary/build=/build")
        }));
        assert!(env.iter().any(|(k, v)| {
            k == "CXXFLAGS"
                && v.contains("-ffile-prefix-map=/tmp/conary/source=/build/source")
                && v.contains("-ffile-prefix-map=/tmp/conary/build=/build")
        }));
    }

    #[test]
    fn final_env_preserves_recipe_flags_and_appends_required_remaps() {
        let source = Path::new("/tmp/conary/source");
        let build = Path::new("/tmp/conary/build");
        let config = ReproducibilityConfig::new(123, source, build);

        let final_env = config
            .merge_env(vec![
                ("RUSTFLAGS".to_string(), "-C target-cpu=native".to_string()),
                ("CFLAGS".to_string(), "-O2".to_string()),
                ("CXXFLAGS".to_string(), "-O3".to_string()),
            ])
            .unwrap();

        let rustflags = final_env
            .iter()
            .find(|(k, _)| k == "RUSTFLAGS")
            .unwrap()
            .1
            .as_str();
        assert!(rustflags.starts_with("-C target-cpu=native "));
        assert!(rustflags.contains("--remap-path-prefix=/tmp/conary/source=/build/source"));
        assert!(rustflags.contains("--remap-path-prefix=/tmp/conary/build=/build"));

        let cflags = final_env
            .iter()
            .find(|(k, _)| k == "CFLAGS")
            .unwrap()
            .1
            .as_str();
        assert!(cflags.starts_with("-O2 "));
        assert!(cflags.contains("-ffile-prefix-map=/tmp/conary/source=/build/source"));
        assert!(cflags.contains("-ffile-prefix-map=/tmp/conary/build=/build"));
    }

    #[test]
    fn final_env_rejects_recipe_source_date_epoch_override() {
        let config = ReproducibilityConfig::new(123, Path::new("/src"), Path::new("/build"));

        let error = config
            .merge_env(vec![("SOURCE_DATE_EPOCH".to_string(), "999".to_string())])
            .unwrap_err();

        assert!(error.to_string().contains("SOURCE_DATE_EPOCH"));
    }

    #[test]
    fn hermetic_env_validation_rejects_missing_required_remap() {
        let config = ReproducibilityConfig::new(123, Path::new("/src"), Path::new("/build"));

        let error = config
            .validate_final_env(&[
                ("SOURCE_DATE_EPOCH".to_string(), "123".to_string()),
                ("RUSTFLAGS".to_string(), "-C opt-level=2".to_string()),
                (
                    "CFLAGS".to_string(),
                    "-ffile-prefix-map=/src=/build/source -ffile-prefix-map=/build=/build"
                        .to_string(),
                ),
                (
                    "CXXFLAGS".to_string(),
                    "-ffile-prefix-map=/src=/build/source -ffile-prefix-map=/build=/build"
                        .to_string(),
                ),
            ])
            .unwrap_err();

        assert!(error.to_string().contains("RUSTFLAGS"));
        assert!(error.to_string().contains("remap-path-prefix"));
    }

    #[test]
    fn default_reproducibility_config_records_planned_controls() {
        let config = ReproducibilityConfig::default();

        assert_eq!(
            config.record(),
            ReproducibilityRecord {
                source_date_epoch: Some(0),
                path_remap_count: 2,
                env_keys: vec![
                    "CFLAGS".to_string(),
                    "CXXFLAGS".to_string(),
                    "RUSTFLAGS".to_string(),
                    "SOURCE_DATE_EPOCH".to_string(),
                ],
            }
        );
    }
}
