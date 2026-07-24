// conary-core/src/ccs/convert/blocked_classes/tests.rs

use super::*;
use crate::ccs::convert::command_evidence::{CommandEvidenceSource, CommandInvocation};
use crate::ccs::legacy_scriptlets::{CommandArgumentProvenance, CommandExecutionContext};

fn invocation(command: &str, argv: &[&str]) -> CommandInvocation {
    CommandInvocation {
        id: format!("entry:line0:cmd0:{command}"),
        entry_id: "entry".to_string(),
        source: CommandEvidenceSource::ShellAst,
        phase: Some("post-install".to_string()),
        lifecycle_paths: vec!["post-install".to_string()],
        interpreter: Some("/bin/sh".to_string()),
        command: command.to_string(),
        command_provenance: CommandArgumentProvenance::Literal,
        argv: argv.iter().map(|arg| arg.to_string()).collect(),
        argument_provenance: vec![CommandArgumentProvenance::Literal; argv.len()],
        execution_context: CommandExecutionContext::Unconditional,
        pipeline_id: None,
        raw_line: Some(format!("{} {}", command, argv.join(" ")).trim().to_string()),
        cwd: None,
        environment: vec![],
    }
}

#[test]
fn blocked_classes_block_live_fetch_and_package_manager_recursion() {
    let registry = BlockedClassRegistry::default();

    for (command, argv) in [
        ("curl", vec!["https://example.invalid"]),
        ("wget", vec!["https://example.invalid/package.tar.gz"]),
        ("scp", vec!["host:/tmp/pkg", "/tmp/pkg"]),
        ("ssh", vec!["builder.example.invalid", "true"]),
        ("git", vec!["clone", "https://example.invalid/repo.git"]),
        (
            "git",
            vec!["-C", "/tmp", "clone", "https://example.invalid/repo.git"],
        ),
        (
            "git",
            vec![
                "-c",
                "http.sslVerify=false",
                "clone",
                "https://example.invalid/repo.git",
            ],
        ),
        (
            "git",
            vec![
                "--git-dir",
                "/tmp/repo",
                "clone",
                "https://example.invalid/repo.git",
            ],
        ),
        (
            "git",
            vec![
                "--work-tree",
                "/tmp/work",
                "clone",
                "https://example.invalid/repo.git",
            ],
        ),
        (
            "git",
            vec![
                "--config-env",
                "foo=BAR",
                "clone",
                "https://example.invalid/repo.git",
            ],
        ),
    ] {
        let class = registry
            .match_invocation(&invocation(command, &argv))
            .unwrap_or_else(|| panic!("missing network blocked class for {command}"));
        assert_eq!(class.id, "network");
        assert_eq!(class.reason_code, "blocked-class-network");
        assert_eq!(class.default_outcome, BlockedClassOutcome::Blocked);
    }

    assert!(
        registry
            .match_invocation(&invocation(
                "git",
                &["config", "--global", "demo.value", "1"]
            ))
            .is_none(),
        "only live-fetch git forms are blocked in this slice"
    );
    assert!(
        registry
            .match_invocation(&invocation("git", &["help", "clone"]))
            .is_none(),
        "git help clone is not a live-fetch clone form"
    );
    assert!(
        registry
            .match_invocation(&invocation(
                "git",
                &["config", "remote.origin.tagOpt", "clone"]
            ))
            .is_none(),
        "git config ... clone is not a live-fetch clone form"
    );

    for (command, argv) in [
        ("apk", vec!["add", "demo"]),
        ("apt", vec!["install", "demo"]),
        ("apt-get", vec!["install", "demo"]),
        ("dnf", vec!["install", "demo"]),
        ("dnf5", vec!["install", "demo"]),
        ("dpkg", vec!["-i", "demo.deb"]),
        ("microdnf", vec!["install", "demo"]),
        ("pacman", vec!["-S", "demo"]),
        ("rpm", vec!["-Uvh", "demo.rpm"]),
        ("yum", vec!["install", "demo"]),
        ("zypper", vec!["install", "demo"]),
    ] {
        let class = registry
            .match_invocation(&invocation(command, &argv))
            .unwrap_or_else(|| {
                panic!("missing package-manager recursion blocked class for {command}")
            });
        assert_eq!(class.id, "package-manager-recursion");
        assert_eq!(class.reason_code, "blocked-class-package-manager-recursion");
        assert_eq!(class.default_outcome, BlockedClassOutcome::Blocked);
    }
}

