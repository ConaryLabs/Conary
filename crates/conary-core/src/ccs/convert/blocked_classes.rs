// conary-core/src/ccs/convert/blocked_classes.rs

use crate::ccs::convert::command_evidence::CommandInvocation;
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockedClassOutcome {
    Review,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockedCommandRule {
    ChmodSetId,
    LdconfigNonstandard,
    SystemdRuntimeAction,
    SystemdUserScope,
    TmpfilesNoncreate,
    SysusersNonstandard,
    AlternativesInteractiveOrBroad,
    CacheRefreshNonstandard,
    UdevMutation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockedClass {
    pub id: &'static str,
    pub description: &'static str,
    pub default_outcome: BlockedClassOutcome,
    pub reason_code: &'static str,
    pub command_names: &'static [&'static str],
    pub command_rules: &'static [BlockedCommandRule],
    pub affected_formats: &'static [&'static str],
    pub preview_distros: &'static [&'static str],
    pub unblock_criteria: &'static str,
}

#[derive(Debug, Clone)]
pub struct BlockedClassRegistry {
    classes: Vec<BlockedClass>,
}

impl Default for BlockedClassRegistry {
    fn default() -> Self {
        let classes = vec![
            blocked_class(
                "network",
                "Network access from scriptlets is not replay-safe.",
                "blocked-class-network",
                &["curl", "wget", "scp", "ssh"],
                &[],
                "Provide a declared package dependency or a curated offline artifact.",
            ),
            blocked_class(
                "package-manager-recursion",
                "Scriptlets must not invoke a foreign or nested package manager.",
                "blocked-class-package-manager-recursion",
                &[
                    "apk", "apt", "apt-get", "dnf", "dnf5", "dpkg", "microdnf", "pacman", "rpm",
                    "yum", "zypper",
                ],
                &[],
                "Model the dependency or transaction effect in Conary rather than nesting a package manager.",
            ),
            blocked_class(
                "pam",
                "Authentication stack mutation requires explicit policy support.",
                "blocked-class-pam",
                &["authconfig", "authselect", "pam-auth-update", "pam-config"],
                &[],
                "Add a native PAM policy adapter with operator-visible review.",
            ),
            blocked_class(
                "selinux",
                "Unsupported SELinux policy and label mutation is not replay-safe.",
                "blocked-class-selinux",
                &[
                    "restorecon",
                    "fixfiles",
                    "semanage",
                    "semodule",
                    "setsebool",
                ],
                &[],
                "Use selinux-policy/v1 for supported declarative forms, or add adapter coverage for the specific unsupported mutation.",
            ),
            blocked_class(
                "apparmor",
                "AppArmor policy mutation is not yet modeled.",
                "blocked-class-apparmor",
                &[
                    "apparmor_parser",
                    "aa-enforce",
                    "aa-complain",
                    "aa-disable",
                    "aa-status",
                ],
                &[],
                "Add a native AppArmor policy adapter and profile lifecycle model.",
            ),
            blocked_class(
                "kernel-module",
                "Kernel module mutation is not replay-safe without kernel compatibility policy.",
                "blocked-class-kernel-module",
                &["modprobe", "depmod", "dkms", "kernel-install"],
                &[],
                "Add a native kernel-module policy adapter and target-kernel compatibility checks.",
            ),
            blocked_class(
                "initramfs",
                "Initramfs regeneration is target-boot critical and not yet modeled.",
                "blocked-class-initramfs",
                &["dracut", "mkinitcpio", "update-initramfs"],
                &[],
                "Add a native initramfs adapter with boot artifact validation.",
            ),
            blocked_class(
                "bootloader",
                "Bootloader mutation is target-boot critical and not yet modeled.",
                "blocked-class-bootloader",
                &["grub-mkconfig", "grub2-mkconfig", "update-grub", "bootctl"],
                &[],
                "Add a native bootloader adapter with explicit generation and rollback semantics.",
            ),
            blocked_class(
                "setuid-setcap",
                "Setuid and file capability mutation is security-sensitive.",
                "blocked-class-setuid-setcap",
                &["setcap", "setpriv"],
                &[BlockedCommandRule::ChmodSetId],
                "Model executable privilege metadata in the package manifest and verify it at install time.",
            ),
            blocked_class(
                "sysctl",
                "Kernel parameter mutation is not yet modeled.",
                "blocked-class-sysctl",
                &["sysctl"],
                &[],
                "Add a native sysctl policy adapter with target policy validation.",
            ),
            blocked_class(
                "legacy-init",
                "Legacy init service registration is not represented by the systemd adapter set.",
                "blocked-class-legacy-init",
                &["chkconfig", "update-rc.d", "rc-update"],
                &[],
                "Add native SysVinit/OpenRC service adapters or convert the package to supported service metadata.",
            ),
            blocked_metadata_class(
                "native-abi-unpreservable",
                BlockedClassOutcome::Blocked,
                "Parser-level native ABI evidence marked unpreservable.",
                "blocked-class-native-abi-unpreservable",
                "Fix the native parser fidelity gap or provide an explicit curated transform.",
            ),
            review_class(
                "dbus-policy",
                "D-Bus activation, service, or policy mutation needs review.",
                "review-class-dbus-policy",
                &["dbus-update-activation-environment", "dbus-send"],
                &[],
                "Add a native D-Bus service/policy adapter or prove the command is a harmless cache refresh.",
            ),
            review_class(
                "ldconfig-nonstandard",
                "ldconfig forms with custom roots, caches, link-only modes, print modes, or explicit directories need review.",
                "review-class-ldconfig-nonstandard",
                &[],
                &[BlockedCommandRule::LdconfigNonstandard],
                "Add a dynamic-linker adapter that models the specific root/cache/link semantics.",
            ),
            review_class(
                "systemd-runtime-action",
                "systemd runtime service actions signal a live manager and are not passive metadata changes.",
                "review-class-systemd-runtime-action",
                &["service", "invoke-rc.d"],
                &[BlockedCommandRule::SystemdRuntimeAction],
                "Add modeled service runtime semantics or keep the package review-only.",
            ),
            review_class(
                "systemd-user-scope",
                "systemd user/global scope enablement is target-user policy, not package-global metadata.",
                "review-class-systemd-user-scope",
                &[],
                &[BlockedCommandRule::SystemdUserScope],
                "Add user-scope service policy and target compatibility checks.",
            ),
            review_class(
                "deb-systemd-helper",
                "DEB systemd helper state is dpkg-family private and must not require installing dpkg helpers on foreign targets.",
                "review-class-deb-systemd-helper",
                &["deb-systemd-helper", "deb-systemd-invoke"],
                &[],
                "Model DEB helper state explicitly or require same-family review policy.",
            ),
            review_class(
                "tmpfiles-noncreate",
                "tmpfiles cleanup, removal, boot-only, user, purge, replace, or stdin forms need lifecycle-specific review.",
                "review-class-tmpfiles-noncreate",
                &[],
                &[BlockedCommandRule::TmpfilesNoncreate],
                "Add tmpfiles lifecycle semantics and remove/purge ordering tests.",
            ),
            review_class(
                "sysusers-nonstandard",
                "sysusers root, replace, or stdin forms need explicit target-root and input modeling.",
                "review-class-sysusers-nonstandard",
                &[],
                &[BlockedCommandRule::SysusersNonstandard],
                "Add sysusers root/input modeling before claiming replacement.",
            ),
            review_class(
                "gconf-schema",
                "GConf schema installation mutates an obsolete desktop configuration registry.",
                "review-class-gconf-schema",
                &["gconftool", "gconftool-2"],
                &[],
                "Migrate obsolete GConf schemas to GSettings XML schemas and glib-compile-schemas.",
            ),
            review_class(
                "install-info",
                "GNU Info directory registration is a common documentation index mutation that is not yet modeled.",
                "review-class-install-info",
                &["install-info"],
                &[],
                "Model Info manual registration as a declarative documentation index/cache effect.",
            ),
            review_class(
                "alternatives-interactive-or-broad",
                "Interactive or broad alternatives commands can alter administrator choice state.",
                "review-class-alternatives-interactive-or-broad",
                &[],
                &[BlockedCommandRule::AlternativesInteractiveOrBroad],
                "Model administrator alternatives state before claiming replacement.",
            ),
            review_class(
                "cache-refresh-nonstandard",
                "Cache refresh command uses nonstandard paths or options outside the bootstrap adapter contract.",
                "review-class-cache-refresh-nonstandard",
                &[],
                &[BlockedCommandRule::CacheRefreshNonstandard],
                "Add a cache-specific adapter rule for the nonstandard path or keep package review-only.",
            ),
            blocked_metadata_class(
                "rpm-verify",
                BlockedClassOutcome::Review,
                "RPM verify scriptlets execute under rpm verification rather than install/update/remove.",
                "review-class-rpm-verify",
                "Define verify-script policy or explicitly omit it from install replay with operator review.",
            ),
            blocked_metadata_class(
                "rpm-trigger",
                BlockedClassOutcome::Review,
                "RPM trigger execution requires target and transaction context.",
                "review-class-rpm-trigger",
                "Add trigger target matching and transaction ordering support.",
            ),
            blocked_metadata_class(
                "deb-trigger",
                BlockedClassOutcome::Review,
                "DEB trigger declarations require dpkg trigger semantics.",
                "review-class-deb-trigger",
                "Add a Conary-native trigger model or an explicit transform.",
            ),
            review_class(
                "debconf",
                "DEB config/debconf behavior is foreign runtime configuration evidence.",
                "review-class-debconf",
                &[
                    "debconf-communicate",
                    "debconf-set-selections",
                    "db_input",
                    "db_go",
                    "db_get",
                    "db_set",
                ],
                &[],
                "Provide modeled Conary-native configuration, source-family policy, or an operator-supplied transform; do not install dpkg/debconf on foreign targets.",
            ),
            review_class(
                "udev",
                "udev trigger/control operations affect host device state.",
                "review-class-udev",
                &[],
                &[BlockedCommandRule::UdevMutation],
                "Add target udev policy support or prove the package only ships static rules.",
            ),
            blocked_metadata_class(
                "arch-alpm-hook",
                BlockedClassOutcome::Review,
                "Arch ALPM hooks require transaction-level hook semantics.",
                "review-class-arch-alpm-hook",
                "Add ALPM hook ordering and target matching support.",
            ),
            blocked_metadata_class(
                "arch-install-function",
                BlockedClassOutcome::Review,
                "Arch .INSTALL function extraction or wrapper behavior requires review.",
                "review-class-arch-install-function",
                "Add modeled .INSTALL wrapper/replay behavior for the target lifecycle path.",
            ),
        ];
        assert_unique_class_ids(&classes);
        Self { classes }
    }
}

