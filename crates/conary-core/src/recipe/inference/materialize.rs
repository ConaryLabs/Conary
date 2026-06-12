// conary-core/src/recipe/inference/materialize.rs

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;

use crate::error::{Error, Result};
use crate::recipe::format::{
    BuildSection, ComponentSection, CrossSection, LocalSourceSection, PackageSection, PatchSection,
    Recipe, SourceSection,
};

#[derive(Debug, Clone)]
pub struct MaterializeOptions {
    pub output_path: PathBuf,
    pub force: bool,
    pub source_override: Option<SourceSection>,
}

pub fn render_recipe_toml(recipe: &Recipe) -> Result<String> {
    render_recipe(recipe)
}

pub fn write_recipe_toml(recipe: &Recipe, options: &MaterializeOptions) -> Result<()> {
    if options.output_path.exists() && !options.force {
        return Err(Error::AlreadyExists(format!(
            "{} already exists; pass force to overwrite it",
            options.output_path.display()
        )));
    }

    if let Some(parent) = options.output_path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }

    let mut materialized = recipe.clone();
    if let Some(source) = &options.source_override {
        materialized.source = source.clone();
    }

    fs::write(&options.output_path, render_recipe(&materialized)?).map_err(|error| {
        Error::IoError(format!(
            "writing {}: {error}",
            options.output_path.display()
        ))
    })
}

pub fn scaffold_named_recipe(name: &str) -> Result<Recipe> {
    validate_scaffold_name(name)?;

    Ok(Recipe {
        package: PackageSection {
            name: name.to_string(),
            version: "0.1.0".to_string(),
            release: "1".to_string(),
            summary: Some(name.to_string()),
            description: None,
            license: Some("MIT".to_string()),
            homepage: None,
        },
        source: SourceSection::Local(LocalSourceSection {
            path: PathBuf::from("."),
        }),
        build: BuildSection {
            requires: Vec::new(),
            makedepends: Vec::new(),
            configure: None,
            make: None,
            install: Some(
                "mkdir -p %(destdir)s/usr/share/%(name)s && cp -a . %(destdir)s/usr/share/%(name)s"
                    .to_string(),
            ),
            check: None,
            setup: None,
            post_install: None,
            environment: HashMap::new(),
            workdir: None,
            script_file: None,
            jobs: None,
            stage: None,
        },
        cross: None,
        patches: None,
        components: None,
        variables: HashMap::new(),
    })
}

fn render_recipe(recipe: &Recipe) -> Result<String> {
    let mut rendered = toml::to_string_pretty(&RecipeToml::from(recipe))
        .map_err(|error| Error::ParseError(format!("Failed to serialize recipe: {error}")))?;
    if !rendered.ends_with('\n') {
        rendered.push('\n');
    }
    Ok(rendered)
}

#[derive(Serialize)]
struct RecipeToml<'a> {
    package: &'a PackageSection,
    source: &'a SourceSection,
    build: BuildToml<'a>,
    cross: &'a Option<CrossSection>,
    patches: &'a Option<PatchSection>,
    components: &'a Option<ComponentSection>,
    variables: BTreeMap<&'a str, &'a str>,
}

impl<'a> From<&'a Recipe> for RecipeToml<'a> {
    fn from(recipe: &'a Recipe) -> Self {
        Self {
            package: &recipe.package,
            source: &recipe.source,
            build: BuildToml::from(&recipe.build),
            cross: &recipe.cross,
            patches: &recipe.patches,
            components: &recipe.components,
            variables: sorted_string_map(&recipe.variables),
        }
    }
}

#[derive(Serialize)]
struct BuildToml<'a> {
    requires: &'a Vec<String>,
    makedepends: &'a Vec<String>,
    configure: &'a Option<String>,
    make: &'a Option<String>,
    install: &'a Option<String>,
    check: &'a Option<String>,
    setup: &'a Option<String>,
    post_install: &'a Option<String>,
    environment: BTreeMap<&'a str, &'a str>,
    workdir: &'a Option<String>,
    script_file: &'a Option<String>,
    jobs: &'a Option<u32>,
    stage: &'a Option<String>,
}

impl<'a> From<&'a BuildSection> for BuildToml<'a> {
    fn from(build: &'a BuildSection) -> Self {
        Self {
            requires: &build.requires,
            makedepends: &build.makedepends,
            configure: &build.configure,
            make: &build.make,
            install: &build.install,
            check: &build.check,
            setup: &build.setup,
            post_install: &build.post_install,
            environment: sorted_string_map(&build.environment),
            workdir: &build.workdir,
            script_file: &build.script_file,
            jobs: &build.jobs,
            stage: &build.stage,
        }
    }
}