#[test]
fn blocked_classes_goal8_boundaries_have_stable_outcomes() {
    let registry = BlockedClassRegistry::default();
    let cases = [
        (
            "package-manager-recursion",
            BlockedClassOutcome::Blocked,
            "blocked-class-package-manager-recursion",
        ),
        (
            "rpm-trigger",
            BlockedClassOutcome::Review,
            "review-class-rpm-trigger",
        ),
        (
            "deb-trigger",
            BlockedClassOutcome::Review,
            "review-class-deb-trigger",
        ),
        (
            "arch-install-function",
            BlockedClassOutcome::Review,
            "review-class-arch-install-function",
        ),
    ];

    for (class_id, expected_outcome, expected_reason) in cases {
        let class = registry
            .class_by_id(class_id)
            .unwrap_or_else(|| panic!("missing blocked class {class_id}"));

        assert_eq!(
            class.default_outcome, expected_outcome,
            "{class_id} outcome"
        );
        assert_eq!(class.reason_code, expected_reason, "{class_id} reason");
    }
}

#[test]
fn blocked_classes_mark_dbus_and_debconf_for_review() {
    let registry = BlockedClassRegistry::default();

    let dbus = registry.match_invocation(&invocation("dbus-update-activation-environment", &[]));
    assert_eq!(dbus.unwrap().default_outcome, BlockedClassOutcome::Review);

    let debconf = registry.class_by_id("debconf").expect("debconf class");
    assert_eq!(debconf.reason_code, "review-class-debconf");
}

#[test]
fn blocked_classes_mark_rpm_verify_legacy_init_and_udev() {
    let registry = BlockedClassRegistry::default();

    let verify = registry
        .class_by_id("rpm-verify")
        .expect("rpm verify class");
    assert_eq!(verify.reason_code, "review-class-rpm-verify");

    let init = registry.match_invocation(&invocation("update-rc.d", &["demo", "defaults"]));
    assert_eq!(init.unwrap().reason_code, "blocked-class-legacy-init");

    let udev = registry.match_invocation(&invocation("udevadm", &["trigger"]));
    assert_eq!(udev.unwrap().default_outcome, BlockedClassOutcome::Review);
    assert_eq!(udev.unwrap().reason_code, "review-class-udev");

    assert!(
        registry
            .match_invocation(&invocation("udevadm", &["info"]))
            .is_none()
    );
}

#[test]
fn blocked_classes_cover_kernel_install_selinux_module_and_label_tools() {
    let registry = BlockedClassRegistry::default();

    for (command, argv, class_id) in [
        (
            "kernel-install",
            vec!["add", "6.10.0", "/lib/modules/6.10.0/vmlinuz"],
            "kernel-module",
        ),
        ("semodule", vec!["-i", "/tmp/demo.pp"], "selinux"),
        ("fixfiles", vec!["restore"], "selinux"),
    ] {
        let class = registry
            .match_invocation(&invocation(command, &argv))
            .unwrap_or_else(|| panic!("missing blocked class for {command}"));
        assert_eq!(class.id, class_id);
        assert_eq!(class.default_outcome, BlockedClassOutcome::Blocked);
    }
}

#[test]
fn blocked_classes_cover_common_pam_stack_helpers() {
    let registry = BlockedClassRegistry::default();

    for (command, argv) in [
        ("authselect", vec!["select", "sssd", "with-mkhomedir"]),
        ("authconfig", vec!["--enablefaillock", "--update"]),
        ("pam-auth-update", vec!["--package"]),
        ("pam-config", vec!["--add", "--mkhomedir"]),
    ] {
        let class = registry
            .match_invocation(&invocation(command, &argv))
            .unwrap_or_else(|| panic!("missing blocked class for {command}"));
        assert_eq!(class.id, "pam");
        assert_eq!(class.reason_code, "blocked-class-pam");
        assert_eq!(class.default_outcome, BlockedClassOutcome::Blocked);
    }
}

#[test]
fn blocked_classes_match_typed_chmod_grammar() {
    let registry = BlockedClassRegistry::default();

    let chmod_form = registry.match_invocation(&invocation("chmod", &["u+s", "/usr/bin/foo"]));
    assert_eq!(
        chmod_form.unwrap().reason_code,
        "blocked-class-setuid-setcap"
    );

    let chmod_mode = registry.match_invocation(&invocation("chmod", &["4755", "/usr/bin/foo"]));
    assert_eq!(
        chmod_mode.unwrap().reason_code,
        "blocked-class-setuid-setcap"
    );

    for argv in [
        vec!["g+s", "/usr/bin/foo"],
        vec!["+s", "/usr/bin/foo"],
        vec!["2755", "/usr/bin/foo"],
        vec!["6755", "/usr/bin/foo"],
    ] {
        let chmod_form = registry.match_invocation(&invocation("chmod", &argv));
        assert_eq!(
            chmod_form.unwrap().reason_code,
            "blocked-class-setuid-setcap"
        );
    }

    assert!(
        registry
            .match_invocation(&invocation("chmod", &["u+x", "/usr/bin/foo"]))
            .is_none()
    );
}