impl BlockedClassRegistry {
    pub fn classes(&self) -> &[BlockedClass] {
        &self.classes
    }

    pub fn class_by_id(&self, id: &str) -> Option<&BlockedClass> {
        self.classes.iter().find(|class| class.id == id)
    }

    pub fn match_invocation(&self, invocation: &CommandInvocation) -> Option<&BlockedClass> {
        self.classes.iter().find(|class| {
            (class.id == "network" && git_clone_subcommand_after_global_options(invocation))
                || class
                    .command_names
                    .iter()
                    .any(|command| *command == invocation.command)
                || class
                    .command_rules
                    .iter()
                    .any(|rule| command_rule_matches(*rule, invocation))
        })
    }
}

fn git_clone_subcommand_after_global_options(invocation: &CommandInvocation) -> bool {
    if invocation.command != "git" {
        return false;
    }

    let mut index = 0;
    while index < invocation.argv.len() {
        let arg = invocation.argv[index].as_str();
        match arg {
            "-C" | "-c" => {
                index += 2;
                continue;
            }
            "--config-env" | "--exec-path" | "--git-dir" | "--namespace" | "--super-prefix"
            | "--work-tree" => {
                index += 2;
                continue;
            }
            value if value.starts_with("--") && value.contains('=') => {
                index += 1;
                continue;
            }
            value if value.starts_with('-') => {
                index += 1;
                continue;
            }
            _ => break,
        }
    }

    invocation.argv.get(index).is_some_and(|arg| arg == "clone")
}

