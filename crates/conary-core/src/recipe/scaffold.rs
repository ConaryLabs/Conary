// conary-core/src/recipe/scaffold.rs

//! Exact named recipe scaffolding and deterministic recipe materialization.

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

    fs::write(&options.output_path, render_recipe(recipe)?).map_err(|error| {
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
    if matches!(name, "." | "..")
        || Path::new(name).components().any(|component| {
            matches!(
                component,
                Component::CurDir
                    | Component::ParentDir
                    | Component::RootDir
                    | Component::Prefix(_)
            )
        })
    {
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

    use crate::error::Error;
    use crate::recipe::{parse_recipe, validate_recipe};

    use super::{MaterializeOptions, render_recipe_toml, scaffold_named_recipe, write_recipe_toml};

    #[test]
    fn scaffold_named_recipe_is_valid_and_explicit() {
        let recipe = scaffold_named_recipe("hello-scaffold").unwrap();
        let rendered = render_recipe_toml(&recipe).unwrap();
        let parsed = parse_recipe(&rendered).unwrap();

        validate_recipe(&parsed).unwrap();
        assert_eq!(parsed.package.name, "hello-scaffold");
        assert_eq!(parsed.package.version, "0.1.0");
        assert_eq!(parsed.package.summary.as_deref(), Some("hello-scaffold"));
        assert_eq!(parsed.package.license.as_deref(), Some("MIT"));
        assert_eq!(parsed.local_source().unwrap().path.as_os_str(), ".");
    }

    #[test]
    fn scaffold_named_recipe_rejects_non_names() {
        for name in [
            "",
            ".",
            "..",
            "nested/path",
            "nested\\path",
            " hello",
            "hello ",
            "hello\nworld",
            "hello\tworld",
        ] {
            assert!(
                scaffold_named_recipe(name).is_err(),
                "expected {name:?} to be rejected"
            );
        }
    }

    #[test]
    fn materialization_refuses_overwrite_without_force() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("recipe.toml");
        fs::write(&output, "existing recipe\n").unwrap();
        let recipe = scaffold_named_recipe("no-overwrite").unwrap();

        let error = write_recipe_toml(
            &recipe,
            &MaterializeOptions {
                output_path: output.clone(),
                force: false,
            },
        )
        .unwrap_err();

        assert!(matches!(error, Error::AlreadyExists(_)));
        assert_eq!(fs::read_to_string(output).unwrap(), "existing recipe\n");
    }

    #[test]
    fn rendering_is_byte_stable_and_sorts_maps() {
        let mut recipe = scaffold_named_recipe("stable-render").unwrap();
        recipe.build.environment = HashMap::from([
            ("Z_VAR".to_string(), "last".to_string()),
            ("ALPHA".to_string(), "first".to_string()),
        ]);
        recipe.variables = HashMap::from([
            ("zeta".to_string(), "last".to_string()),
            ("alpha".to_string(), "first".to_string()),
        ]);

        let first = render_recipe_toml(&recipe).unwrap();
        let second = render_recipe_toml(&recipe).unwrap();
        assert_eq!(first, second);
        assert!(first.find("ALPHA").unwrap() < first.find("Z_VAR").unwrap());
        assert!(first.find("alpha").unwrap() < first.find("zeta").unwrap());
    }
}
