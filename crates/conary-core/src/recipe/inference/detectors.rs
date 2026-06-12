// conary-core/src/recipe/inference/detectors.rs

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use crate::error::{Error, Result};
use crate::recipe::format::{
    BuildSection, LocalSourceSection, PackageSection, Recipe, SourceSection,
};

use super::types::{BuildSystem, InferenceOptions, InferenceResult, InferenceTrace};

const DEFAULT_VERSION: &str = "0.1.0";
const DETECTOR_CONFIDENCE: u8 = 100;

pub fn infer_recipe_from_path(
    source_root: &Path,
    options: InferenceOptions,
) -> Result<InferenceResult> {
    let source_root = source_root.canonicalize()?;
    if !source_root.is_dir() {
        return Err(Error::ConfigError(format!(
            "Source root {} is not a directory",
            source_root.display()
        )));
    }

    let matches = run_detectors(&source_root);
    let mut trace = InferenceTrace::new();
    for detector_match in &matches {
        trace.record_detector(
            detector_match.detector,
            detector_match.confidence,
            detector_match.evidence.join(", "),
            detector_match.detail.clone(),
        );
    }

    if matches.is_empty() {
        return Err(Error::ConfigError(format!(
            "No supported build-system markers found in {}. Add a supported build marker \
             (Cargo.toml, CMakeLists.txt, meson.build, configure.ac/configure.in, executable \
             configure, package.json, pyproject.toml/setup.cfg/setup.py, or go.mod) before \
             running `conary new --from .`, or write recipe.toml.",
            source_root.display()
        )));
    }

    let highest_confidence = matches
        .iter()
        .map(|detector_match| detector_match.confidence)
        .max()
        .unwrap_or(0);
    let tied_matches = matches
        .iter()
        .filter(|detector_match| detector_match.confidence == highest_confidence)
        .collect::<Vec<_>>();

    if tied_matches.len() > 1 {
        let tied = tied_matches
            .iter()
            .map(|detector_match| {
                format!(
                    "{} ({})",
                    detector_match.detector,
                    detector_match.evidence.join(", ")
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        return Err(Error::ConfigError(format!(
            "Ambiguous build-system inference in {}: highest confidence {} tied between {}. \
             Add recipe.toml or remove conflicting build markers.",
            source_root.display(),
            highest_confidence,
            tied
        )));
    }

    let selected = tied_matches[0];
    let GeneratedRecipe {
        recipe,
        normalized_name_from,
    } = generate_recipe(&source_root, &options, selected)?;
    if let Some(original_name) = normalized_name_from {
        trace.warn(format!(
            "normalized package name from {original_name:?} to {:?}",
            recipe.package.name
        ));
    }
    trace.record_decision(
        "build-system",
        build_system_slug(selected.build_system),
        format!(
            "highest-confidence detector {} with evidence {}",
            selected.confidence,
            selected.evidence.join(", ")
        ),
    );
    for warning in &selected.warnings {
        trace.warn(warning);
    }

    Ok(InferenceResult {
        build_system: selected.build_system,
        recipe,
        trace,
        source_root,
    })
}

#[derive(Debug, Clone)]
struct DetectorMatch {
    detector: &'static str,
    build_system: BuildSystem,
    confidence: u8,
    evidence: Vec<String>,
    detail: String,
    metadata: PackageMetadata,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Default)]
struct PackageMetadata {
    name: Option<String>,
    version: Option<String>,
    summary: Option<String>,
    license: Option<String>,
    homepage: Option<String>,
}

#[derive(Debug, Clone)]
struct GeneratedRecipe {
    recipe: Recipe,
    normalized_name_from: Option<String>,
}

fn run_detectors(source_root: &Path) -> Vec<DetectorMatch> {
    let mut matches = Vec::new();

    if let Some(detector_match) = detect_cargo(source_root) {
        matches.push(detector_match);
    }
    if let Some(detector_match) = detect_cmake(source_root) {
        matches.push(detector_match);
    }
    if let Some(detector_match) = detect_meson(source_root) {
        matches.push(detector_match);
    }
    if let Some(detector_match) = detect_autotools(source_root) {
        matches.push(detector_match);
    }
    if let Some(detector_match) = detect_npm(source_root) {
        matches.push(detector_match);
    }
    if let Some(detector_match) = detect_python(source_root) {
        matches.push(detector_match);
    }
    if let Some(detector_match) = detect_go(source_root) {
        matches.push(detector_match);
    }

    matches
}

fn detect_cargo(source_root: &Path) -> Option<DetectorMatch> {
    let manifest = source_root.join("Cargo.toml");
    if !manifest.is_file() {
        return None;
    }

    let metadata = fs::read_to_string(&manifest)
        .ok()
        .as_deref()
        .map(parse_cargo_metadata)
        .unwrap_or_default();
    let detail = if metadata.name.is_some() || metadata.version.is_some() {
        "found Cargo.toml package metadata".to_string()
    } else {
        "found Cargo.toml".to_string()
    };

    Some(DetectorMatch {
        detector: "cargo",
        build_system: BuildSystem::Cargo,
        confidence: DETECTOR_CONFIDENCE,
        evidence: vec!["Cargo.toml".to_string()],
        detail,
        metadata,
        warnings: Vec::new(),
    })
}

fn detect_cmake(source_root: &Path) -> Option<DetectorMatch> {
    source_root
        .join("CMakeLists.txt")
        .is_file()
        .then(|| DetectorMatch {
            detector: "cmake",
            build_system: BuildSystem::CMake,
            confidence: DETECTOR_CONFIDENCE,
            evidence: vec!["CMakeLists.txt".to_string()],
            detail: "found CMakeLists.txt".to_string(),
            metadata: PackageMetadata::default(),
            warnings: Vec::new(),
        })
}

fn detect_meson(source_root: &Path) -> Option<DetectorMatch> {
    source_root
        .join("meson.build")
        .is_file()
        .then(|| DetectorMatch {
            detector: "meson",
            build_system: BuildSystem::Meson,
            confidence: DETECTOR_CONFIDENCE,
            evidence: vec!["meson.build".to_string()],
            detail: "found meson.build".to_string(),
            metadata: PackageMetadata::default(),
            warnings: Vec::new(),
        })
}

fn detect_autotools(source_root: &Path) -> Option<DetectorMatch> {
    let configure = source_root.join("configure");
    let has_executable_configure = is_executable_file(&configure);
    let mut evidence = Vec::new();

    for marker in ["configure.ac", "configure.in"] {
        if source_root.join(marker).is_file() {
            evidence.push(marker.to_string());
        }
    }
    if has_executable_configure {
        evidence.push("configure".to_string());
    }

    if evidence.is_empty() {
        return None;
    }

    evidence.sort();
    Some(DetectorMatch {
        detector: "autotools",
        build_system: BuildSystem::Autotools,
        confidence: DETECTOR_CONFIDENCE,
        detail: format!("found {}", evidence.join(", ")),
        evidence,
        metadata: PackageMetadata::default(),
        warnings: Vec::new(),
    })
}

fn detect_npm(source_root: &Path) -> Option<DetectorMatch> {
    let manifest = source_root.join("package.json");
    if !manifest.is_file() {
        return None;
    }

    let package_json = fs::read_to_string(&manifest)
        .ok()
        .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok());
    let metadata = package_json
        .as_ref()
        .map(parse_npm_metadata)
        .unwrap_or_default();
    let mut warnings = vec![
        "npm dependency resolution may use the network in M1b; offline/reproducible npm builds are M2 work."
            .to_string(),
    ];
    if package_json
        .as_ref()
        .and_then(|value| value.get("bin"))
        .and_then(|value| value.as_str())
        .is_some()
    {
        warnings.push(
            "package.json has a string bin entry; automatic wrapper generation is not in M1b."
                .to_string(),
        );
    }

    Some(DetectorMatch {
        detector: "npm",
        build_system: BuildSystem::Npm,
        confidence: DETECTOR_CONFIDENCE,
        evidence: vec!["package.json".to_string()],
        detail: "found package.json".to_string(),
        metadata,
        warnings,
    })
}

fn detect_python(source_root: &Path) -> Option<DetectorMatch> {
    let mut evidence = Vec::new();
    for marker in ["pyproject.toml", "setup.cfg", "setup.py"] {
        if source_root.join(marker).is_file() {
            evidence.push(marker.to_string());
        }
    }

    if evidence.is_empty() {
        return None;
    }

    let metadata = parse_python_metadata(source_root);
    Some(DetectorMatch {
        detector: "python",
        build_system: BuildSystem::Python,
        confidence: DETECTOR_CONFIDENCE,
        detail: format!("found {}", evidence.join(", ")),
        evidence,
        metadata,
        warnings: vec![
            "Python build backends may still use network/build isolation in M1b; offline/reproducible Python builds are M2 work."
                .to_string(),
        ],
    })
}

fn detect_go(source_root: &Path) -> Option<DetectorMatch> {
    let manifest = source_root.join("go.mod");
    if !manifest.is_file() {
        return None;
    }

    let mut metadata = PackageMetadata::default();
    if let Ok(content) = fs::read_to_string(&manifest) {
        metadata.name = parse_go_module_name(&content);
    }

    let vendor_dir = source_root.join("vendor");
    let warnings = if vendor_dir.is_dir() {
        Vec::new()
    } else {
        vec![
            "Go module resolution may use the network when vendor/ is absent in M1b; offline/reproducible Go builds are M2 work."
                .to_string(),
        ]
    };

    Some(DetectorMatch {
        detector: "go",
        build_system: BuildSystem::Go,
        confidence: DETECTOR_CONFIDENCE,
        evidence: vec!["go.mod".to_string()],
        detail: "found go.mod".to_string(),
        metadata,
        warnings,
    })
}

fn generate_recipe(
    source_root: &Path,
    options: &InferenceOptions,
    detector_match: &DetectorMatch,
) -> Result<GeneratedRecipe> {
    let raw_name = options
        .package_name_override
        .clone()
        .or_else(|| detector_match.metadata.name.clone())
        .unwrap_or_else(|| fallback_package_name(source_root));
    let name = normalize_package_name(&raw_name).ok_or_else(|| {
        Error::ConfigError(format!(
            "Invalid inferred package name {raw_name:?}: empty after normalization"
        ))
    })?;
    let normalized_name_from = (name != raw_name).then_some(raw_name);
    let version = options
        .version_override
        .clone()
        .or_else(|| detector_match.metadata.version.clone())
        .unwrap_or_else(|| DEFAULT_VERSION.to_string());
    let summary = detector_match
        .metadata
        .summary
        .clone()
        .unwrap_or_else(|| format!("{name} inferred from {}", detector_match.detector));

    Ok(GeneratedRecipe {
        normalized_name_from,
        recipe: Recipe {
            package: PackageSection {
                name: name.clone(),
                version,
                release: "1".to_string(),
                summary: Some(summary),
                description: None,
                license: detector_match.metadata.license.clone(),
                homepage: detector_match.metadata.homepage.clone(),
            },
            source: SourceSection::Local(LocalSourceSection {
                path: PathBuf::from("."),
            }),
            build: build_section_for(detector_match.build_system, source_root, &name),
            cross: None,
            patches: None,
            components: None,
            variables: HashMap::new(),
        },
    })
}

fn build_section_for(
    build_system: BuildSystem,
    source_root: &Path,
    package_name: &str,
) -> BuildSection {
    match build_system {
        BuildSystem::Cargo => {
            let make = if source_root.join("Cargo.lock").is_file() {
                "cargo build --release --locked"
            } else {
                "cargo build --release"
            };
            build_section(
                None,
                Some(make.to_string()),
                Some(format!(
                    "install -Dm755 target/release/{package_name} %(destdir)s/usr/bin/{package_name}"
                )),
                None,
            )
        }
        BuildSystem::CMake => build_section(
            Some(
                "cmake -S . -B build -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr"
                    .to_string(),
            ),
            Some("cmake --build build".to_string()),
            Some("DESTDIR=%(destdir)s cmake --install build".to_string()),
            None,
        ),
        BuildSystem::Meson => build_section(
            Some("meson setup build --prefix=/usr --buildtype=release".to_string()),
            Some("meson compile -C build".to_string()),
            Some("DESTDIR=%(destdir)s meson install -C build".to_string()),
            None,
        ),
        BuildSystem::Autotools => {
            let setup = if is_executable_file(&source_root.join("configure")) {
                None
            } else {
                Some("autoreconf -fi".to_string())
            };
            build_section(
                Some("./configure --prefix=/usr".to_string()),
                Some("make".to_string()),
                Some("DESTDIR=%(destdir)s make install".to_string()),
                setup,
            )
        }
        BuildSystem::Npm => {
            let make = if source_root.join("package-lock.json").is_file() {
                "npm ci --omit=dev"
            } else {
                "npm install --omit=dev"
            };
            build_section(
                None,
                Some(make.to_string()),
                Some(format!(
                    "mkdir -p \"%(destdir)s/usr/lib/conary/node/{package_name}\" && cp -a . \"%(destdir)s/usr/lib/conary/node/{package_name}/\""
                )),
                None,
            )
        }
        BuildSystem::Python => build_section(
            None,
            None,
            Some("python -m pip install --root %(destdir)s --prefix /usr --no-deps .".to_string()),
            None,
        ),
        BuildSystem::Go => {
            let make = if source_root.join("vendor").is_dir() {
                format!("go build -mod=vendor -trimpath -o {package_name} .")
            } else {
                format!("go build -trimpath -o {package_name} .")
            };
            build_section(
                None,
                Some(make),
                Some(format!(
                    "install -Dm755 {package_name} %(destdir)s/usr/bin/{package_name}"
                )),
                None,
            )
        }
    }
}

fn build_section(
    configure: Option<String>,
    make: Option<String>,
    install: Option<String>,
    setup: Option<String>,
) -> BuildSection {
    BuildSection {
        requires: Vec::new(),
        makedepends: Vec::new(),
        configure,
        make,
        install,
        check: None,
        setup,
        post_install: None,
        environment: HashMap::new(),
        workdir: None,
        script_file: None,
        jobs: None,
        stage: None,
    }
}

fn parse_cargo_metadata(content: &str) -> PackageMetadata {
    let Ok(table) = content.parse::<toml::Table>() else {
        return PackageMetadata::default();
    };
    let Some(package) = table.get("package").and_then(|value| value.as_table()) else {
        return PackageMetadata::default();
    };

    PackageMetadata {
        name: toml_string(package, "name"),
        version: toml_string(package, "version"),
        summary: toml_string(package, "description"),
        license: toml_string(package, "license"),
        homepage: toml_string(package, "homepage"),
    }
}

fn parse_npm_metadata(value: &serde_json::Value) -> PackageMetadata {
    PackageMetadata {
        name: json_string(value, "name"),
        version: json_string(value, "version"),
        summary: json_string(value, "description"),
        license: json_string(value, "license"),
        homepage: json_string(value, "homepage"),
    }
}

fn parse_python_metadata(source_root: &Path) -> PackageMetadata {
    let pyproject = source_root.join("pyproject.toml");
    if pyproject.is_file()
        && let Ok(content) = fs::read_to_string(&pyproject)
    {
        let metadata = parse_pyproject_metadata(&content);
        if metadata.name.is_some() || metadata.version.is_some() || metadata.summary.is_some() {
            return metadata;
        }
    }

    let setup_cfg = source_root.join("setup.cfg");
    if setup_cfg.is_file()
        && let Ok(content) = fs::read_to_string(&setup_cfg)
    {
        let metadata = parse_setup_cfg_metadata(&content);
        if metadata.name.is_some() || metadata.version.is_some() || metadata.summary.is_some() {
            return metadata;
        }
    }

    let setup_py = source_root.join("setup.py");
    if setup_py.is_file()
        && let Ok(content) = fs::read_to_string(&setup_py)
    {
        return parse_setup_py_metadata(&content);
    }

    PackageMetadata::default()
}

fn parse_pyproject_metadata(content: &str) -> PackageMetadata {
    let Ok(table) = content.parse::<toml::Table>() else {
        return PackageMetadata::default();
    };
    let Some(project) = table.get("project").and_then(|value| value.as_table()) else {
        return PackageMetadata::default();
    };

    PackageMetadata {
        name: toml_string(project, "name"),
        version: toml_string(project, "version"),
        summary: toml_string(project, "description"),
        license: toml_license(project),
        homepage: toml_string(project, "homepage"),
    }
}

fn parse_setup_cfg_metadata(content: &str) -> PackageMetadata {
    let mut metadata = PackageMetadata::default();
    let mut in_metadata_section = false;

    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_metadata_section = line.eq_ignore_ascii_case("[metadata]");
            continue;
        }
        if !in_metadata_section {
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim().to_ascii_lowercase().as_str() {
            "name" => metadata.name = non_empty_string(value),
            "version" => metadata.version = non_empty_string(value),
            "description" => metadata.summary = non_empty_string(value),
            "license" => metadata.license = non_empty_string(value),
            "url" | "home_page" | "homepage" => metadata.homepage = non_empty_string(value),
            _ => {}
        }
    }

    metadata
}