fn blocked_class(
    id: &'static str,
    description: &'static str,
    reason_code: &'static str,
    command_names: &'static [&'static str],
    command_rules: &'static [BlockedCommandRule],
    unblock_criteria: &'static str,
) -> BlockedClass {
    BlockedClass {
        id,
        description,
        default_outcome: BlockedClassOutcome::Blocked,
        reason_code,
        command_names,
        command_rules,
        affected_formats: &["rpm", "deb", "arch"],
        preview_distros: &["fedora", "ubuntu", "arch"],
        unblock_criteria,
    }
}

fn review_class(
    id: &'static str,
    description: &'static str,
    reason_code: &'static str,
    command_names: &'static [&'static str],
    command_rules: &'static [BlockedCommandRule],
    unblock_criteria: &'static str,
) -> BlockedClass {
    BlockedClass {
        id,
        description,
        default_outcome: BlockedClassOutcome::Review,
        reason_code,
        command_names,
        command_rules,
        affected_formats: &["rpm", "deb", "arch"],
        preview_distros: &["fedora", "ubuntu", "arch"],
        unblock_criteria,
    }
}

fn blocked_metadata_class(
    id: &'static str,
    default_outcome: BlockedClassOutcome,
    description: &'static str,
    reason_code: &'static str,
    unblock_criteria: &'static str,
) -> BlockedClass {
    BlockedClass {
        id,
        description,
        default_outcome,
        reason_code,
        command_names: &[],
        command_rules: &[],
        affected_formats: &["rpm", "deb", "arch"],
        preview_distros: &["fedora", "ubuntu", "arch"],
        unblock_criteria,
    }
}

fn assert_unique_class_ids(classes: &[BlockedClass]) {
    let mut seen = BTreeSet::new();
    for class in classes {
        assert!(
            seen.insert(class.id),
            "duplicate blocked class id: {}",
            class.id
        );
    }
}