fn sorted_string_map(map: &HashMap<String, String>) -> BTreeMap<&str, &str> {
    map.iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect()
}

fn validate_scaffold_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(Error::ConfigError(
            "Scaffold recipe name cannot be empty".to_string(),
        ));
    }
    if name.trim() != name {
        return Err(Error::InvalidPath(format!(
            "Scaffold recipe name {name:?} cannot contain leading or trailing whitespace"
        )));
    }
    if name.chars().any(char::is_control) {
        return Err(Error::InvalidPath(format!(
            "Scaffold recipe name {name:?} cannot contain control characters"
        )));
    }
    if name.contains('/') || name.contains('\\') {
        return Err(Error::InvalidPath(format!(
            "Scaffold recipe name {name:?} cannot contain path separators"
        )));
    }
    if matches!(name, "." | "..") {
        return Err(Error::InvalidPath(format!(
            "Scaffold recipe name {name:?} cannot be a path component"
        )));
    }
    if Path::new(name).components().any(|component| {
        matches!(
            component,
            Component::CurDir | Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(Error::InvalidPath(format!(
            "Scaffold recipe name {name:?} cannot be a path component"
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;
    use std::path::Path;

    use crate::error::Error;
    use crate::recipe::{
        InferenceOptions, LocalSourceSection, Recipe, SourceSection, parse_recipe, validate_recipe,
    };

    use super::super::infer_recipe_from_path;
    use super::{MaterializeOptions, render_recipe_toml, scaffold_named_recipe, write_recipe_toml};

    fn cargo_source_tree(root: &Path, name: &str, version: &str) {
        fs::write(
            root.join("Cargo.toml"),
            format!(
                r#"[package]
name = "{name}"
version = "{version}"
description = "test package"
license = "MIT"
"#
            ),
        )
        .unwrap();
    }

    fn infer_cargo_recipe(root: &Path) -> Recipe {
        infer_recipe_from_path(root, InferenceOptions::for_source_root(root))
            .unwrap()
            .recipe
    }

    fn parse_and_validate(rendered: &str) -> Recipe {
        let from_toml: Recipe = toml::from_str(rendered).expect("rendered recipe must be TOML");
        validate_recipe(&from_toml).expect("rendered recipe must validate");

        let from_parser = parse_recipe(rendered).expect("rendered recipe must parse");
        validate_recipe(&from_parser).expect("parsed recipe must validate");

        from_toml
    }

    fn assert_text_order(rendered: &str, expected_order: &[&str]) {
        let mut previous_index = 0;
        for expected in expected_order {
            let offset = rendered[previous_index..]
                .find(expected)
                .unwrap_or_else(|| panic!("expected rendered TOML to contain {expected:?}"));
            previous_index += offset + expected.len();
        }
    }

    #[test]
    fn writes_inferred_recipe_to_recipe_toml() {
        let source = tempfile::tempdir().unwrap();
        cargo_source_tree(source.path(), "hello-materialize", "1.2.3");
        let recipe = infer_cargo_recipe(source.path());
        let output_path = source.path().join("out").join("recipe.toml");

        write_recipe_toml(
            &recipe,
            &MaterializeOptions {
                output_path: output_path.clone(),
                force: false,
                source_override: None,
            },
        )
        .unwrap();

        let rendered = fs::read_to_string(output_path).unwrap();
        let parsed = parse_and_validate(&rendered);

        assert_eq!(parsed.package.name, "hello-materialize");
        assert_eq!(parsed.package.version, "1.2.3");
        assert_eq!(
            parsed.local_source().unwrap().path,
            std::path::PathBuf::from(".")
        );
        assert!(rendered.ends_with('\n'));
    }

    #[test]
    fn refuses_to_overwrite_recipe_toml_without_force() {
        let source = tempfile::tempdir().unwrap();
        cargo_source_tree(source.path(), "no-overwrite", "1.2.3");
        let recipe = infer_cargo_recipe(source.path());
        let output_path = source.path().join("recipe.toml");
        fs::write(&output_path, "existing recipe\n").unwrap();

        let error = write_recipe_toml(
            &recipe,
            &MaterializeOptions {
                output_path: output_path.clone(),
                force: false,
                source_override: None,
            },
        )
        .unwrap_err();

        assert!(
            matches!(error, Error::AlreadyExists(ref message) if message.contains("recipe.toml")),
            "{error:?}"
        );
        assert_eq!(
            fs::read_to_string(output_path).unwrap(),
            "existing recipe\n"
        );
    }

    #[test]
    fn force_overwrites_recipe_toml_deterministically() {
        let source = tempfile::tempdir().unwrap();
        cargo_source_tree(source.path(), "force-overwrite", "1.2.3");
        let recipe = infer_cargo_recipe(source.path());
        let output_path = source.path().join("recipe.toml");
        fs::write(&output_path, "existing recipe\n").unwrap();

        let options = MaterializeOptions {
            output_path: output_path.clone(),
            force: true,
            source_override: None,
        };

        write_recipe_toml(&recipe, &options).unwrap();
        let first = fs::read_to_string(&output_path).unwrap();
        write_recipe_toml(&recipe, &options).unwrap();
        let second = fs::read_to_string(&output_path).unwrap();

        assert_eq!(first, second);
        assert_eq!(first, render_recipe_toml(&recipe).unwrap());
        parse_and_validate(&first);
    }

    #[test]
    fn source_override_replaces_inferred_local_source_when_writing() {
        let source = tempfile::tempdir().unwrap();
        cargo_source_tree(source.path(), "source-override", "1.2.3");
        let recipe = infer_cargo_recipe(source.path());
        let output_path = source.path().join("recipe.toml");

        write_recipe_toml(
            &recipe,
            &MaterializeOptions {
                output_path: output_path.clone(),
                force: false,
                source_override: Some(SourceSection::Local(LocalSourceSection {
                    path: "unpacked/source".into(),
                })),
            },
        )
        .unwrap();

        let rendered = fs::read_to_string(output_path).unwrap();
        let parsed = parse_and_validate(&rendered);

        assert_eq!(
            parsed.local_source().unwrap().path,
            std::path::PathBuf::from("unpacked/source")
        );
        assert_eq!(
            recipe.local_source().unwrap().path,
            std::path::PathBuf::from(".")
        );
    }

    #[test]
    fn scaffold_named_recipe_parses_with_parse_recipe() {
        let recipe = scaffold_named_recipe("hello-scaffold").unwrap();
        let rendered = render_recipe_toml(&recipe).unwrap();
        let parsed = parse_recipe(&rendered).unwrap();

        validate_recipe(&parsed).unwrap();
        assert_eq!(parsed.package.name, "hello-scaffold");
        assert_eq!(parsed.package.version, "0.1.0");
        assert_eq!(parsed.package.summary.as_deref(), Some("hello-scaffold"));
        assert_eq!(parsed.package.license.as_deref(), Some("MIT"));
        assert_eq!(
            parsed.local_source().unwrap().path,
            std::path::PathBuf::from(".")
        );
        assert_eq!(
            parsed.build.install.as_deref(),
            Some(
                "mkdir -p %(destdir)s/usr/share/%(name)s && cp -a . %(destdir)s/usr/share/%(name)s"
            )
        );
    }

    #[test]
    fn scaffold_named_recipe_rejects_empty_dots_and_path_separators() {
        for name in ["", ".", "..", "nested/path", "nested\\path"] {
            assert!(
                scaffold_named_recipe(name).is_err(),
                "expected {name:?} to be rejected"
            );
        }
    }

    #[test]
    fn scaffold_named_recipe_rejects_whitespace_and_control_characters() {
        for name in [" hello", "hello ", "hello\nworld", "hello\tworld"] {
            assert!(
                scaffold_named_recipe(name).is_err(),
                "expected {name:?} to be rejected"
            );
        }
    }

    #[test]
    fn rendered_toml_is_byte_stable_for_same_inference_result() {
        let source = tempfile::tempdir().unwrap();
        cargo_source_tree(source.path(), "stable-render", "1.2.3");
        let recipe = infer_cargo_recipe(source.path());

        let first = render_recipe_toml(&recipe).unwrap();
        let second = render_recipe_toml(&recipe).unwrap();

        assert_eq!(first, second);
        parse_and_validate(&first);
    }

    #[test]
    fn rendered_toml_sorts_maps_canonically() {
        let mut recipe = scaffold_named_recipe("sorted-render").unwrap();
        recipe.build.environment = HashMap::from([
            ("Z_VAR".to_string(), "last".to_string()),
            ("ALPHA".to_string(), "first".to_string()),
            ("MIDDLE".to_string(), "middle".to_string()),
        ]);
        recipe.variables = HashMap::from([
            ("zeta".to_string(), "last".to_string()),
            ("alpha".to_string(), "first".to_string()),
            ("middle".to_string(), "middle".to_string()),
        ]);

        let rendered = render_recipe_toml(&recipe).unwrap();

        assert_text_order(
            &rendered,
            &[
                "[build.environment]",
                "ALPHA = \"first\"",
                "MIDDLE = \"middle\"",
                "Z_VAR = \"last\"",
            ],
        );
        assert_text_order(
            &rendered,
            &[
                "[variables]",
                "alpha = \"first\"",
                "middle = \"middle\"",
                "zeta = \"last\"",
            ],
        );
        parse_and_validate(&rendered);
    }
}