fn parse_setup_py_metadata(content: &str) -> PackageMetadata {
    PackageMetadata {
        name: extract_python_string_arg(content, "name"),
        version: extract_python_string_arg(content, "version"),
        summary: extract_python_string_arg(content, "description"),
        license: extract_python_string_arg(content, "license"),
        homepage: extract_python_string_arg(content, "url"),
    }
}

fn parse_go_module_name(content: &str) -> Option<String> {
    content.lines().find_map(|line| {
        let line = line.trim();
        let module_path = line.strip_prefix("module ")?;
        module_path
            .split_whitespace()
            .next()
            .and_then(|path| path.rsplit('/').find(|part| !part.is_empty()))
            .map(str::to_string)
    })
}

fn toml_string(table: &toml::Table, key: &str) -> Option<String> {
    table
        .get(key)
        .and_then(|value| value.as_str())
        .and_then(non_empty_string)
}

fn toml_license(table: &toml::Table) -> Option<String> {
    match table.get("license") {
        Some(toml::Value::String(value)) => non_empty_string(value),
        Some(toml::Value::Table(license)) => toml_string(license, "text"),
        _ => None,
    }
}

fn json_string(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|value| value.as_str())
        .and_then(non_empty_string)
}

fn extract_python_string_arg(content: &str, key: &str) -> Option<String> {
    let mut rest = content;
    while let Some(index) = rest.find(key) {
        let after_key = &rest[index + key.len()..];
        let trimmed = after_key.trim_start();
        if !trimmed.starts_with('=') {
            rest = after_key;
            continue;
        }

        let after_equals = trimmed[1..].trim_start();
        let quote = after_equals.chars().next()?;
        if quote != '"' && quote != '\'' {
            rest = after_equals;
            continue;
        }

        let value_start = quote.len_utf8();
        let value = &after_equals[value_start..];
        if let Some(end) = value.find(quote) {
            return non_empty_string(&value[..end]);
        }
        return None;
    }

    None
}

