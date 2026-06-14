// apps/conary/src/commands/hermetic_config.rs

//! Command-owned hermetic builder configuration.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use conary_core::recipe::Recipe;
use conary_core::recipe::hermetic::{BuilderEnvironmentIdentity, BuilderEnvironmentKind};
use serde::Deserialize;

#[derive(Debug, Clone)]
pub(crate) struct HermeticBuilder {
    pub(crate) identity: BuilderEnvironmentIdentity,
    pub(crate) sysroot_path: PathBuf,
}

#[derive(Debug, Deserialize)]
struct HermeticConfigFile {
    default_builder: String,
    builders: BTreeMap<String, BuilderConfigFile>,
}

#[derive(Debug, Deserialize)]
struct BuilderConfigFile {
    kind: String,
    sysroot_path: PathBuf,
    #[serde(default)]
    sysroot_hash: Option<String>,
    #[serde(default)]
    toolchain_hash: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

pub(crate) fn load_default_hermetic_builder() -> Result<HermeticBuilder> {
    let path = resolve_default_config_path()?;
    load_default_hermetic_builder_from_path(&path)
}

pub(crate) fn load_default_hermetic_builder_from_path(
    path: impl AsRef<Path>,
) -> Result<HermeticBuilder> {
    let path = path.as_ref();
    let canonical_config = path
        .canonicalize()
        .with_context(|| format!("hermetic config is required at {}", path.display()))?;
    check_config_file_policy(&canonical_config)
        .with_context(|| format!("hermetic config policy check failed for {}", path.display()))?;

    let content = std::fs::read_to_string(&canonical_config)
        .with_context(|| format!("read hermetic config {}", path.display()))?;
    let parsed: HermeticConfigFile = toml::from_str(&content)
        .with_context(|| format!("parse hermetic config {}", path.display()))?;

    let builder = parsed
        .builders
        .get(&parsed.default_builder)
        .with_context(|| {
            format!(
                "hermetic config {} references unknown default_builder {:?}",
                path.display(),
                parsed.default_builder
            )
        })?;

    if builder.kind != "pristine" {
        bail!(
            "hermetic config {} builder {:?} has unsupported kind {:?}; M2a accepts only \"pristine\"",
            path.display(),
            parsed.default_builder,
            builder.kind
        );
    }

    if builder.sysroot_hash.is_none() && builder.toolchain_hash.is_none() {
        bail!(
            "hermetic config {} builder {:?} must set sysroot_hash or toolchain_hash",
            path.display(),
            parsed.default_builder
        );
    }

    validate_hash_field(
        path,
        &parsed.default_builder,
        "sysroot_hash",
        &builder.sysroot_hash,
    )?;
    validate_hash_field(
        path,
        &parsed.default_builder,
        "toolchain_hash",
        &builder.toolchain_hash,
    )?;

    let sysroot_path = builder.sysroot_path.canonicalize().with_context(|| {
        format!(
            "configured sysroot_path {} from hermetic config {} must exist",
            builder.sysroot_path.display(),
            path.display()
        )
    })?;
    if !sysroot_path.is_dir() {
        bail!(
            "configured sysroot_path {} from hermetic config {} is not a directory",
            sysroot_path.display(),
            path.display()
        );
    }
    check_sysroot_policy(&sysroot_path).with_context(|| {
        format!(
            "hermetic sysroot policy check failed for {} from {}",
            sysroot_path.display(),
            path.display()
        )
    })?;

    let _description = builder.description.as_deref();

    Ok(HermeticBuilder {
        identity: BuilderEnvironmentIdentity {
            kind: BuilderEnvironmentKind::Pristine,
            sysroot_hash: builder.sysroot_hash.clone(),
            toolchain_hash: builder.toolchain_hash.clone(),
            diagnostics: Vec::new(),
        },
        sysroot_path,
    })
}

pub(crate) fn ensure_no_build_dependencies_for_m2a(recipe: &Recipe) -> Result<()> {
    let deps = recipe.all_build_deps();
    if deps.is_empty() {
        return Ok(());
    }

    bail!(
        "recipe declares build dependencies ({}) but M2a hermetic cook/publish refuses them until dependency content locks are available",
        deps.join(", ")
    );
}

fn resolve_default_config_path() -> Result<PathBuf> {
    resolve_default_config_path_with(|key| std::env::var_os(key))
}

fn resolve_default_config_path_with(
    mut var: impl FnMut(&str) -> Option<OsString>,
) -> Result<PathBuf> {
    if let Some(path) = non_empty_os(var("CONARY_HERMETIC_CONFIG")) {
        return Ok(PathBuf::from(path));
    }

    if let Some(config_home) = non_empty_os(var("XDG_CONFIG_HOME")) {
        return Ok(PathBuf::from(config_home)
            .join("conary")
            .join("hermetic.toml"));
    }

    if let Some(home) = non_empty_os(var("HOME")) {
        return Ok(PathBuf::from(home)
            .join(".config")
            .join("conary")
            .join("hermetic.toml"));
    }

    bail!(
        "cannot determine hermetic config path; set CONARY_HERMETIC_CONFIG, XDG_CONFIG_HOME, or HOME"
    );
}

fn non_empty_os(value: Option<OsString>) -> Option<OsString> {
    value.filter(|value| !value.is_empty())
}

fn validate_hash_field(
    config_path: &Path,
    builder_name: &str,
    field: &str,
    value: &Option<String>,
) -> Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    let rest = value.strip_prefix("sha256:").with_context(|| {
        format!(
            "hermetic config {} builder {:?} field {field} must be sha256:<64 hex>",
            config_path.display(),
            builder_name
        )
    })?;
    if rest.len() != 64 || !rest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!(
            "hermetic config {} builder {:?} field {field} must be sha256:<64 hex>",
            config_path.display(),
            builder_name
        );
    }
    Ok(())
}

