// conary-core/src/recipe/recording/draft.rs

use anyhow::{Result, bail};

use super::InstalledFileEvidence;

#[derive(Debug, Clone)]
pub struct DraftRecipeInput {
    pub package_name: String,
    pub package_version: String,
    pub command: Vec<String>,
    pub recording_destdir: String,
    pub installed_files: Vec<String>,
    pub network_likely: bool,
}

pub fn render_recorded_command(command: &[String], recording_destdir: &str) -> String {
    command
        .iter()
        .map(|arg| {
            let normalized = arg
                .replace("${CONARY_DESTDIR}", "%(destdir)s")
                .replace("$CONARY_DESTDIR", "%(destdir)s")
                .replace(recording_destdir, "%(destdir)s");
            shell_quote_for_recipe(&normalized)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn derive_draft_recipe(input: DraftRecipeInput) -> Result<String> {
    if input.command.is_empty() {
        bail!("draft recipe requires recorded command");
    }
    let rendered = render_recorded_command(&input.command, &input.recording_destdir);
    let step = if input.installed_files.is_empty() {
        format!("build = \"{rendered}\"")
    } else {
        format!("install = \"{rendered}\"")
    };
    let review_note = if input.network_likely {
        "# Review: network-like behavior was observed or could not be ruled out.\n"
    } else {
        ""
    };
    Ok(format!(
        r#"{review_note}[package]
name = "{name}"
version = "{version}"
release = "1"

[source]
path = "source"

[build]
{step}
"#,
        name = input.package_name,
        version = input.package_version,
    ))
}

pub fn installed_file_paths_from_evidence(files: &[InstalledFileEvidence]) -> Vec<String> {
    let mut paths = files
        .iter()
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    paths
}

/// Quote an argument for generated recipe text.
///
/// `%(destdir)s` syntax remains unquoted so normal Kitchen substitution can
/// replace it. Do not use this helper for live command execution.
fn shell_quote_for_recipe(value: &str) -> String {
    if value.chars().all(|ch| {
        ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | '%' | '(' | ')')
    }) {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_renderer_preserves_arguments_and_redacts_destdir_forms() {
        let rendered = render_recorded_command(
            &[
                "make".to_string(),
                "install".to_string(),
                "DESTDIR=$CONARY_DESTDIR".to_string(),
                "/tmp/conary-record/demo/destdir/usr/bin/app".to_string(),
            ],
            "/tmp/conary-record/demo/destdir",
        );
        assert_eq!(
            rendered,
            "make install 'DESTDIR=%(destdir)s' %(destdir)s/usr/bin/app"
        );
    }

    #[test]
    fn recipe_quote_preserves_destdir_macro_without_shell_expansion() {
        assert_eq!(
            shell_quote_for_recipe("%(destdir)s/usr/bin/app"),
            "%(destdir)s/usr/bin/app"
        );
        assert_eq!(
            shell_quote_for_recipe("$CONARY_DESTDIR/usr/bin/app"),
            "'$CONARY_DESTDIR/usr/bin/app'"
        );
    }

    #[test]
    fn draft_recipe_uses_public_source_snapshot_and_install_step_when_files_exist() {
        let recipe = derive_draft_recipe(DraftRecipeInput {
            package_name: "demo".to_string(),
            package_version: "0.1.0-recorded".to_string(),
            command: vec!["make".to_string(), "install".to_string()],
            recording_destdir: "/tmp/private/destdir".to_string(),
            installed_files: vec!["usr/bin/demo".to_string()],
            network_likely: false,
        })
        .unwrap();

        assert!(recipe.contains("[source]"));
        assert!(recipe.contains("path = \"source\""));
        assert!(recipe.contains("install = \"make install\""));
        assert!(!recipe.contains("/tmp/private"));
    }

    #[test]
    fn installed_file_paths_are_sorted_and_deduplicated() {
        let paths = installed_file_paths_from_evidence(&[
            InstalledFileEvidence {
                path: "usr/bin/z".to_string(),
                file_type: "file".to_string(),
                executable: true,
                size: 1,
                link_target: None,
            },
            InstalledFileEvidence {
                path: "usr/bin/a".to_string(),
                file_type: "file".to_string(),
                executable: true,
                size: 1,
                link_target: None,
            },
            InstalledFileEvidence {
                path: "usr/bin/a".to_string(),
                file_type: "file".to_string(),
                executable: true,
                size: 1,
                link_target: None,
            },
        ]);

        assert_eq!(paths, vec!["usr/bin/a", "usr/bin/z"]);
    }
}