fn non_empty_string(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn normalize_package_name(raw_name: &str) -> Option<String> {
    let mut normalized = String::new();
    let mut needs_separator = false;

    for character in raw_name.chars() {
        if character.is_ascii_alphanumeric() {
            if needs_separator && !normalized.is_empty() {
                normalized.push('-');
            }
            normalized.push(character.to_ascii_lowercase());
            needs_separator = false;
        } else if !normalized.is_empty() {
            needs_separator = true;
        }
    }

    (!normalized.is_empty()).then_some(normalized)
}

fn fallback_package_name(source_root: &Path) -> String {
    source_root
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(non_empty_string)
        .unwrap_or_else(|| "inferred-package".to_string())
}

fn is_executable_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        fs::metadata(path)
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }

    #[cfg(not(unix))]
    {
        true
    }
}

fn build_system_slug(build_system: BuildSystem) -> &'static str {
    match build_system {
        BuildSystem::Cargo => "cargo",
        BuildSystem::CMake => "cmake",
        BuildSystem::Meson => "meson",
        BuildSystem::Autotools => "autotools",
        BuildSystem::Npm => "npm",
        BuildSystem::Python => "python",
        BuildSystem::Go => "go",
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use crate::recipe::{BuildSystem, InferenceEvent, InferenceOptions, SourceSection};

    use super::infer_recipe_from_path;

    fn inference_options(source_root: &Path, name: &str, version: &str) -> InferenceOptions {
        InferenceOptions {
            source_root: source_root.to_path_buf(),
            package_name_override: Some(name.to_string()),
            version_override: Some(version.to_string()),
        }
    }

    fn warning_messages(result: &crate::recipe::InferenceResult) -> Vec<&str> {
        result
            .trace
            .events
            .iter()
            .filter_map(|event| match event {
                InferenceEvent::Warning { message } => Some(message.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn cargo_detector_uses_package_metadata() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            r#"[package]
name = "hello-conary"
version = "0.2.0"
description = "hello from cargo"
license = "MIT"
"#,
        )
        .unwrap();

        let result =
            infer_recipe_from_path(dir.path(), InferenceOptions::for_source_root(dir.path()))
                .unwrap();

        assert_eq!(result.build_system, BuildSystem::Cargo);
        assert_eq!(result.recipe.package.name, "hello-conary");
        assert_eq!(result.recipe.package.version, "0.2.0");
        assert_eq!(result.recipe.package.release, "1");
        assert_eq!(
            result.recipe.package.summary.as_deref(),
            Some("hello from cargo")
        );
        assert_eq!(result.recipe.package.license.as_deref(), Some("MIT"));
        assert!(matches!(result.recipe.source, SourceSection::Local(_)));
        assert_eq!(
            result.recipe.local_source().unwrap().path,
            std::path::PathBuf::from(".")
        );
        assert!(
            result
                .recipe
                .build
                .make
                .as_deref()
                .unwrap()
                .contains("cargo build --release")
        );
        assert!(
            result
                .recipe
                .build
                .install
                .as_deref()
                .unwrap()
                .contains("%(destdir)s/usr/bin/hello-conary")
        );
    }

    #[test]
    fn cargo_detector_uses_locked_build_when_lockfile_exists() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            r#"[package]
name = "locked-cargo"
version = "1.0.0"
"#,
        )
        .unwrap();
        fs::write(dir.path().join("Cargo.lock"), "").unwrap();

        let result =
            infer_recipe_from_path(dir.path(), InferenceOptions::for_source_root(dir.path()))
                .unwrap();

        assert_eq!(
            result.recipe.build.make.as_deref(),
            Some("cargo build --release --locked")
        );
    }

    #[test]
    fn cmake_detector_generates_cmake_commands() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("CMakeLists.txt"), "project(cmake_app)\n").unwrap();

        let result = infer_recipe_from_path(
            dir.path(),
            inference_options(dir.path(), "cmake-app", "1.2.3"),
        )
        .unwrap();

        assert_eq!(result.build_system, BuildSystem::CMake);
        assert_eq!(result.recipe.package.name, "cmake-app");
        assert_eq!(result.recipe.package.version, "1.2.3");
        assert_eq!(
            result.recipe.package.summary.as_deref(),
            Some("cmake-app inferred from cmake")
        );
        assert_eq!(
            result.recipe.build.configure.as_deref(),
            Some("cmake -S . -B build -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr")
        );
        assert_eq!(
            result.recipe.build.make.as_deref(),
            Some("cmake --build build")
        );
        assert_eq!(
            result.recipe.build.install.as_deref(),
            Some("DESTDIR=%(destdir)s cmake --install build")
        );
    }

    #[test]
    fn meson_detector_generates_meson_commands() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("meson.build"), "project('meson-app')\n").unwrap();

        let result = infer_recipe_from_path(
            dir.path(),
            inference_options(dir.path(), "meson-app", "1.2.3"),
        )
        .unwrap();

        assert_eq!(result.build_system, BuildSystem::Meson);
        assert_eq!(
            result.recipe.build.configure.as_deref(),
            Some("meson setup build --prefix=/usr --buildtype=release")
        );
        assert_eq!(
            result.recipe.build.make.as_deref(),
            Some("meson compile -C build")
        );
        assert_eq!(
            result.recipe.build.install.as_deref(),
            Some("DESTDIR=%(destdir)s meson install -C build")
        );
    }

    #[test]
    fn autotools_detector_recognizes_configure_ac_and_generates_autoreconf_setup() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("configure.ac"),
            "AC_INIT([auto-app], [1.0])\n",
        )
        .unwrap();

        let result = infer_recipe_from_path(
            dir.path(),
            inference_options(dir.path(), "auto-app", "1.2.3"),
        )
        .unwrap();

        assert_eq!(result.build_system, BuildSystem::Autotools);
        assert_eq!(result.recipe.build.setup.as_deref(), Some("autoreconf -fi"));
        assert_eq!(
            result.recipe.build.configure.as_deref(),
            Some("./configure --prefix=/usr")
        );
        assert_eq!(result.recipe.build.make.as_deref(), Some("make"));
        assert_eq!(
            result.recipe.build.install.as_deref(),
            Some("DESTDIR=%(destdir)s make install")
        );
    }

    #[test]
    fn autotools_detector_recognizes_configure_in() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("configure.in"),
            "AC_INIT([auto-in], [1.0])\n",
        )
        .unwrap();

        let result = infer_recipe_from_path(
            dir.path(),
            inference_options(dir.path(), "auto-in", "1.2.3"),
        )
        .unwrap();

        assert_eq!(result.build_system, BuildSystem::Autotools);
        assert_eq!(result.recipe.build.setup.as_deref(), Some("autoreconf -fi"));
    }

    #[cfg(unix)]
    #[test]
    fn autotools_detector_skips_autoreconf_when_configure_is_executable() {
        let dir = tempfile::tempdir().unwrap();
        let configure = dir.path().join("configure");
        fs::write(&configure, "#!/bin/sh\n").unwrap();
        let mut permissions = fs::metadata(&configure).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&configure, permissions).unwrap();

        let result = infer_recipe_from_path(
            dir.path(),
            inference_options(dir.path(), "configured-app", "1.2.3"),
        )
        .unwrap();

        assert_eq!(result.build_system, BuildSystem::Autotools);
        assert!(result.recipe.build.setup.is_none());
    }

    #[test]
    fn npm_detector_uses_package_metadata_and_warns_about_networked_resolution() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{
  "name": "@conary/hello-node",
  "version": "3.4.5",
  "description": "hello from npm",
  "license": "Apache-2.0",
  "bin": "cli.js"
}
"#,
        )
        .unwrap();
        fs::write(dir.path().join("package-lock.json"), "{}\n").unwrap();

        let result =
            infer_recipe_from_path(dir.path(), InferenceOptions::for_source_root(dir.path()))
                .unwrap();

        assert_eq!(result.build_system, BuildSystem::Npm);
        assert_eq!(result.recipe.package.name, "conary-hello-node");
        assert_eq!(result.recipe.package.version, "3.4.5");
        assert_eq!(
            result.recipe.package.summary.as_deref(),
            Some("hello from npm")
        );
        assert_eq!(result.recipe.package.license.as_deref(), Some("Apache-2.0"));
        assert_eq!(
            result.recipe.build.make.as_deref(),
            Some("npm ci --omit=dev")
        );
        assert!(
            result
                .recipe
                .build
                .install
                .as_deref()
                .unwrap()
                .contains("%(destdir)s/usr/lib/conary/node/conary-hello-node")
        );

        let warnings = warning_messages(&result);
        assert!(
            warnings.iter().any(|message| {
                message.contains("normalized package name")
                    && message.contains("@conary/hello-node")
                    && message.contains("conary-hello-node")
            }),
            "expected package name normalization warning, got {warnings:?}"
        );
        assert!(
            warnings.iter().any(|message| {
                message.contains("npm dependency resolution may use the network")
                    && message.contains("offline/reproducible npm builds are M2 work")
            }),
            "expected npm network warning, got {warnings:?}"
        );
        assert!(
            warnings
                .iter()
                .any(|message| message.contains("automatic wrapper generation is not in M1b")),
            "expected npm bin wrapper warning, got {warnings:?}"
        );
    }

    #[test]
    fn unsafe_package_name_override_is_normalized_before_generating_commands() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            r#"[package]
