// src/commands/ccs/install/component_selection.rs

use anyhow::Result;
use conary_core::ccs::CcsPackage;
use conary_core::components::ComponentType;
use conary_core::packages::traits::PackageFormat;

use crate::commands::install::ComponentSelection;

#[derive(Debug, Clone)]
pub(super) struct SelectedCcsComponents {
    pub(super) names: Vec<String>,
    recognized_types: Vec<ComponentType>,
}

impl SelectedCcsComponents {
    pub(super) fn to_install_component_selection(
        &self,
        available_names: &[String],
    ) -> ComponentSelection {
        if self.names.len() == available_names.len()
            && available_names
                .iter()
                .all(|available| self.names.iter().any(|name| name == available))
        {
            return ComponentSelection::All;
        }

        if self.recognized_types.is_empty() {
            return ComponentSelection::All;
        }

        ComponentSelection::Specific(self.recognized_types.clone())
    }
}

pub(super) fn sorted_available_component_names(ccs_pkg: &CcsPackage) -> Vec<String> {
    let mut names: Vec<String> = ccs_pkg.components().keys().cloned().collect();
    names.sort();
    names
}

pub(super) fn select_ccs_components(
    ccs_pkg: &CcsPackage,
    requested: Option<Vec<String>>,
) -> Result<SelectedCcsComponents> {
    let available = sorted_available_component_names(ccs_pkg);
    if available.is_empty() {
        if ccs_pkg.file_entries().is_empty() {
            return Ok(SelectedCcsComponents {
                names: Vec::new(),
                recognized_types: Vec::new(),
            });
        }
        anyhow::bail!(
            "Package {} does not contain any installable components",
            ccs_pkg.name()
        );
    }

    let names = if let Some(requested_components) = requested {
        let mut selected = Vec::new();
        let mut select_all = false;

        for raw in requested_components {
            let component = raw.trim().to_ascii_lowercase();
            if component.is_empty() {
                continue;
            }

            if component == "all" {
                select_all = true;
                break;
            }

            if !available
                .iter()
                .any(|available_name| available_name == &component)
            {
                anyhow::bail!(
                    "Unknown component '{}'. Available components: {}",
                    raw,
                    available.join(", ")
                );
            }

            if !selected.iter().any(|name| name == &component) {
                selected.push(component);
            }
        }

        if select_all {
            available.clone()
        } else if selected.is_empty() {
            anyhow::bail!(
                "No components selected. Available components: {}",
                available.join(", ")
            );
        } else {
            selected
        }
    } else {
        let mut defaults = Vec::new();
        for component in &ccs_pkg.manifest().components.default {
            let normalized = component.trim().to_ascii_lowercase();
            if available
                .iter()
                .any(|available_name| available_name == &normalized)
                && !defaults.iter().any(|name| name == &normalized)
            {
                defaults.push(normalized);
            }
        }

        if defaults.is_empty() {
            available.clone()
        } else {
            defaults
        }
    };

    let recognized_types = names
        .iter()
        .filter_map(|name| ComponentType::parse(name))
        .collect();

    Ok(SelectedCcsComponents {
        names,
        recognized_types,
    })
}