fn command_rule_matches(rule: BlockedCommandRule, invocation: &CommandInvocation) -> bool {
    match rule {
        BlockedCommandRule::ChmodSetId => crate::security::command_risk::is_setid_chmod(invocation),
        BlockedCommandRule::LdconfigNonstandard => ldconfig_is_nonstandard(invocation),
        BlockedCommandRule::SystemdRuntimeAction => systemd_runtime_action(invocation),
        BlockedCommandRule::SystemdUserScope => {
            invocation.command == "systemctl"
                && invocation
                    .argv
                    .iter()
                    .any(|arg| matches!(arg.as_str(), "--user" | "--global"))
        }
        BlockedCommandRule::TmpfilesNoncreate => {
            invocation.command == "systemd-tmpfiles"
                && has_option_name(
                    &invocation.argv,
                    &[
                        "--remove",
                        "--clean",
                        "--purge",
                        "--boot",
                        "--user",
                        "--replace",
                    ],
                )
        }
        BlockedCommandRule::SysusersNonstandard => {
            invocation.command == "systemd-sysusers"
                && has_option_name(&invocation.argv, &["--replace", "--root"])
        }
        BlockedCommandRule::AlternativesInteractiveOrBroad => {
            matches!(
                invocation.command.as_str(),
                "update-alternatives" | "alternatives"
            ) && has_exact_option(
                &invocation.argv,
                &["--config", "--set", "--auto", "--all", "--remove-all"],
            )
        }
        BlockedCommandRule::CacheRefreshNonstandard => {
            matches!(
                invocation.command.as_str(),
                "update-mime-database"
                    | "update-desktop-database"
                    | "gtk-update-icon-cache"
                    | "glib-compile-schemas"
                    | "fc-cache"
            ) && invocation.argv.iter().any(|arg| {
                let path = std::path::Path::new(arg);
                path.is_absolute() && (path.starts_with("/opt") || path.starts_with("/usr/local"))
            })
        }
        BlockedCommandRule::UdevMutation => {
            invocation.command == "udevadm"
                && first_non_option(&invocation.argv)
                    .is_some_and(|arg| matches!(arg, "trigger" | "control"))
        }
    }
}

fn ldconfig_is_nonstandard(invocation: &CommandInvocation) -> bool {
    if invocation.command != "ldconfig" {
        return false;
    }
    invocation.argv.iter().any(|arg| {
        matches!(
            arg.as_str(),
            "-p" | "--print-cache" | "-l" | "-n" | "-N" | "-X" | "-C" | "-f" | "-r"
        ) || arg.starts_with("-C")
            || arg.starts_with("-f")
            || arg.starts_with("-r")
            || std::path::Path::new(arg).is_absolute()
    })
}

fn systemd_runtime_action(invocation: &CommandInvocation) -> bool {
    if invocation.command != "systemctl"
        || invocation
            .argv
            .iter()
            .any(|arg| matches!(arg.as_str(), "--user" | "--global"))
    {
        return false;
    }
    let Some(action) = systemctl_subcommand(&invocation.argv) else {
        return false;
    };
    matches!(
        action,
        "start"
            | "stop"
            | "restart"
            | "try-restart"
            | "reload"
            | "reload-or-restart"
            | "preset-all"
    ) || matches!(action, "enable" | "disable" | "preset")
        && invocation.argv.iter().any(|arg| arg == "--now")
}

fn systemctl_subcommand(argv: &[String]) -> Option<&str> {
    let mut index = 0;
    while let Some(arg) = argv.get(index) {
        match arg.as_str() {
            "--root" | "--image" | "--machine" | "--host" | "--type" | "--state" | "--property" => {
                index += 2
            }
            "--system" | "--user" | "--global" | "--runtime" | "--no-reload" | "--no-block"
            | "--no-pager" | "--no-legend" | "--plain" | "--quiet" | "--full" | "--force"
            | "--dry-run" => index += 1,
            value
                if value.starts_with("--root=")
                    || value.starts_with("--image=")
                    || value.starts_with("--machine=")
                    || value.starts_with("--host=")
                    || value.starts_with("--type=")
                    || value.starts_with("--state=")
                    || value.starts_with("--property=") =>
            {
                index += 1;
            }
            value if value.starts_with('-') => return None,
            value => return Some(value),
        }
    }
    None
}

fn has_exact_option(argv: &[String], options: &[&str]) -> bool {
    argv.iter().any(|arg| options.contains(&arg.as_str()))
}

fn has_option_name(argv: &[String], options: &[&str]) -> bool {
    argv.iter().any(|arg| {
        options.iter().any(|option| {
            arg == option
                || arg
                    .strip_prefix(option)
                    .is_some_and(|rest| rest.starts_with('='))
        })
    })
}

fn first_non_option(argv: &[String]) -> Option<&str> {
    argv.iter()
        .find(|arg| !arg.starts_with('-'))
        .map(String::as_str)
}

#[cfg(test)]
#[path = "blocked_classes/tests.rs"]
mod tests;