name = "safe-from-metadata"
version = "1.0.0"
"#,
        )
        .unwrap();

        let result = infer_recipe_from_path(
            dir.path(),
            inference_options(dir.path(), "bad;$(name)/pkg", "1.2.3"),
        )
        .unwrap();

        assert_eq!(result.recipe.package.name, "bad-name-pkg");
        assert!(!result.recipe.package.name.contains('/'));
        assert!(!result.recipe.package.name.contains(';'));
        assert!(!result.recipe.package.name.contains('$'));
        let install = result.recipe.build.install.as_deref().unwrap();
        assert!(install.contains("target/release/bad-name-pkg"));
        assert!(install.contains("%(destdir)s/usr/bin/bad-name-pkg"));
        assert!(!install.contains(';'));
        assert!(!install.contains('$'));
        assert!(!install.contains("bad;"));
        assert!(!install.contains("$(name)"));
        assert!(!install.contains("/pkg"));

        let warnings = warning_messages(&result);
        assert!(
            warnings.iter().any(|message| {
                message.contains("normalized package name")
                    && message.contains("bad;$(name)/pkg")
                    && message.contains("bad-name-pkg")
            }),
            "expected package name normalization warning, got {warnings:?}"
        );
    }

    #[test]
    fn package_name_that_normalizes_empty_returns_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            r#"[package]
