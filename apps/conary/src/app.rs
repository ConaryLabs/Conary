// apps/conary/src/app.rs
//! Conary application bootstrap and top-level error presentation.

use anyhow::Result;
use clap::Parser;

use crate::cli::Cli;
use crate::dispatch;

pub async fn run() -> Result<()> {
    conary_bootstrap::init_tracing();

    let cli = Cli::parse();
    conary_core::scriptlet::set_seccomp_warn_override(cli.seccomp_warn);

    dispatch::dispatch(cli).await
}

pub(crate) fn report_error(err: &anyhow::Error) {
    for line in render_error_lines(err) {
        eprintln!("{line}");
    }
}

fn render_error_lines(err: &anyhow::Error) -> Vec<String> {
    if let Some(core_err) = err.downcast_ref::<conary_core::Error>() {
        match core_err {
            conary_core::Error::DatabaseNotFound(_) => vec![
                "Error: Database not initialized.".to_string(),
                "Run 'conary system init' to set up the package database.".to_string(),
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
    fn test_render_error_lines_for_database_not_found() {
        let err = anyhow::Error::new(conary_core::Error::DatabaseNotFound(
            "/tmp/conary.db".to_string(),
        ));

        assert_eq!(
            render_error_lines(&err),
            vec![
                "Error: Database not initialized.".to_string(),
                "Run 'conary system init' to set up the package database.".to_string(),
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
        assert_eq!(render_error_lines(&err), vec!["Error: plain failure".to_string()]);
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