#[cfg(unix)]
fn check_config_file_policy(path: &Path) -> Result<()> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("read metadata for hermetic config {}", path.display()))?;
    ensure_owned_by_current_user_or_root(path, &metadata)?;
    ensure_not_group_or_world_writable(path, &metadata)?;
    if let Some(parent) = path.parent() {
        check_directory_trust_chain(parent)?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_config_file_policy(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn check_sysroot_policy(path: &Path) -> Result<()> {
    check_directory_trust_chain(path)
}

#[cfg(not(unix))]
fn check_sysroot_policy(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn check_directory_trust_chain(start: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut current = start.to_path_buf();
    let mut first = true;
    loop {
        let metadata = std::fs::metadata(&current)
            .with_context(|| format!("read metadata for {}", current.display()))?;
        ensure_owned_by_current_user_or_root(&current, &metadata)?;
        let mode = metadata.permissions().mode();
        let writable = mode & 0o022 != 0;
        let sticky = mode & 0o1000 != 0;
        if writable {
            if !first && sticky {
                break;
            }
            bail!("{} must not be group- or world-writable", current.display());
        }

        let Some(parent) = current.parent() else {
            break;
        };
        if parent == current {
            break;
        }
        current = parent.to_path_buf();
        first = false;
    }

    Ok(())
}

#[cfg(unix)]
fn ensure_owned_by_current_user_or_root(path: &Path, metadata: &std::fs::Metadata) -> Result<()> {
    use std::os::unix::fs::MetadataExt;

    let owner = metadata.uid();
    let current = nix::unistd::Uid::effective().as_raw();
    if owner == 0 || owner == current {
        return Ok(());
    }

    bail!(
        "{} must be owned by the current user or root (uid {}, current uid {})",
        path.display(),
        owner,
        current
    );
}

#[cfg(unix)]
fn ensure_not_group_or_world_writable(path: &Path, metadata: &std::fs::Metadata) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    if metadata.permissions().mode() & 0o022 == 0 {
        return Ok(());
    }

    bail!("{} must not be group- or world-writable", path.display());
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::recipe::parse_recipe;
    use std::ffi::OsString;

    const HASH_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const HASH_B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn write_config(path: &Path, sysroot: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(sysroot).unwrap();
        std::fs::write(path, body.replace("{SYSROOT}", &sysroot.to_string_lossy())).unwrap();
    }

    fn valid_config(sysroot: &Path) -> String {
        format!(
            r#"
default_builder = "native"

[builders.native]
kind = "pristine"
sysroot_path = "{}"
sysroot_hash = "{HASH_A}"
toolchain_hash = "{HASH_B}"
description = "test builder"
"#,
            sysroot.display()
        )
    }

    #[test]
    fn config_path_resolution_prefers_explicit_env_over_xdg_and_home() {
        let path = resolve_default_config_path_with(|key| match key {
            "CONARY_HERMETIC_CONFIG" => Some(OsString::from("/explicit/hermetic.toml")),
            "XDG_CONFIG_HOME" => Some(OsString::from("/xdg")),
            "HOME" => Some(OsString::from("/home/test")),
            _ => None,
        })
        .unwrap();

        assert_eq!(path, PathBuf::from("/explicit/hermetic.toml"));
    }

    #[test]
    fn config_path_resolution_uses_xdg_before_home() {
        let path = resolve_default_config_path_with(|key| match key {
            "XDG_CONFIG_HOME" => Some(OsString::from("/xdg")),
            "HOME" => Some(OsString::from("/home/test")),
            _ => None,
        })
        .unwrap();

        assert_eq!(path, PathBuf::from("/xdg/conary/hermetic.toml"));
    }

    #[test]
    fn explicit_path_loads_valid_pristine_builder() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("hermetic.toml");
        let sysroot = temp.path().join("sysroot");
        write_config(&config, &sysroot, &valid_config(&sysroot));

        let builder = load_default_hermetic_builder_from_path(&config).unwrap();

        assert_eq!(builder.sysroot_path, sysroot.canonicalize().unwrap());
        assert_eq!(builder.identity.kind, BuilderEnvironmentKind::Pristine);
        assert_eq!(builder.identity.sysroot_hash.as_deref(), Some(HASH_A));
        assert_eq!(builder.identity.toolchain_hash.as_deref(), Some(HASH_B));
    }

    #[test]
    fn missing_default_builder_fails_closed() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("hermetic.toml");
        let sysroot = temp.path().join("sysroot");
        write_config(
            &config,
            &sysroot,
            r#"
[builders.native]
kind = "pristine"
sysroot_path = "{SYSROOT}"
sysroot_hash = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
"#,
        );

        let error = load_default_hermetic_builder_from_path(&config).unwrap_err();

        assert!(
            format!("{error:#}").contains("default_builder"),
            "{error:#}"
        );
    }

    #[test]
    fn unknown_default_builder_fails_closed() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("hermetic.toml");
        let sysroot = temp.path().join("sysroot");
        write_config(
            &config,
            &sysroot,
            r#"
default_builder = "missing"

[builders.native]
kind = "pristine"
sysroot_path = "{SYSROOT}"
sysroot_hash = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
"#,
        );

        let error = load_default_hermetic_builder_from_path(&config).unwrap_err();

        assert!(format!("{error:#}").contains("unknown default_builder"));
    }

    #[test]
    fn invalid_hash_fails_closed() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("hermetic.toml");
        let sysroot = temp.path().join("sysroot");
        write_config(
            &config,
            &sysroot,
            r#"
default_builder = "native"

[builders.native]
kind = "pristine"
sysroot_path = "{SYSROOT}"
sysroot_hash = "sha256:nothex"
"#,
        );

        let error = load_default_hermetic_builder_from_path(&config).unwrap_err();

        assert!(format!("{error:#}").contains("sha256:<64 hex>"));
    }

    #[test]
    fn unsupported_kind_fails_closed() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("hermetic.toml");
        let sysroot = temp.path().join("sysroot");
        write_config(
            &config,
            &sysroot,
            r#"
default_builder = "native"

[builders.native]
kind = "host-mounted"
sysroot_path = "{SYSROOT}"
sysroot_hash = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
"#,
        );

        let error = load_default_hermetic_builder_from_path(&config).unwrap_err();

        assert!(format!("{error:#}").contains("unsupported kind"));
    }

    #[test]
    fn missing_sysroot_fails_closed() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("hermetic.toml");
        let sysroot = temp.path().join("missing-sysroot");
        std::fs::write(&config, valid_config(&sysroot)).unwrap();

        let error = load_default_hermetic_builder_from_path(&config).unwrap_err();

        assert!(format!("{error:#}").contains("sysroot_path"));
    }

    #[test]
    fn build_dependencies_are_refused_until_content_locks_exist() {
        let recipe = parse_recipe(
            r#"
[package]
name = "deps"
version = "1.0"

[source]
path = "."

[build]
requires = ["make"]
makedepends = ["gcc"]
install = "true"
"#,
        )
        .unwrap();

        let error = ensure_no_build_dependencies_for_m2a(&recipe).unwrap_err();
        let error = format!("{error:#}");

        assert!(error.contains("build dependencies"), "{error}");
        assert!(error.contains("content locks"), "{error}");
        assert!(error.contains("make"), "{error}");
        assert!(error.contains("gcc"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn group_writable_config_file_fails_closed() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("hermetic.toml");
        let sysroot = temp.path().join("sysroot");
        write_config(&config, &sysroot, &valid_config(&sysroot));

        let mut permissions = std::fs::metadata(&config).unwrap().permissions();
        permissions.set_mode(0o664);
        std::fs::set_permissions(&config, permissions).unwrap();

        let error = load_default_hermetic_builder_from_path(&config).unwrap_err();

        assert!(format!("{error:#}").contains("group- or world-writable"));
    }

    #[cfg(unix)]
    #[test]
    fn group_writable_sysroot_fails_closed() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("hermetic.toml");
        let sysroot = temp.path().join("sysroot");
        write_config(&config, &sysroot, &valid_config(&sysroot));

        let mut permissions = std::fs::metadata(&sysroot).unwrap().permissions();
        permissions.set_mode(0o775);
        std::fs::set_permissions(&sysroot, permissions).unwrap();

        let error = load_default_hermetic_builder_from_path(&config).unwrap_err();

        assert!(format!("{error:#}").contains("group- or world-writable"));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_config_is_canonicalized_before_policy_checks() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let real_dir = temp.path().join("real");
        let real_config = real_dir.join("hermetic.toml");
        let link_config = temp.path().join("link.toml");
        let sysroot = temp.path().join("sysroot");
        write_config(&real_config, &sysroot, &valid_config(&sysroot));
        symlink(&real_config, &link_config).unwrap();

        let builder = load_default_hermetic_builder_from_path(&link_config).unwrap();

        assert_eq!(builder.sysroot_path, sysroot.canonicalize().unwrap());
    }
}
