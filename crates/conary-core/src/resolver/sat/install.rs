// conary-core/src/resolver/sat/install.rs

use resolvo::{ConditionalRequirement, SolvableId};
use rusqlite::Connection;
use std::collections::HashSet;
use std::time::Instant;

use crate::error::Result;
use crate::version::VersionConstraint;

use super::super::provider::ConaryProvider;
use super::{SatPackage, SatSource, check_transitive_loading_limits};

pub(super) fn build_provider_for_install<'conn>(
    conn: &'conn Connection,
    requests: &[(String, VersionConstraint)],
) -> Result<ConaryProvider<'conn>> {
    let mut provider = ConaryProvider::new(conn);
    provider.load_installed_packages()?;
    provider.build_provides_index()?;
    provider.load_canonical_index()?;
    load_transitive_repo_packages(&mut provider, requests)?;
    provider.intern_all_dependency_version_sets()?;
    Ok(provider)
}

fn load_transitive_repo_packages(
    provider: &mut ConaryProvider<'_>,
    requests: &[(String, VersionConstraint)],
) -> Result<()> {
    let mut loaded_names: HashSet<String> = requests.iter().map(|(name, _)| name.clone()).collect();
    let mut to_load: Vec<String> = loaded_names.iter().cloned().collect();
    let load_start = Instant::now();

    while !to_load.is_empty() {
        check_transitive_loading_limits(load_start.elapsed(), loaded_names.len())?;
        provider.load_repo_packages_for_names(&to_load)?;

        let mut new_names = provider
            .new_dependency_names(&loaded_names)
            .into_iter()
            .filter(|name| loaded_names.insert(name.clone()))
            .collect::<Vec<_>>();

        let canonical_equivalents = new_names
            .iter()
            .flat_map(|name| provider.canonical_equivalents(name).iter().cloned())
            .filter(|name| loaded_names.insert(name.clone()))
            .collect::<Vec<_>>();

        new_names.extend(canonical_equivalents);
        check_transitive_loading_limits(load_start.elapsed(), loaded_names.len())?;
        to_load = new_names;
    }

    Ok(())
}

pub(super) fn build_requirements(
    provider: &mut ConaryProvider<'_>,
    requests: &[(String, VersionConstraint)],
) -> Result<Vec<ConditionalRequirement>> {
    let mut requirements = Vec::with_capacity(requests.len());

    for (name, constraint) in requests {
        let name_id = provider.intern_name(name)?;
        let version_set_id = provider.intern_version_set(name_id, constraint.clone())?;
        requirements.push(ConditionalRequirement::from(version_set_id));
    }

    Ok(requirements)
}

pub(super) fn collect_install_order(
    provider: &ConaryProvider<'_>,
    solvable_ids: &[SolvableId],
) -> Vec<SatPackage> {
    solvable_ids
        .iter()
        .map(|sid| {
            let pkg = provider.get_solvable(*sid);
            SatPackage {
                name: pkg.name.clone(),
                version: pkg.version.clone(),
                source: if pkg.installed_trove_id.is_some() {
                    SatSource::Installed
                } else {
                    SatSource::Repository
                },
            }
        })
        .collect()
}
