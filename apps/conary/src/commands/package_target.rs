// apps/conary/src/commands/package_target.rs

//! Shared installed-package selector and rendering helpers.

use anyhow::Result;
use conary_core::db::models::{InstallSource, Trove, TroveType};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InstalledPackageSelector {
    pub(crate) name: String,
    pub(crate) version: Option<String>,
    pub(crate) architecture: Option<String>,
}

impl InstalledPackageSelector {
    pub(crate) fn new(name: String, version: Option<String>, architecture: Option<String>) -> Self {
        Self {
            name,
            version,
            architecture,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedInstalledPackage {
    pub(crate) trove: Trove,
    pub(crate) trove_id: i64,
}

pub(crate) fn resolve_installed_package(
    conn: &rusqlite::Connection,
    selector: &InstalledPackageSelector,
) -> Result<ResolvedInstalledPackage> {
    let troves = Trove::find_by_name(conn, &selector.name)?
        .into_iter()
        .filter(|trove| trove.trove_type == TroveType::Package)
        .collect::<Vec<_>>();

    if troves.is_empty() {
        anyhow::bail!("Package '{}' is not installed", selector.name);
    }

    let matches = matching_installed_packages(&troves, selector);
    match matches.as_slice() {
        [] => anyhow::bail!(
            "Package '{}' with selector version={:?} architecture={:?} is not installed. Installed variants: {}",
            selector.name,
            selector.version,
            selector.architecture,
            format_installed_variants(&troves)
        ),
        [trove] => {
            let trove_id = trove
                .id
                .ok_or_else(|| anyhow::anyhow!("Package '{}' has no database ID", selector.name))?;
            Ok(ResolvedInstalledPackage {
                trove: (*trove).clone(),
                trove_id,
            })
        }
        _ => {
            let variants = matches
                .iter()
                .map(|trove| format!("  - {}", format_installed_variant(trove)))
                .collect::<Vec<_>>()
                .join("\n");
            anyhow::bail!(
                "Multiple installed variants of '{}' match the selector:\n{}\nUse --version and/or --arch to choose one.",
                selector.name,
                variants
            )
        }
    }
}

pub(crate) fn matching_installed_packages<'a>(
    troves: &'a [Trove],
    selector: &InstalledPackageSelector,
) -> Vec<&'a Trove> {
    troves
        .iter()
        .filter(|trove| {
            selector
                .version
                .as_deref()
                .is_none_or(|version| trove.version == version)
                && architecture_matches(
                    selector.architecture.as_deref(),
                    trove.architecture.as_deref(),
                )
        })
        .collect()
}

fn architecture_matches(selector: Option<&str>, actual: Option<&str>) -> bool {
    match selector {
        None => true,
        Some("none" | "unspecified") => actual.is_none(),
        Some(arch) => actual == Some(arch),
    }
}

pub(crate) fn format_installed_variant(trove: &Trove) -> String {
    format!(
        "version {} [{}] ({}, {})",
        trove.version,
        trove.architecture.as_deref().unwrap_or("none"),
        package_authority_label(trove.install_source.clone()),
        trove.version_scheme.as_deref().unwrap_or("unknown-scheme")
    )
}

pub(crate) fn format_installed_variants(troves: &[Trove]) -> String {
    troves
        .iter()
        .map(format_installed_variant)
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn package_authority_label(source: InstallSource) -> &'static str {
    if source.is_adopted() {
        "native-authority"
    } else {
        "conary-owned"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::models::{InstallSource, Trove, TroveType};

    fn db_with_variants() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conary_core::db::schema::migrate(&conn).unwrap();

        let mut x86 = Trove::new_with_source(
            "demo".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            InstallSource::Repository,
        );
        x86.architecture = Some("x86_64".to_string());
        x86.insert(&conn).unwrap();

        let mut arm = Trove::new_with_source(
            "demo".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            InstallSource::Repository,
        );
        arm.architecture = Some("aarch64".to_string());
        arm.insert(&conn).unwrap();

        conn
    }

    #[test]
    fn selector_refuses_ambiguous_package_without_variant_fields() {
        let conn = db_with_variants();
        let selector = InstalledPackageSelector::new("demo".to_string(), None, None);

        let err = resolve_installed_package(&conn, &selector)
            .unwrap_err()
            .to_string();

        assert!(err.contains("Multiple installed variants of 'demo' match"));
        assert!(err.contains("version 1.0.0 [x86_64]"));
        assert!(err.contains("version 1.0.0 [aarch64]"));
        assert!(err.contains("--version"));
        assert!(err.contains("--arch"));
    }

    #[test]
    fn selector_resolves_version_and_architecture() {
        let conn = db_with_variants();
        let selector = InstalledPackageSelector::new(
            "demo".to_string(),
            Some("1.0.0".to_string()),
            Some("aarch64".to_string()),
        );

        let resolved = resolve_installed_package(&conn, &selector).unwrap();

        assert_eq!(resolved.trove.name, "demo");
        assert_eq!(resolved.trove.version, "1.0.0");
        assert_eq!(resolved.trove.architecture.as_deref(), Some("aarch64"));
    }

    #[test]
    fn selector_reports_available_variants_when_filter_matches_none() {
        let conn = db_with_variants();
        let selector = InstalledPackageSelector::new(
            "demo".to_string(),
            Some("2.0.0".to_string()),
            Some("x86_64".to_string()),
        );

        let err = resolve_installed_package(&conn, &selector)
            .unwrap_err()
            .to_string();

        assert!(err.contains("Package 'demo' with selector"));
        assert!(err.contains("Installed variants:"));
        assert!(err.contains("1.0.0 [x86_64]"));
    }

    #[test]
    fn selector_ignores_non_package_troves() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conary_core::db::schema::migrate(&conn).unwrap();

        let mut component = Trove::new(
            "demo".to_string(),
            "1.0.0".to_string(),
            TroveType::Component,
        );
        component.architecture = Some("aarch64".to_string());
        component.insert(&conn).unwrap();

        let mut package = Trove::new("demo".to_string(), "1.0.0".to_string(), TroveType::Package);
        package.architecture = Some("x86_64".to_string());
        package.insert(&conn).unwrap();

        let selector = InstalledPackageSelector::new("demo".to_string(), None, None);
        let resolved = resolve_installed_package(&conn, &selector).unwrap();

        assert_eq!(resolved.trove.trove_type, TroveType::Package);
    }

    #[test]
    fn selector_can_target_unspecified_architecture() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conary_core::db::schema::migrate(&conn).unwrap();

        let mut unspecified =
            Trove::new("demo".to_string(), "1.0.0".to_string(), TroveType::Package);
        unspecified.insert(&conn).unwrap();

        let mut x86 = Trove::new("demo".to_string(), "1.0.0".to_string(), TroveType::Package);
        x86.architecture = Some("x86_64".to_string());
        x86.insert(&conn).unwrap();

        let selector = InstalledPackageSelector::new(
            "demo".to_string(),
            Some("1.0.0".to_string()),
            Some("none".to_string()),
        );
        let resolved = resolve_installed_package(&conn, &selector).unwrap();

        assert_eq!(resolved.trove.architecture, None);
    }
}
