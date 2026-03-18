// conary-core/src/bootstrap/build_helpers.rs

//! Shared build utilities for bootstrap builders
//!
//! Common operations shared across bootstrap phases: extract tarballs,
//! find source directories, expand environment variables, set up sandbox
//! environments, and run shell commands.
//!
//! Some functions are not yet called by the current phase implementations
//! but are retained for use when recipe-driven execution is wired end-to-end.

use crate::container::{BindMount, ContainerConfig, Sandbox};
use crate::recipe::Recipe;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::debug;

/// Determine the tar flag based on archive filename extension
pub fn tar_flag_for_archive(filename: &str) -> &'static str {
    if filename.ends_with(".tar.xz") || filename.ends_with(".txz") {
        "xJf"
    } else if filename.ends_with(".tar.gz") || filename.ends_with(".tgz") {
        "xzf"
    } else if filename.ends_with(".tar.bz2") || filename.ends_with(".tbz2") {
        "xjf"
    } else if filename.ends_with(".tar.zst") || filename.ends_with(".tzst") {
        "--zstd -xf"
    } else {
        "xf"
    }
}

/// Extract a tar archive to a destination directory
///
/// If `strip_components` is true, strips the top-level directory from the
/// archive (useful for in-tree dependencies like GMP, MPFR, MPC).
pub fn extract_tar(archive: &Path, dest: &Path, strip_components: bool) -> Result<(), String> {
    fs::create_dir_all(dest).map_err(|e| e.to_string())?;

    let archive_str = archive
        .to_str()
        .ok_or_else(|| format!("archive path is not valid UTF-8: {}", archive.display()))?;
    let dest_str = dest
        .to_str()
        .ok_or_else(|| format!("dest path is not valid UTF-8: {}", dest.display()))?;
    let filename = archive
        .file_name()
        .ok_or_else(|| format!("archive path has no filename: {}", archive.display()))?
        .to_string_lossy();

    let flag = tar_flag_for_archive(&filename);

    let mut cmd = Command::new("tar");
    for part in flag.split_whitespace() {
        cmd.arg(part);
    }
    cmd.args([archive_str, "-C", dest_str]);
    if strip_components {
        cmd.arg("--strip-components=1");
    }

    let output = cmd.output().map_err(|e| e.to_string())?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("tar extraction failed: {stderr}"));
    }

    Ok(())
}

/// Find the actual source directory after extraction
///
/// If the archive extracted into a single top-level directory, returns that
/// directory. Otherwise returns the extraction directory itself.
pub fn find_source_dir(dir: &Path) -> Result<PathBuf, std::io::Error> {
    let entries: Vec<_> = fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .collect();

    if entries.len() == 1 {
        Ok(entries[0].path())
    } else {
        Ok(dir.to_path_buf())
    }
}

/// Expand environment variables in a string.
///
/// Variables use `${VAR}` and `$VAR` syntax. Looks up values in `build_env` only.
/// Variables not found in `build_env` expand to empty string (host
/// environment is intentionally not consulted for build hermiticity).
#[allow(dead_code)]
pub fn expand_env_vars(value: &str, build_env: &HashMap<String, String>) -> String {
    let mut result = value.to_string();

    // Handle ${VAR} syntax
    while let Some(start) = result.find("${") {
        if let Some(end) = result[start..].find('}') {
            let var_name = &result[start + 2..start + end];
            let replacement = build_env.get(var_name).cloned().unwrap_or_default();
            result = format!(
                "{}{}{}",
                &result[..start],
                replacement,
                &result[start + end + 1..]
            );
        } else {
            break;
        }
    }

    // Handle $VAR syntax (must come after ${VAR} to avoid conflicts)
    let mut i = 0;
    while i < result.len() {
        if result[i..].starts_with('$') && !result[i..].starts_with("${") {
            let rest = &result[i + 1..];
            let var_end = rest
                .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .unwrap_or(rest.len());
            if var_end > 0 {
                let var_name = &rest[..var_end];
                let replacement = build_env.get(var_name).cloned().unwrap_or_default();
                result = format!(
                    "{}{}{}",
                    &result[..i],
                    replacement,
                    &result[i + 1 + var_end..]
                );
                i += replacement.len();
                continue;
            }
        }
        i += 1;
    }

    result
}