name = "safe-from-metadata"
version = "1.0.0"
"#,
        )
        .unwrap();

        let error =
            infer_recipe_from_path(dir.path(), inference_options(dir.path(), "///", "1.2.3"))
                .unwrap_err();
        let message = error.to_string();

        assert!(
            message.contains("Invalid inferred package name"),
            "{message}"
        );
        assert!(message.contains("///"), "{message}");
        assert!(message.contains("empty after normalization"), "{message}");
    }

    #[test]
    fn npm_detector_uses_npm_install_without_lockfile() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{"name":"plain-node","version":"1.0.0"}"#,
        )
        .unwrap();

        let result =
            infer_recipe_from_path(dir.path(), InferenceOptions::for_source_root(dir.path()))
                .unwrap();

        assert_eq!(
            result.recipe.build.make.as_deref(),
            Some("npm install --omit=dev")
        );
    }

    #[test]
    fn python_detector_uses_pyproject_metadata_and_warns_about_build_backends() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("pyproject.toml"),
            r#"[project]
name = "hello-python"
version = "2.0.1"
description = "hello from python"
license = "BSD-3-Clause"
"#,
        )
        .unwrap();

        let result =
            infer_recipe_from_path(dir.path(), InferenceOptions::for_source_root(dir.path()))
                .unwrap();

        assert_eq!(result.build_system, BuildSystem::Python);
        assert_eq!(result.recipe.package.name, "hello-python");
        assert_eq!(result.recipe.package.version, "2.0.1");
        assert_eq!(
            result.recipe.package.summary.as_deref(),
            Some("hello from python")
        );
        assert_eq!(
            result.recipe.package.license.as_deref(),
            Some("BSD-3-Clause")
        );
        assert_eq!(
            result.recipe.build.install.as_deref(),
            Some("python -m pip install --root %(destdir)s --prefix /usr --no-deps .")
        );

        let warnings = warning_messages(&result);
        assert!(
            warnings.iter().any(|message| {
                message.contains("Python build backends may still use network/build isolation")
                    && message.contains("offline/reproducible Python builds are M2 work")
            }),
            "expected Python network/build isolation warning, got {warnings:?}"
        );
    }

    #[test]
    fn python_detector_recognizes_setup_cfg() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("setup.cfg"),
            r#"[metadata]
