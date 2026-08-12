// conary-core/src/model/diff/tests.rs
use super::super::state::InstalledPackage;
use super::*;

fn make_state_with_packages(packages: &[(&str, &str, bool)]) -> SystemState {
    let mut state = SystemState::new();
    for (name, version, explicit) in packages {
        state.add_package(
            name.to_string(),
            InstalledPackage {
                name: name.to_string(),
                version: version.to_string(),
                architecture: None,
                explicit: *explicit,
                pinned: false,
                label: None,
            },
        );
    }
    state
}

#[test]
fn test_empty_diff() {
    let model = SystemModel::new();
    let state = SystemState::new();
    let diff = compute_diff(&model, &state);
    assert!(diff.is_empty());
}

#[test]
fn test_install_needed() {
    let mut model = SystemModel::new();
    model.config.install = vec!["nginx".to_string()];

    let state = SystemState::new();
    let diff = compute_diff(&model, &state);

    assert_eq!(diff.packages_to_install(), vec!["nginx"]);
}

#[test]
fn test_already_installed() {
    let mut model = SystemModel::new();
    model.config.install = vec!["nginx".to_string()];

    let state = make_state_with_packages(&[("nginx", "1.24.0", true)]);
    let diff = compute_diff(&model, &state);

    assert!(diff.is_empty());
}

#[test]
fn test_demote_to_dependency() {
    let model = SystemModel::new(); // Empty install list

    let state = make_state_with_packages(&[("nginx", "1.24.0", true)]);
    let diff = compute_diff(&model, &state);

    // Should demote to dependency, not remove
    assert!(diff.actions.iter().any(|a| matches!(
        a,
        DiffAction::MarkDependency { package } if package == "nginx"
    )));
}

#[test]
fn test_excluded_package_removed() {
    let mut model = SystemModel::new();
    model.config.exclude = vec!["sendmail".to_string()];

    let state = make_state_with_packages(&[("sendmail", "1.0.0", true)]);
    let diff = compute_diff(&model, &state);

    assert!(diff.packages_to_remove().contains(&"sendmail"));
}

#[test]
fn test_mark_explicit() {
    let mut model = SystemModel::new();
    model.config.install = vec!["nginx".to_string()];

    // nginx is installed but as a dependency
    let state = make_state_with_packages(&[("nginx", "1.24.0", false)]);
    let diff = compute_diff(&model, &state);

    assert!(diff.actions.iter().any(|a| matches!(
        a,
        DiffAction::MarkExplicit { package } if package == "nginx"
    )));
}

#[test]
fn test_optional_package() {
    let mut model = SystemModel::new();
    model.optional.packages = vec!["nginx-geoip".to_string()];

    let state = SystemState::new();
    let diff = compute_diff(&model, &state);

    assert!(diff.actions.iter().any(|a| matches!(
        a,
        DiffAction::Install { package, optional, .. }
        if package == "nginx-geoip" && *optional
    )));
}

#[test]
fn test_derived_package() {
    use super::super::parser::DerivedPackage;

    let mut model = SystemModel::new();
    model.derive = vec![DerivedPackage {
        name: "nginx-custom".to_string(),
        from: "nginx".to_string(),
        version: "inherit".to_string(),
        patches: vec![],
        override_files: std::collections::HashMap::new(),
    }];

    // Parent not installed
    let state = SystemState::new();
    let diff = compute_diff(&model, &state);

    // Should have BuildDerived action with needs_parent = true
    assert!(diff.actions.iter().any(|a| matches!(
        a,
        DiffAction::BuildDerived { name, parent, needs_parent }
        if name == "nginx-custom" && parent == "nginx" && *needs_parent
    )));

    // Should also install the parent
    assert!(diff.actions.iter().any(|a| matches!(
        a,
        DiffAction::Install { package, .. } if package == "nginx"
    )));
}

#[test]
fn test_derived_package_with_parent_installed() {
    use super::super::parser::DerivedPackage;

    let mut model = SystemModel::new();
    model.derive = vec![DerivedPackage {
        name: "nginx-custom".to_string(),
        from: "nginx".to_string(),
        version: "inherit".to_string(),
        patches: vec![],
        override_files: std::collections::HashMap::new(),
    }];

    // Parent already installed
    let state = make_state_with_packages(&[("nginx", "1.24.0", true)]);
    let diff = compute_diff(&model, &state);

    // Should have BuildDerived action with needs_parent = false
    assert!(diff.actions.iter().any(|a| matches!(
        a,
        DiffAction::BuildDerived { name, parent, needs_parent }
        if name == "nginx-custom" && parent == "nginx" && !*needs_parent
    )));

    // Should NOT install parent again
    assert!(!diff.actions.iter().any(|a| matches!(
        a,
        DiffAction::Install { package, .. } if package == "nginx"
    )));
}

#[test]
fn test_excluded_package_no_duplicate_remove() {
    // Regression test: excluded packages that are explicit should produce
    // exactly one Remove action, not two.
    let mut model = SystemModel::new();
    model.config.exclude = vec!["sendmail".to_string()];

    let state = make_state_with_packages(&[("sendmail", "1.0.0", true)]);
    let diff = compute_diff(&model, &state);

    let remove_count = diff
        .actions
        .iter()
        .filter(|a| {
            matches!(
                a,
                DiffAction::Remove { package, .. } if package == "sendmail"
            )
        })
        .count();
    assert_eq!(
        remove_count, 1,
        "Expected exactly one Remove action for excluded package"
    );
}

#[test]
fn test_excluded_dependency_package_removed() {
    // An excluded package that is installed as a dependency (non-explicit)
    // should still be removed via the excluded-packages loop.
    let mut model = SystemModel::new();
    model.config.exclude = vec!["sendmail".to_string()];

    // sendmail installed as a dependency (explicit=false)
    let state = make_state_with_packages(&[("sendmail", "1.0.0", false)]);
    let diff = compute_diff(&model, &state);

    assert!(diff.packages_to_remove().contains(&"sendmail"));
    let remove_count = diff
        .actions
        .iter()
        .filter(|a| {
            matches!(
                a,
                DiffAction::Remove { package, .. } if package == "sendmail"
            )
        })
        .count();
    assert_eq!(
        remove_count, 1,
        "Expected exactly one Remove action for excluded dependency"
    );
}

#[test]
fn test_summary_reports_visible_realignment_candidates() {
    let mut diff = ModelDiff::new();
    diff.visible_realignment_candidates = Some(crate::model::VisibleRealignmentCandidates {
        target_distro: "arch".to_string(),
        candidate_count: 7,
    });

    let summary = diff.summary();

    assert_eq!(summary.visible_realignment_candidates, Some(7));
}

#[test]
fn test_replatform_replace_is_structural_and_descriptive() {
    let action = DiffAction::ReplatformReplace {
        package: "vim".to_string(),
        current_distro: Some("fedora-44".to_string()),
        target_distro: "arch".to_string(),
        current_version: "9.0.1".to_string(),
        current_architecture: Some("x86_64".to_string()),
        target_version: "9.1.0".to_string(),
        architecture: Some("x86_64".to_string()),
        target_repository: Some("arch-core".to_string()),
        target_repository_package_id: Some(42),
    };

    assert!(action.is_structural());
    assert_eq!(action.package(), "vim");

    let description = action.description();
    assert!(description.contains("Replatform vim"));
    assert!(description.contains("fedora-44"));
    assert!(description.contains("arch"));
    assert!(description.contains("9.0.1 -> 9.1.0"));
    assert!(description.contains("via arch-core"));
}
