// conary-core/src/recipe/hermetic/reproducibility.rs

use crate::recipe::hermetic::evidence::ReproducibilityRecord;
use crate::{Error, Result};
use std::path::{Path, PathBuf};

const SOURCE_DATE_EPOCH: &str = "SOURCE_DATE_EPOCH";
const RUSTFLAGS: &str = "RUSTFLAGS";
const CFLAGS: &str = "CFLAGS";
const CXXFLAGS: &str = "CXXFLAGS";
const SHELLOPTS: &str = "SHELLOPTS";
const BASHOPTS: &str = "BASHOPTS";
const BASH_ENV: &str = "BASH_ENV";
const ENV: &str = "ENV";
const BASH_FUNC_PREFIX: &str = "BASH_FUNC_";
const MAKEFLAGS: &str = "MAKEFLAGS";
const GNUMAKEFLAGS: &str = "GNUMAKEFLAGS";
const MAKEOVERRIDES: &str = "MAKEOVERRIDES";
const MAKEFILES: &str = "MAKEFILES";
const PATH_REMAP_COUNT: usize = 2;
const CONTROLLED_ENV_KEYS: &[&str] = &[
    SOURCE_DATE_EPOCH,
    RUSTFLAGS,
    CFLAGS,
    CXXFLAGS,
    SHELLOPTS,
    BASHOPTS,
    BASH_ENV,
    ENV,
];
const SHELL_STARTUP_ENV_KEYS: &[&str] = &[SHELLOPTS, BASHOPTS, BASH_ENV, ENV];
const MAKE_ENV_KEYS: &[&str] = &[MAKEFLAGS, GNUMAKEFLAGS, MAKEOVERRIDES, MAKEFILES];

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
        validate_no_shell_startup_env(&recipe_env)?;
        validate_make_environment_values(&recipe_env)?;
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
        validate_no_shell_startup_env(env)?;
        validate_make_environment_values(env)?;

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
        CONTROLLED_ENV_KEYS
    }

    pub(crate) fn is_forbidden_shell_environment_key(key: &str) -> bool {
        is_forbidden_shell_environment_key(key)
    }

    pub(crate) fn validate_make_environment_value(key: &str, value: &str) -> Result<()> {
        validate_make_environment_value(key, value)
    }

    pub(crate) fn is_make_environment_key(key: &str) -> bool {
        is_make_environment_key(key)
    }

    pub(crate) fn controlled_make_assignment_key(token: &str) -> Option<&str> {
        controlled_make_assignment_key(token)
    }

    pub(crate) fn is_make_eval_option(token: &str) -> bool {
        is_make_eval_option(token)
    }

    pub(crate) fn is_makefile_import_option(token: &str) -> bool {
        is_makefile_import_option(token)
    }

    pub(crate) fn command_local_assignment_allowed(&self, key: &str, value: &str) -> bool {
        match key {
            SOURCE_DATE_EPOCH => false,
            SHELLOPTS | BASHOPTS | BASH_ENV | ENV => false,
            MAKEFLAGS | GNUMAKEFLAGS | MAKEOVERRIDES | MAKEFILES => {
                validate_make_environment_value(key, value).is_ok()
            }
            RUSTFLAGS => self
                .rust_remaps()
                .iter()
                .all(|required| has_exact_flag(value, required)),
            CFLAGS | CXXFLAGS => self
                .file_prefix_remaps()
                .iter()
                .all(|required| has_exact_flag(value, required)),
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
            if !has_exact_flag(value, token) {
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

fn validate_no_shell_startup_env(env: &[(String, String)]) -> Result<()> {
    for (key, _) in env {
        if is_forbidden_shell_environment_key(key) {
            return Err(Error::ConfigError(format!(
                "hermetic reproducibility rejects shell startup/import environment variable {key}; recipe or extra environment cannot set it"
            )));
        }
    }
    Ok(())
}

fn validate_make_environment_values(env: &[(String, String)]) -> Result<()> {
    for (key, value) in env {
        validate_make_environment_value(key, value)?;
    }
    Ok(())
}

fn validate_make_environment_value(key: &str, value: &str) -> Result<()> {
    if !is_make_environment_key(key) {
        return Ok(());
    }
    if key == MAKEFILES {
        return Err(Error::ConfigError(
            "hermetic reproducibility rejects MAKEFILES make startup import environment"
                .to_string(),
        ));
    }
    if value.contains('$') {
        return Err(Error::ConfigError(format!(
            "hermetic reproducibility rejects {key} make environment value with shell expansion"
        )));
    }
    for token in value.split_whitespace() {
        let token = clean_make_token(token);
        if is_make_eval_option(token) {
            return Err(Error::ConfigError(format!(
                "hermetic reproducibility rejects {key} make eval option {token}"
            )));
        }
        if is_makefile_import_option(token) {
            return Err(Error::ConfigError(format!(
                "hermetic reproducibility rejects {key} makefile import option {token}"
            )));
        }
        if let Some(controlled) = controlled_make_assignment_key(token) {
            return Err(Error::ConfigError(format!(
                "hermetic reproducibility rejects {key} assignment to controlled make variable {controlled}"
            )));
        }
    }
    Ok(())
}

fn is_make_environment_key(key: &str) -> bool {
    MAKE_ENV_KEYS.contains(&key)
}

fn is_forbidden_shell_environment_key(key: &str) -> bool {
    SHELL_STARTUP_ENV_KEYS.contains(&key) || key.starts_with(BASH_FUNC_PREFIX)
}

fn controlled_make_assignment_key(token: &str) -> Option<&str> {
    let (target, value) = make_assignment(token)?;
    if is_make_environment_key(target) && validate_make_environment_value(target, value).is_err() {
        return Some(target);
    }
    if is_forbidden_shell_environment_key(target)
        || CONTROLLED_ENV_KEYS
            .iter()
            .any(|controlled| *controlled == target)
    {
        return Some(target);
    }
    None
}

fn make_assignment(token: &str) -> Option<(&str, &str)> {
    let token = clean_make_token(token);
    for operator in ["::=", "+=", ":=", "?=", "!=", "="] {
        let Some((target, value)) = token.split_once(operator) else {
            continue;
        };
        let target = target.trim();
        if !target.is_empty() {
            return Some((target, value));
        }
    }
    None
}

fn is_make_eval_option(token: &str) -> bool {
    let token = clean_make_token(token);
    long_make_option_matches(token, "eval", 2) || short_make_option_bundle_contains(token, 'E')
}

fn is_makefile_import_option(token: &str) -> bool {
    let token = clean_make_token(token);
    short_make_option_bundle_contains(token, 'f')
        || short_make_option_bundle_contains(token, 'I')
        || long_make_option_matches(token, "file", 1)
        || long_make_option_matches(token, "makefile", 3)
        || long_make_option_matches(token, "include-dir", 3)
}

fn long_make_option_matches(token: &str, option: &str, min_prefix_len: usize) -> bool {
    let Some(token) = token.strip_prefix("--") else {
        return false;
    };
    let name = token.split_once('=').map_or(token, |(name, _)| name);
    name.len() >= min_prefix_len && option.starts_with(name)
}

fn short_make_option_bundle_contains(token: &str, option: char) -> bool {
    token.starts_with('-')
        && !token.starts_with("--")
        && token[1..].chars().any(|candidate| candidate == option)
}

fn clean_make_token(token: &str) -> &str {
    token.trim_matches(|ch| matches!(ch, '"' | '\''))
}

fn effective_env_value<'a>(env: &'a [(String, String)], key: &str) -> Option<&'a str> {
    env.iter()
        .rev()
        .find(|(candidate, _)| candidate == key)
        .map(|(_, value)| value.as_str())
}

fn has_exact_flag(value: &str, required: &str) -> bool {
    value.split_whitespace().any(|token| token == required)
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
    fn merge_env_rejects_shell_startup_environment_controls() {
        let config = ReproducibilityConfig::new(123, Path::new("/src"), Path::new("/build"));
        let cases = [
            ("SHELLOPTS", "keyword"),
            ("BASHOPTS", "expand_aliases"),
            ("BASH_ENV", "/tmp/env.sh"),
            ("ENV", "/tmp/env.sh"),
            ("BASH_FUNC_true%%", "() { SOURCE_DATE_EPOCH=999 true; }"),
        ];

        for (key, value) in cases {
            let error = config
                .merge_env(vec![(key.to_string(), value.to_string())])
                .unwrap_err();

            assert!(
                error.to_string().contains(key),
                "expected {key} rejection, got: {error}"
            );
        }
    }

    #[test]
    fn hermetic_env_validation_rejects_shell_startup_environment_controls() {
        let config = ReproducibilityConfig::new(123, Path::new("/src"), Path::new("/build"));
        let cases = [
            ("SHELLOPTS", "keyword"),
            ("BASHOPTS", "expand_aliases"),
            ("BASH_FUNC_true%%", "() { SOURCE_DATE_EPOCH=999 true; }"),
        ];

        for (key, value) in cases {
            let mut env = config.env_vars();
            env.push((key.to_string(), value.to_string()));

            let error = config.validate_final_env(&env).unwrap_err();

            assert!(
                error.to_string().contains(key),
                "expected {key} rejection, got: {error}"
            );
        }
    }

    #[test]
    fn make_environment_controls_allow_jobs_and_reject_controlled_assignments() {
        let config = ReproducibilityConfig::new(123, Path::new("/src"), Path::new("/build"));

        let safe_env = config
            .merge_env(vec![("MAKEFLAGS".to_string(), "-j8".to_string())])
            .unwrap();
        config.validate_final_env(&safe_env).unwrap();

        let cases = [
            ("MAKEFLAGS", "SOURCE_DATE_EPOCH=999"),
            ("GNUMAKEFLAGS", "RUSTFLAGS+=bad"),
            ("MAKEOVERRIDES", "CFLAGS:=bad"),
            ("MAKEFLAGS", "CXXFLAGS?=bad"),
            ("GNUMAKEFLAGS", "SOURCE_DATE_EPOCH!=date"),
            ("MAKEOVERRIDES", "RUSTFLAGS::=bad"),
            ("MAKEFLAGS", "MAKEFLAGS=SOURCE_DATE_EPOCH=999"),
            ("MAKEFILES", "evil.mk"),
            ("MAKEFLAGS", "--file=evil.mk"),
            ("GNUMAKEFLAGS", "-fevil.mk"),
            ("MAKEFLAGS", "-rfevil.mk"),
            ("MAKEFLAGS", "-rEexport SOURCE_DATE_EPOCH=999"),
            ("MAKEFLAGS", "--ev=export CFLAGS=bad"),
            ("GNUMAKEFLAGS", "--fi=evil.mk"),
            ("MAKEFLAGS", "--mak=evil.mk"),
            ("MAKEFLAGS", "-Ievil"),
            ("GNUMAKEFLAGS", "--include-dir=evil"),
            ("MAKEFLAGS", "--inc=evil"),
            ("MAKEFLAGS", "$BAD"),
        ];

        for (key, value) in cases {
            let merge_error = config
                .merge_env(vec![(key.to_string(), value.to_string())])
                .unwrap_err();
            assert!(
                merge_error.to_string().contains(key),
                "expected {key} merge rejection, got: {merge_error}"
            );

            let mut env = config.env_vars();
            env.push((key.to_string(), value.to_string()));
            let validation_error = config.validate_final_env(&env).unwrap_err();
            assert!(
                validation_error.to_string().contains(key),
                "expected {key} validation rejection, got: {validation_error}"
            );
        }
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
    fn hermetic_env_validation_rejects_prefix_extension_remaps() {
        let config = ReproducibilityConfig::new(123, Path::new("/src"), Path::new("/build"));

        let rust_error = config
            .validate_final_env(&[
                ("SOURCE_DATE_EPOCH".to_string(), "123".to_string()),
                (
                    "RUSTFLAGS".to_string(),
                    "--remap-path-prefix=/src=/build/source-old --remap-path-prefix=/build=/build"
                        .to_string(),
                ),
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
        assert!(rust_error.to_string().contains("RUSTFLAGS"));

        let c_error = config
            .validate_final_env(&[
                ("SOURCE_DATE_EPOCH".to_string(), "123".to_string()),
                (
                    "RUSTFLAGS".to_string(),
                    "--remap-path-prefix=/src=/build/source --remap-path-prefix=/build=/build"
                        .to_string(),
                ),
                (
                    "CFLAGS".to_string(),
                    "-ffile-prefix-map=/src=/build/source-old -ffile-prefix-map=/build=/build"
                        .to_string(),
                ),
                (
                    "CXXFLAGS".to_string(),
                    "-ffile-prefix-map=/src=/build/source -ffile-prefix-map=/build=/build"
                        .to_string(),
                ),
            ])
            .unwrap_err();
        assert!(c_error.to_string().contains("CFLAGS"));
    }

    #[test]
    fn command_local_assignment_rejects_prefix_extension_remap() {
        let config = ReproducibilityConfig::new(123, Path::new("/src"), Path::new("/build"));

        assert!(!config.command_local_assignment_allowed(
            "RUSTFLAGS",
            "--remap-path-prefix=/src=/build/source-old --remap-path-prefix=/build=/build",
        ));
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