#[test]
fn typed_command_rules_do_not_match_substrings() {
    let registry = BlockedClassRegistry::default();

    assert!(
        registry
            .match_invocation(&invocation(
                "systemd-tmpfiles",
                &["--remove-old", "/usr/lib/tmpfiles.d/demo.conf"]
            ))
            .is_none()
    );
    assert!(
        registry
            .match_invocation(&invocation(
                "update-mime-database",
                &["/optional/share/mime"]
            ))
            .is_none()
    );
    assert!(
        registry
            .match_invocation(&invocation("systemctl", &["help", "restart"]))
            .is_none()
    );
}

#[test]
fn blocked_classes_review_systemd_runtime_user_and_deb_helpers() {
    let registry = BlockedClassRegistry::default();

    let runtime = registry.match_invocation(&invocation("systemctl", &["restart", "demo.service"]));
    assert_eq!(
        runtime.unwrap().reason_code,
        "review-class-systemd-runtime-action"
    );

    let service_without_args = registry.match_invocation(&invocation("service", &[]));
    assert_eq!(
        service_without_args.unwrap().reason_code,
        "review-class-systemd-runtime-action"
    );

    let invoke_rc_without_args = registry.match_invocation(&invocation("invoke-rc.d", &[]));
    assert_eq!(
        invoke_rc_without_args.unwrap().reason_code,
        "review-class-systemd-runtime-action"
    );

    let user = registry.match_invocation(&invocation(
        "systemctl",
        &["--user", "enable", "demo.service"],
    ));
    assert_eq!(user.unwrap().reason_code, "review-class-systemd-user-scope");

    let deb = registry.match_invocation(&invocation(
        "deb-systemd-helper",
        &["enable", "demo.service"],
    ));
    assert_eq!(deb.unwrap().reason_code, "review-class-deb-systemd-helper");

    let preset_all = registry.match_invocation(&invocation("systemctl", &["preset-all"]));
    assert_eq!(
        preset_all.unwrap().reason_code,
        "review-class-systemd-runtime-action"
    );
}

#[test]
fn blocked_classes_review_tmpfiles_and_sysusers_unsupported_forms() {
    let registry = BlockedClassRegistry::default();

    let tmpfiles_remove = registry.match_invocation(&invocation("systemd-tmpfiles", &["--remove"]));
    assert_eq!(
        tmpfiles_remove.unwrap().reason_code,
        "review-class-tmpfiles-noncreate"
    );

    let tmpfiles_boot =
        registry.match_invocation(&invocation("systemd-tmpfiles", &["--boot", "--create"]));
    assert_eq!(
        tmpfiles_boot.unwrap().reason_code,
        "review-class-tmpfiles-noncreate"
    );

    let tmpfiles_create_boot =
        registry.match_invocation(&invocation("systemd-tmpfiles", &["--create", "--boot"]));
    assert_eq!(
        tmpfiles_create_boot.unwrap().reason_code,
        "review-class-tmpfiles-noncreate"
    );

    let sysusers_replace = registry.match_invocation(&invocation(
        "systemd-sysusers",
        &["--replace=/usr/lib/sysusers.d/demo.conf"],
    ));
    assert_eq!(
        sysusers_replace.unwrap().reason_code,
        "review-class-sysusers-nonstandard"
    );

    let sysusers_root =
        registry.match_invocation(&invocation("systemd-sysusers", &["--root=/tmp/root"]));
    assert_eq!(
        sysusers_root.unwrap().reason_code,
        "review-class-sysusers-nonstandard"
    );

    let sysusers_late_root = registry.match_invocation(&invocation(
        "systemd-sysusers",
        &["/usr/lib/sysusers.d/demo.conf", "--root=/tmp/root"],
    ));
    assert_eq!(
        sysusers_late_root.unwrap().reason_code,
        "review-class-sysusers-nonstandard"
    );
}

#[test]
fn blocked_classes_review_gconf_and_install_info_helpers() {
    let registry = BlockedClassRegistry::default();

    let gconf = registry.match_invocation(&invocation(
        "gconftool-2",
        &["--makefile-install-rule", "/etc/gconf/schemas/demo.schemas"],
    ));
    assert_eq!(gconf.unwrap().reason_code, "review-class-gconf-schema");

    let info = registry.match_invocation(&invocation(
        "install-info",
        &["/usr/share/info/demo.info.gz", "/usr/share/info/dir"],
    ));
    assert_eq!(info.unwrap().reason_code, "review-class-install-info");
}
