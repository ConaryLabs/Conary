// apps/conary/src/commands/install/conversion/tests/dependencies.rs

use super::*;

#[test]
fn detects_conditional_rpm_dependencies() {
    assert!(is_conditional_rpm_dependency(
        "((kernel-modules-extra-uname-r = 6.19.6-200.fc44.x86_64) if kernel-modules-extra-matched)"
    ));
    assert!(!is_conditional_rpm_dependency("kernel-core-uname-r"));
}

#[test]
fn ignores_rpm_internal_config_dependencies() {
    assert!(is_ignored_rpm_dependency("config(dracut) = 107-8.fc44"));
    assert!(is_ignored_rpm_dependency("rpmlib(CompressedFileNames)"));
    assert!(is_ignored_rpm_dependency("/usr/bin/bash"));
    assert!(!is_ignored_rpm_dependency("kernel-core-uname-r"));
}

#[test]
fn promote_only_repo_resolvable_deps() {
    let conn = test_db();

    let mut repo = Repository::new("fedora-remi".to_string(), "https://example.invalid".into());
    repo.insert(&conn).unwrap();
    let repo_id = repo.id.unwrap();

    // coreutils-common exists in the repo (direct name match)
    let mut pkg = RepositoryPackage::new(
        repo_id,
        "coreutils-common".to_string(),
        "9.7-8.fc44".to_string(),
        "sha256:cc".to_string(),
        100,
        "https://example.invalid/coreutils-common.rpm".to_string(),
    );
    pkg.insert(&conn).unwrap();

    // kernel-core exists and provides kernel-core-uname-r via normalized table
    let mut kpkg = RepositoryPackage::new(
        repo_id,
        "kernel-core".to_string(),
        "6.19.6-200.fc44".to_string(),
        "sha256:kc".to_string(),
        200,
        "https://example.invalid/kernel-core.rpm".to_string(),
    );
    kpkg.insert(&conn).unwrap();
    let kpkg_id = kpkg.id.unwrap();

    let mut provide = RepositoryProvide::new(
        kpkg_id,
        "kernel-core-uname-r".to_string(),
        Some("6.19.6-200.fc44.x86_64".to_string()),
        "package".to_string(),
        Some("kernel-core-uname-r = 6.19.6-200.fc44.x86_64".to_string()),
    );
    provide.insert(&conn).unwrap();

    let mut dep_plan = dep_resolution::DepResolutionPlan {
        unresolvable: vec![
            conary_core::resolver::MissingDependency {
                name: "coreutils-common".to_string(),
                constraint: VersionConstraint::Any,
                required_by: vec!["kernel".to_string()],
            },
            conary_core::resolver::MissingDependency {
                name: "kernel-core-uname-r".to_string(),
                constraint: VersionConstraint::parse("= 6.19.6-200.fc44.x86_64").unwrap(),
                required_by: vec!["kernel".to_string()],
            },
            conary_core::resolver::MissingDependency {
                name: "nonexistent-fantasy-pkg".to_string(),
                constraint: VersionConstraint::Any,
                required_by: vec!["kernel".to_string()],
            },
        ],
        ..Default::default()
    };

    promote_repo_resolvable_satisfy_deps(&conn, &mut dep_plan);

    // Two deps should have been promoted
    assert_eq!(dep_plan.to_install.len(), 2, "expected 2 promoted deps");
    let promoted_names: Vec<&str> = dep_plan
        .to_install
        .iter()
        .map(|d| d.name.as_str())
        .collect();
    assert!(promoted_names.contains(&"coreutils-common"));
    assert!(promoted_names.contains(&"kernel-core-uname-r"));

    // The nonexistent dep should remain unresolvable
    assert_eq!(
        dep_plan.unresolvable.len(),
        1,
        "expected 1 still-unresolvable"
    );
    assert_eq!(dep_plan.unresolvable[0].name, "nonexistent-fantasy-pkg");
}

#[test]
fn promote_skips_when_all_unresolvable() {
    let conn = test_db();

    let mut dep_plan = dep_resolution::DepResolutionPlan {
        unresolvable: vec![conary_core::resolver::MissingDependency {
            name: "nonexistent-pkg".to_string(),
            constraint: VersionConstraint::Any,
            required_by: vec!["test".to_string()],
        }],
        ..Default::default()
    };

    promote_repo_resolvable_satisfy_deps(&conn, &mut dep_plan);

    assert!(dep_plan.to_install.is_empty());
    assert_eq!(dep_plan.unresolvable.len(), 1);
    assert_eq!(dep_plan.unresolvable[0].name, "nonexistent-pkg");
}