/// Merge environment variables for a build, combining base env, cross-compilation
/// env, and recipe-specific env.
///
/// Flag variables (CFLAGS, CXXFLAGS, LDFLAGS) are merged by prepending base
/// flags. All other variables are replaced.
#[allow(dead_code)]
pub fn merge_build_env(
    base_env: &HashMap<String, String>,
    cross_env: HashMap<String, String>,
    recipe_env: HashMap<String, String>,
    build_env_for_expansion: &HashMap<String, String>,
) -> Vec<(String, String)> {
    let mut env_vec: Vec<(String, String)> = Vec::new();

    // Add base environment
    for (key, value) in base_env {
        env_vec.push((key.clone(), value.clone()));
    }

    // Add cross-compilation environment
    for (key, value) in cross_env {
        if key == "CFLAGS" || key == "CXXFLAGS" || key == "LDFLAGS" {
            let base_flags = base_env.get(&key).map(|s| s.as_str()).unwrap_or("");
            let merged = format!("{} {}", base_flags, value);
            env_vec.retain(|(k, _)| k != &key);
            env_vec.push((key, merged.trim().to_string()));
        } else {
            env_vec.retain(|(k, _)| k != &key);
            env_vec.push((key, value));
        }
    }

    // Add recipe environment
    for (key, value) in recipe_env {
        let expanded = expand_env_vars(&value, build_env_for_expansion);
        env_vec.retain(|(k, _)| k != &key);
        env_vec.push((key, expanded));
    }

    env_vec
}

/// Substitute standard build variables in a command string
///
/// Replaces `%(target)s`, `%(jobs)s`, `%(stage1_sysroot)s`, and
/// cross-compilation section variables from the recipe.
#[allow(dead_code)]
pub fn substitute_build_vars(
    cmd: &str,
    triple: &str,
    jobs: usize,
    sysroot: &Path,
    recipe: &Recipe,
) -> String {
    let mut result = cmd.to_string();

    result = result.replace("%(target)s", triple);
    result = result.replace("%(jobs)s", &jobs.to_string());
    result = result.replace("%(stage1_sysroot)s", &sysroot.to_string_lossy());

    if let Some(cross) = &recipe.cross {
        if let Some(target) = &cross.target {
            result = result.replace("%(target)s", target);
        }
        if let Some(sysroot) = &cross.sysroot {
            result = result.replace("%(sysroot)s", sysroot);
        }
    }

    result
}

/// Execute a shell command inside a sandboxed environment
///
/// Sets up a bootstrap sandbox with the toolchain mounted at `/tools`,
/// network isolation, and the provided environment variables.
///
/// Returns `(exit_code, stdout, stderr)`.
#[allow(dead_code)]
pub fn run_sandboxed_command(
    cmd: &str,
    workdir: &Path,
    sysroot: &Path,
    sources_dir: &Path,
    build_dir: &Path,
    toolchain_root: &Path,
    env_vec: &[(String, String)],
) -> Result<(i32, String, String), String> {
    let env_refs: Vec<(&str, &str)> = env_vec
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    debug!("Running in sandbox: bash -c \"{}\"", cmd);
    debug!("Workdir: {}", workdir.display());

    let mut config =
        ContainerConfig::pristine_for_bootstrap(sysroot, sources_dir, build_dir, sysroot);

    config.add_bind_mount(BindMount::readonly(toolchain_root, "/tools"));
    config.workdir = workdir.to_path_buf();
    config.deny_network();

    let mut sandbox = Sandbox::new(config);

    sandbox
        .execute("bash", &format!("set -e\n{}", cmd), &[], &env_refs)
        .map_err(|e| e.to_string())
}

/// The standard sysroot directories created for bootstrap stages
#[allow(dead_code)]
pub const SYSROOT_DIRS: &[&str] = &[
    "usr",
    "usr/bin",
    "usr/lib",
    "usr/include",
    "lib",
    "lib64",
    "bin",
];

/// Create the standard sysroot directory structure
#[allow(dead_code)]
pub fn create_sysroot_dirs(sysroot: &Path) -> Result<(), std::io::Error> {
    for dir in SYSROOT_DIRS {
        fs::create_dir_all(sysroot.join(dir))?;
    }
    Ok(())
}

