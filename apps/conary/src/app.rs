// apps/conary/src/app.rs
//! Conary application bootstrap and top-level error presentation.

use anyhow::Result;
use clap::Parser;

use crate::cli::Cli;
use crate::dispatch;

pub async fn run() -> Result<()> {
    let cli = Cli::parse();
    conary_bootstrap::init_cli_tracing(crate::logging::verbosity_directive(
        cli.quiet,
        cli.log_verbose,
    ));

    if cli.help_advanced {
        print!("{}", crate::cli::render_advanced_help());
        return Ok(());
    }
    dispatch::dispatch(cli).await
}

pub fn report_error(err: &anyhow::Error) {
    for line in render_error_lines(err) {
        eprintln!("{line}");
    }
}

fn render_error_lines(err: &anyhow::Error) -> Vec<String> {
    if let Some(core_err) = err.downcast_ref::<conary_core::Error>() {
        match core_err {
            conary_core::Error::DatabaseNotFound(path)
                if std::path::Path::new(path)
                    == conary_core::runtime_root::ConaryRuntimeRoot::default().db_path() =>
            {
                vec![
                    "Error: Database not initialized.".to_string(),
                    "Run 'sudo conary system init' to set up the system database and all built-in source feeds."
                        .to_string(),
                ]
            }
            conary_core::Error::DatabaseNotFound(path) => vec![
                format!("Error: Custom database not initialized at {path:?}."),
                "Run 'conary system init --db-path <PATH>' with the same custom path.".to_string(),
            ],
            conary_core::Error::NotFound(detail) => vec![format!("Error: {detail}")],
            conary_core::Error::ConflictError(detail) => vec![
                format!("Error: Conflict -- {detail}"),
                "Try 'conary remove' first or use '--force' if available.".to_string(),
            ],
            conary_core::Error::PathTraversal(detail) => vec![
                format!("Error: Path safety violation -- {detail}"),
                "This may indicate a malicious or corrupt package.".to_string(),
            ],
            other => vec![format!("Error: {other}")],
        }
    } else {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| format!("{err:#}"))) {
            Ok(msg) => vec![format!("Error: {msg}")],
            Err(_) => vec![format!("Error: {err}")],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_error_lines_for_custom_database_not_found() {
        let err = anyhow::Error::new(conary_core::Error::DatabaseNotFound(
            "/tmp/conary.db".to_string(),
        ));

        assert_eq!(
            render_error_lines(&err),
            vec![
                "Error: Custom database not initialized at \"/tmp/conary.db\".".to_string(),
                "Run 'conary system init --db-path <PATH>' with the same custom path.".to_string(),
            ]
        );
    }

    #[test]
    fn test_render_error_lines_for_system_database_not_found() {
        let err = anyhow::Error::new(conary_core::Error::DatabaseNotFound(
            "/var/lib/conary/conary.db".to_string(),
        ));

        assert_eq!(
            render_error_lines(&err),
            vec![
                "Error: Database not initialized.".to_string(),
                "Run 'sudo conary system init' to set up the system database and all built-in source feeds."
                    .to_string(),
            ]
        );
    }

    #[test]
    fn test_render_error_lines_for_conflict_error() {
        let err = anyhow::Error::new(conary_core::Error::ConflictError(
            "package already installed".to_string(),
        ));

        assert_eq!(
            render_error_lines(&err),
            vec![
                "Error: Conflict -- package already installed".to_string(),
                "Try 'conary remove' first or use '--force' if available.".to_string(),
            ]
        );
    }

    #[test]
    fn test_render_error_lines_for_generic_anyhow_error() {
        let err = anyhow::anyhow!("plain failure");
        assert_eq!(
            render_error_lines(&err),
            vec!["Error: plain failure".to_string()]
        );
    }

    #[test]
    fn test_render_error_lines_for_path_traversal() {
        let err = anyhow::Error::new(conary_core::Error::PathTraversal(
            "../etc/passwd".to_string(),
        ));

        assert_eq!(
            render_error_lines(&err),
            vec![
                "Error: Path safety violation -- ../etc/passwd".to_string(),
                "This may indicate a malicious or corrupt package.".to_string(),
            ]
        );
    }
}