#[test]
fn pending_providers_satisfy_versioned_kernel_family_capability() {
    let conn = test_db();

    let mut repo = Repository::new("fedora-remi".to_string(), "https://example.invalid".into());
    repo.insert(&conn).unwrap();
    let repo_id = repo.id.unwrap();

    let mut pkg = RepositoryPackage::new(
        repo_id,
        "kernel".to_string(),
        "6.17.1-300.fc44".to_string(),
        "sha256:kernel".to_string(),
        123,
        "https://example.invalid/kernel.rpm".to_string(),
    );
    pkg.insert(&conn).unwrap();
    let pkg_id = pkg.id.unwrap();

    let mut provide = RepositoryProvide::new(
        pkg_id,
        "kernel-uname-r".to_string(),
        Some("6.17.1-300.fc44.x86_64".to_string()),
        "package".to_string(),
        Some("kernel-uname-r = 6.17.1-300.fc44.x86_64".to_string()),
    );
    provide.insert(&conn).unwrap();

    let pending = vec![PendingCcsProvider {
        name: "kernel".to_string(),
        version: "6.17.1-300.fc44".to_string(),
        provides: vec!["kernel-uname-r".to_string()],
    }];
    let dep = conary_core::resolver::MissingDependency {
        name: "kernel-uname-r".to_string(),
        constraint: VersionConstraint::parse("= 6.17.1-300.fc44.x86_64").unwrap(),
        required_by: vec!["kernel-modules-core".to_string()],
    };

    assert!(
        pending_provider_satisfies_dependency(&conn, &pending, &dep),
        "recursive CCS dependency installs must honor providers already pending in the transaction"
    );
}

#[test]
fn promote_does_not_convert_blocked_runtime_capability_into_repo_install() {
    let conn = test_db();

    let mut repo = Repository::new("fedora-remi".to_string(), "https://example.invalid".into());
    repo.insert(&conn).unwrap();
    let repo_id = repo.id.unwrap();

    let mut pkg = RepositoryPackage::new(
        repo_id,
        "glibc".to_string(),
        "2.42-4.fc44".to_string(),
        "sha256:glibc".to_string(),
        123,
        "https://example.invalid/glibc.rpm".to_string(),
    );
    pkg.insert(&conn).unwrap();
    let pkg_id = pkg.id.unwrap();

    let mut provide = RepositoryProvide::new(
        pkg_id,
        "libc.so.6(GLIBC_2.34)(64bit)".to_string(),
        Some("2.42-4.fc44".to_string()),
        "package".to_string(),
        Some("libc.so.6(GLIBC_2.34)(64bit)".to_string()),
    );
    provide.insert(&conn).unwrap();

    let dep_name = "libc.so.6(GLIBC_2.34)(64bit)".to_string();
    let mut dep_plan = dep_resolution::DepResolutionPlan {
        unresolvable: vec![conary_core::resolver::MissingDependency {
            name: dep_name.clone(),
            constraint: VersionConstraint::Any,
            required_by: vec!["tree".to_string()],
        }],
        ..Default::default()
    };

    promote_repo_resolvable_satisfy_deps(&conn, &mut dep_plan);

    assert!(
        dep_plan.to_install.is_empty(),
        "blocked runtime capabilities must never be promoted into repo installs"
    );
    assert!(dep_plan.unresolvable.is_empty());
    assert_eq!(dep_plan.blocked, vec![dep_name]);
}

#[test]
fn promote_does_not_convert_pam_soname_capabilities_into_repo_installs() {
    let conn = test_db();

    let mut repo = Repository::new("fedora-remi".to_string(), "https://example.invalid".into());
    repo.insert(&conn).unwrap();
    let repo_id = repo.id.unwrap();

    let mut pkg = RepositoryPackage::new(
        repo_id,
        "pam".to_string(),
        "1.7.1-3.fc44".to_string(),
        "sha256:pam".to_string(),
        123,
        "https://example.invalid/pam.rpm".to_string(),
    );
    pkg.insert(&conn).unwrap();
    let pkg_id = pkg.id.unwrap();

    let mut provide = RepositoryProvide::new(
        pkg_id,
        "libpam.so.0(LIBPAM_1.0)(64bit)".to_string(),
        Some("1.7.1-3.fc44".to_string()),
        "soname".to_string(),
        Some("libpam.so.0(LIBPAM_1.0)(64bit)".to_string()),
    );
    provide.insert(&conn).unwrap();

    let dep_name = "libpam.so.0(LIBPAM_1.0)(64bit)".to_string();
    let mut dep_plan = dep_resolution::DepResolutionPlan {
        unresolvable: vec![conary_core::resolver::MissingDependency {
            name: dep_name.clone(),
            constraint: VersionConstraint::Any,
            required_by: vec!["qemu-img".to_string()],
        }],
        ..Default::default()
    };

    promote_repo_resolvable_satisfy_deps(&conn, &mut dep_plan);

    assert!(
        dep_plan.to_install.is_empty(),
        "libpam runtime capabilities must not ask Remi to convert pam"
    );
    assert!(dep_plan.unresolvable.is_empty());
    assert_eq!(dep_plan.blocked, vec![dep_name]);
}