/// Set up the standard build environment for a bootstrap stage
#[allow(dead_code)]
pub fn setup_build_env(
    toolchain: &super::toolchain::Toolchain,
    sysroot: &Path,
) -> HashMap<String, String> {
    let mut build_env = toolchain.env();

    build_env.insert("CFLAGS".to_string(), "-O2 -pipe".to_string());
    build_env.insert("CXXFLAGS".to_string(), "-O2 -pipe".to_string());

    let sysroot_str = sysroot.to_string_lossy().to_string();
    build_env.insert("SYSROOT".to_string(), sysroot_str.clone());

    // Sanitize PATH: build on top of the toolchain's already-sanitized PATH
    // (which uses a fixed fallback rather than the host PATH) and prepend the
    // sysroot's usr/bin so that freshly-built Stage 1 tools take precedence.
    let base_path = build_env
        .get("PATH")
        .cloned()
        .unwrap_or_else(|| super::toolchain::Toolchain::BOOTSTRAP_PATH_FALLBACK.to_string());
    let path = format!("{}/usr/bin:{}", sysroot_str, base_path);
    build_env.insert("PATH".to_string(), path);

    build_env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tar_flag_for_archive() {
        assert_eq!(tar_flag_for_archive("foo.tar.xz"), "xJf");
        assert_eq!(tar_flag_for_archive("foo.txz"), "xJf");
        assert_eq!(tar_flag_for_archive("foo.tar.gz"), "xzf");
        assert_eq!(tar_flag_for_archive("foo.tgz"), "xzf");
        assert_eq!(tar_flag_for_archive("foo.tar.bz2"), "xjf");
        assert_eq!(tar_flag_for_archive("foo.tbz2"), "xjf");
        assert_eq!(tar_flag_for_archive("foo.tar"), "xf");
        assert_eq!(tar_flag_for_archive("foo.tar.zst"), "--zstd -xf");
    }

    #[test]
    fn test_find_source_dir_single() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("foo-1.0")).unwrap();
        let result = find_source_dir(dir.path()).unwrap();
        assert_eq!(result, dir.path().join("foo-1.0"));
    }

    #[test]
    fn test_find_source_dir_multiple() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("a")).unwrap();
        fs::create_dir(dir.path().join("b")).unwrap();
        let result = find_source_dir(dir.path()).unwrap();
        assert_eq!(result, dir.path());
    }

    #[test]
    fn test_expand_env_vars_braced() {
        let mut env = HashMap::new();
        env.insert("FOO".to_string(), "bar".to_string());
        assert_eq!(expand_env_vars("${FOO}/baz", &env), "bar/baz");
    }

    #[test]
    fn test_expand_env_vars_unbraced() {
        let mut env = HashMap::new();
        env.insert("FOO".to_string(), "bar".to_string());
        assert_eq!(expand_env_vars("$FOO/baz", &env), "bar/baz");
    }

    #[test]
    fn test_expand_env_vars_missing() {
        let env = HashMap::new();
        assert_eq!(expand_env_vars("${MISSING}/baz", &env), "/baz");
    }

    #[test]
    fn test_merge_build_env_flag_merging() {
        let mut base = HashMap::new();
        base.insert("CFLAGS".to_string(), "-O2".to_string());
        base.insert("CC".to_string(), "gcc".to_string());

        let mut cross = HashMap::new();
        cross.insert("CFLAGS".to_string(), "-march=native".to_string());
        cross.insert("LD".to_string(), "ld".to_string());

        let result = merge_build_env(&base, cross, HashMap::new(), &base);

        let cflags = result.iter().find(|(k, _)| k == "CFLAGS").unwrap();
        assert_eq!(cflags.1, "-O2 -march=native");

        let ld = result.iter().find(|(k, _)| k == "LD").unwrap();
        assert_eq!(ld.1, "ld");
    }

    #[test]
    fn test_create_sysroot_dirs() {
        let dir = tempfile::tempdir().unwrap();
        create_sysroot_dirs(dir.path()).unwrap();
        assert!(dir.path().join("usr/bin").exists());
        assert!(dir.path().join("usr/lib").exists());
        assert!(dir.path().join("usr/include").exists());
        assert!(dir.path().join("lib64").exists());
    }
}