name = cfg-python
version = 7.8.9
description = setup cfg metadata
"#,
        )
        .unwrap();

        let result =
            infer_recipe_from_path(dir.path(), InferenceOptions::for_source_root(dir.path()))
                .unwrap();

        assert_eq!(result.build_system, BuildSystem::Python);
        assert_eq!(result.recipe.package.name, "cfg-python");
        assert_eq!(result.recipe.package.version, "7.8.9");
        assert_eq!(
            result.recipe.package.summary.as_deref(),
            Some("setup cfg metadata")
        );
    }

    #[test]
    fn python_detector_recognizes_setup_py() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("setup.py"),
            r#"from setuptools import setup

setup(name="setup-py-app", version="4.5.6", description="setup py metadata")
"#,
        )
        .unwrap();

        let result =
            infer_recipe_from_path(dir.path(), InferenceOptions::for_source_root(dir.path()))
                .unwrap();

        assert_eq!(result.build_system, BuildSystem::Python);
        assert_eq!(result.recipe.package.name, "setup-py-app");
        assert_eq!(result.recipe.package.version, "4.5.6");
        assert_eq!(
            result.recipe.package.summary.as_deref(),
            Some("setup py metadata")
        );
    }

    #[test]
    fn go_detector_uses_module_name_and_warns_without_vendor() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("go.mod"),
            "module example.com/conary/hello-go\n",
        )
        .unwrap();

        let result =
            infer_recipe_from_path(dir.path(), InferenceOptions::for_source_root(dir.path()))
                .unwrap();

        assert_eq!(result.build_system, BuildSystem::Go);
        assert_eq!(result.recipe.package.name, "hello-go");
        assert_eq!(result.recipe.package.version, "0.1.0");
        assert_eq!(
            result.recipe.build.make.as_deref(),
            Some("go build -trimpath -o hello-go .")
        );
        assert!(
            result
                .recipe
                .build
                .install
                .as_deref()
                .unwrap()
                .contains("%(destdir)s/usr/bin/hello-go")
        );

        let warnings = warning_messages(&result);
        assert!(
            warnings.iter().any(|message| {
                message.contains("Go module resolution may use the network when vendor/ is absent")
                    && message.contains("offline/reproducible Go builds are M2 work")
            }),
            "expected Go module warning, got {warnings:?}"
        );
    }

    #[test]
    fn go_detector_uses_vendor_mode_when_vendor_directory_exists() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("go.mod"), "module example.com/vendor-go\n").unwrap();
        fs::create_dir(dir.path().join("vendor")).unwrap();

        let result =
            infer_recipe_from_path(dir.path(), InferenceOptions::for_source_root(dir.path()))
                .unwrap();

        assert_eq!(
            result.recipe.build.make.as_deref(),
            Some("go build -mod=vendor -trimpath -o vendor-go .")
        );
        let warnings = warning_messages(&result);
        assert!(
            warnings
                .iter()
                .all(|message| !message.contains("Go module resolution may use the network")),
            "vendor mode should not emit Go network warning"
        );
    }

    #[test]
    fn ambiguous_same_confidence_trees_fail_and_name_tied_detectors() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("CMakeLists.txt"), "project(ambiguous)\n").unwrap();
        fs::write(dir.path().join("meson.build"), "project('ambiguous')\n").unwrap();

        let error = infer_recipe_from_path(
            dir.path(),
            inference_options(dir.path(), "ambiguous", "1.0.0"),
        )
        .unwrap_err();
        let message = error.to_string();

        assert!(
            message.contains("Ambiguous build-system inference"),
            "{message}"
        );
        assert!(message.contains("cmake"), "{message}");
        assert!(message.contains("CMakeLists.txt"), "{message}");
        assert!(message.contains("meson"), "{message}");
        assert!(message.contains("meson.build"), "{message}");
    }

    #[test]
    fn trees_without_supported_markers_fail_with_guidance() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("README.md"), "hello\n").unwrap();

        let error =
            infer_recipe_from_path(dir.path(), InferenceOptions::for_source_root(dir.path()))
                .unwrap_err();
        let message = error.to_string();

        assert!(
            message.contains("No supported build-system markers found"),
            "{message}"
        );
        assert!(message.contains("supported build marker"), "{message}");
        assert!(message.contains("recipe.toml"), "{message}");
        assert!(message.contains("conary new --from ."), "{message}");
    }
}