#[test]
fn promote_does_not_convert_pcre2_soname_capabilities_into_repo_installs() {
    let conn = test_db();

    let mut repo = Repository::new("fedora-remi".to_string(), "https://example.invalid".into());
    repo.insert(&conn).unwrap();
    let repo_id = repo.id.unwrap();

    let mut pkg = RepositoryPackage::new(
        repo_id,
        "pcre2".to_string(),
        "10.45-1.fc44".to_string(),
        "sha256:pcre2".to_string(),
        123,
        "https://example.invalid/pcre2.rpm".to_string(),
    );
    pkg.insert(&conn).unwrap();
    let pkg_id = pkg.id.unwrap();

    let mut provide = RepositoryProvide::new(
        pkg_id,
        "libpcre2-8.so.0()(64bit)".to_string(),
        Some("10.45-1.fc44".to_string()),
        "soname".to_string(),
        Some("libpcre2-8.so.0()(64bit)".to_string()),
    );
    provide.insert(&conn).unwrap();

    let dep_name = "libpcre2-8.so.0()(64bit)".to_string();
    let mut dep_plan = dep_resolution::DepResolutionPlan {
        unresolvable: vec![conary_core::resolver::MissingDependency {
            name: dep_name.clone(),
            constraint: VersionConstraint::Any,
            required_by: vec!["libselinux".to_string()],
        }],
        ..Default::default()
    };

    promote_repo_resolvable_satisfy_deps(&conn, &mut dep_plan);

    assert!(
        dep_plan.to_install.is_empty(),
        "live pcre2 runtime capabilities must not replace the running userspace"
    );
    assert!(dep_plan.unresolvable.is_empty());
    assert_eq!(dep_plan.blocked, vec![dep_name]);
}

#[test]
fn promote_uses_normalized_repository_provides_for_rpm_soname_deps() {
    let conn = test_db();

    let mut repo = Repository::new("fedora-remi".to_string(), "https://example.invalid".into());
    repo.insert(&conn).unwrap();
    let repo_id = repo.id.unwrap();

    let mut pkg = RepositoryPackage::new(
        repo_id,
        "glib2".to_string(),
        "2.86.0-2.fc44".to_string(),
        "sha256:glib2".to_string(),
        123,
        "https://example.invalid/glib2.rpm".to_string(),
    );
    pkg.insert(&conn).unwrap();
    let pkg_id = pkg.id.unwrap();

    let mut provide = RepositoryProvide::new(
        pkg_id,
        "libglib-2.0.so.0".to_string(),
        None,
        "soname".to_string(),
        Some("libglib-2.0.so.0()(64bit)".to_string()),
    );
    provide.insert(&conn).unwrap();

    let dep_name = "libglib-2.0.so.0()(64bit)".to_string();
    let mut dep_plan = dep_resolution::DepResolutionPlan {
        unresolvable: vec![conary_core::resolver::MissingDependency {
            name: dep_name.clone(),
            constraint: VersionConstraint::Any,
            required_by: vec!["qemu-img".to_string()],
        }],
        ..Default::default()
    };

    promote_repo_resolvable_satisfy_deps(&conn, &mut dep_plan);

    assert_eq!(dep_plan.to_install.len(), 1);
    assert_eq!(dep_plan.to_install[0].name, "libglib-2.0.so.0");
    assert!(dep_plan.unresolvable.is_empty());
}

#[test]
fn promote_does_not_convert_blocked_package_names_into_repo_installs() {
    let conn = test_db();

    let mut repo = Repository::new("fedora-remi".to_string(), "https://example.invalid".into());
    repo.insert(&conn).unwrap();
    let repo_id = repo.id.unwrap();

    for name in ["systemd", "coreutils"] {
        let mut pkg = RepositoryPackage::new(
            repo_id,
            name.to_string(),
            "1.0".to_string(),
            format!("sha256:{name}"),
            123,
            format!("https://example.invalid/{name}.rpm"),
        );
        pkg.insert(&conn).unwrap();
    }

    let mut dep_plan = dep_resolution::DepResolutionPlan {
        unresolvable: vec![
            conary_core::resolver::MissingDependency {
                name: "systemd".to_string(),
                constraint: VersionConstraint::Any,
                required_by: vec!["kernel-core".to_string()],
            },
            conary_core::resolver::MissingDependency {
                name: "coreutils".to_string(),
                constraint: VersionConstraint::Any,
                required_by: vec!["kernel-core".to_string()],
            },
        ],
        ..Default::default()
    };

    promote_repo_resolvable_satisfy_deps(&conn, &mut dep_plan);

    assert!(
        dep_plan.to_install.is_empty(),
        "blocked package names must never be promoted into repo installs"
    );
    assert!(dep_plan.unresolvable.is_empty());
    assert_eq!(dep_plan.blocked, vec!["systemd", "coreutils"]);
}

#[test]
fn detects_already_installed_errors_in_context_chain() {
    let error = anyhow::anyhow!("Package kernel-core version 6.17.1 is already installed")
        .context("Failed to install CCS dependency kernel-uname-r");

    assert!(is_already_installed_error(&error));
}

#[test]
fn default_dependency_passes_reach_kernel_initramfs_toolchain() {
    assert_eq!(
        DEFAULT_CCS_DEPENDENCY_PASSES, 2,
        "kernel installs must be able to resolve kernel-core -> dracut -> cpio"
    );
}
