// conary-core/src/ccs/convert/blocked_classes.rs

use crate::ccs::convert::command_evidence::CommandInvocation;
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockedClassOutcome {
    Review,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockedClass {
    pub id: &'static str,
    pub description: &'static str,
    pub default_outcome: BlockedClassOutcome,
    pub reason_code: &'static str,
    pub command_names: &'static [&'static str],
    pub command_forms: &'static [&'static str],
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
                &["dnf", "yum", "rpm", "apt", "apt-get", "dpkg", "pacman"],
                &[],
                "Model the dependency or transaction effect in Conary rather than nesting a package manager.",
            ),
            blocked_class(
                "pam",
                "Authentication stack mutation requires explicit policy support.",
                "blocked-class-pam",
                &["authselect", "pam-auth-update"],
                &[],
                "Add a native PAM policy adapter with operator-visible review.",
            ),
            blocked_class(
                "selinux",
                "SELinux policy and label mutation is not yet modeled.",
                "blocked-class-selinux",
                &["restorecon", "semanage", "setsebool"],
                &[],
                "Add a native SELinux policy adapter and label reconciliation plan.",
            ),
            blocked_class(
                "apparmor",
                "AppArmor policy mutation is not yet modeled.",
                "blocked-class-apparmor",
                &["apparmor_parser", "aa-enforce", "aa-disable"],
                &[],
                "Add a native AppArmor policy adapter and profile lifecycle model.",
            ),
            blocked_class(
                "kernel-module",
                "Kernel module mutation is not replay-safe without kernel compatibility policy.",
                "blocked-class-kernel-module",
                &["modprobe", "depmod", "dkms"],
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
                &["chmod u+s*", "chmod 4*"],
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
                &[
                    "ldconfig -p*",
                    "ldconfig -l*",
                    "ldconfig -n*",
                    "ldconfig -N*",
                    "ldconfig -X*",
                    "ldconfig -C*",
                    "ldconfig -f*",
                    "ldconfig -r*",
                    "ldconfig /*",
                ],
                "Add a dynamic-linker adapter that models the specific root/cache/link semantics.",
            ),
            review_class(
                "systemd-runtime-action",
                "systemd runtime service actions signal a live manager and are not passive metadata changes.",
                "review-class-systemd-runtime-action",
                &["service", "invoke-rc.d"],
                &[
                    "systemctl start*",
                    "systemctl stop*",
                    "systemctl restart*",
                    "systemctl try-restart*",
                    "systemctl reload*",
                    "systemctl reload-or-restart*",
                    "systemctl enable --now*",
                    "systemctl disable --now*",
                    "systemctl preset --now*",
                    "systemctl preset-all*",
                    "service *",
                    "invoke-rc.d *",
                ],
                "Add modeled service runtime semantics or keep the package review-only.",
            ),
            review_class(
                "systemd-user-scope",
                "systemd user/global scope enablement is target-user policy, not package-global metadata.",
                "review-class-systemd-user-scope",
                &[],
                &["systemctl --user*", "systemctl --global*"],
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
                &[
                    "systemd-tmpfiles *--remove*",
                    "systemd-tmpfiles *--clean*",
                    "systemd-tmpfiles *--purge*",
                    "systemd-tmpfiles *--boot*",
                    "systemd-tmpfiles *--user*",
                    "systemd-tmpfiles *--replace*",
                ],
                "Add tmpfiles lifecycle semantics and remove/purge ordering tests.",
            ),
            review_class(
                "sysusers-nonstandard",
                "sysusers root, replace, or stdin forms need explicit target-root and input modeling.",
                "review-class-sysusers-nonstandard",
                &[],
                &["systemd-sysusers *--replace*", "systemd-sysusers *--root*"],
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
                &[
                    "update-alternatives *--config*",
                    "update-alternatives *--set*",
                    "update-alternatives *--auto*",
                    "update-alternatives *--all*",
                    "update-alternatives *--remove-all*",
                    "alternatives *--config*",
                    "alternatives *--set*",
                    "alternatives *--auto*",
                    "alternatives *--all*",
                    "alternatives *--remove-all*",
                ],
                "Model administrator alternatives state before claiming replacement.",
            ),
            review_class(
                "cache-refresh-nonstandard",
                "Cache refresh command uses nonstandard paths or options outside the bootstrap adapter contract.",
                "review-class-cache-refresh-nonstandard",
                &[],
                &[
                    "update-mime-database */opt*",
                    "update-mime-database */usr/local*",
                    "update-desktop-database */opt*",
                    "update-desktop-database */usr/local*",
                    "gtk-update-icon-cache */opt*",
                    "gtk-update-icon-cache */usr/local*",
                    "glib-compile-schemas */opt*",
                    "glib-compile-schemas */usr/local*",
                    "fc-cache */opt*",
                    "fc-cache */usr/local*",
                ],
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
                &["udevadm trigger*", "udevadm control*"],
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
        let form = invocation_form(invocation);
        self.classes.iter().find(|class| {
            class
                .command_names
                .iter()
                .any(|command| *command == invocation.command)
                || class
                    .command_forms
                    .iter()
                    .any(|pattern| form_matches(pattern, &form))
        })
    }
}

fn blocked_class(
    id: &'static str,
    description: &'static str,
    reason_code: &'static str,
    command_names: &'static [&'static str],
    command_forms: &'static [&'static str],
    unblock_criteria: &'static str,
) -> BlockedClass {
    BlockedClass {
        id,
        description,
        default_outcome: BlockedClassOutcome::Blocked,
        reason_code,
        command_names,
        command_forms,
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
    command_forms: &'static [&'static str],
    unblock_criteria: &'static str,
) -> BlockedClass {
    BlockedClass {
        id,
        description,
        default_outcome: BlockedClassOutcome::Review,
        reason_code,
        command_names,
        command_forms,
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
        command_forms: &[],
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

fn invocation_form(invocation: &CommandInvocation) -> String {
    if invocation.argv.is_empty() {
        invocation.command.clone()
    } else {
        format!("{} {}", invocation.command, invocation.argv.join(" "))
    }
}

fn form_matches(pattern: &str, form: &str) -> bool {
    if !pattern.contains('*') {
        return pattern == form;
    }

    let parts: Vec<&str> = pattern.split('*').collect();
    let mut position = 0;

    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if index == 0 && !pattern.starts_with('*') {
            if !form[position..].starts_with(part) {
                return false;
            }
            position += part.len();
        } else if index == parts.len() - 1 && !pattern.ends_with('*') {
            if !form.ends_with(part) {
                return false;
            }
            let start = form.len() - part.len();
            if start < position {
                return false;
            }
            position = form.len();
        } else if let Some(offset) = form[position..].find(part) {
            position += offset + part.len();
        } else {
            return false;
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::convert::command_evidence::{CommandEvidenceSource, CommandInvocation};

    fn invocation(command: &str, argv: &[&str]) -> CommandInvocation {
        CommandInvocation {
            id: format!("entry:line0:cmd0:{command}"),
            entry_id: "entry".to_string(),
            source: CommandEvidenceSource::StaticSignal,
            phase: Some("post-install".to_string()),
            lifecycle_paths: vec!["post-install".to_string()],
            interpreter: Some("/bin/sh".to_string()),
            command: command.to_string(),
            argv: argv.iter().map(|arg| arg.to_string()).collect(),
            raw_line: Some(format!("{} {}", command, argv.join(" ")).trim().to_string()),
            cwd: None,
            environment: vec![],
        }
    }

    #[test]
    fn blocked_classes_block_network_and_package_manager_recursion() {
        let registry = BlockedClassRegistry::default();

        let network = registry.match_invocation(&invocation("curl", &["https://example.invalid"]));
        assert_eq!(network.unwrap().reason_code, "blocked-class-network");

        let pm = registry.match_invocation(&invocation("dnf", &["install", "foo"]));
        assert_eq!(
            pm.unwrap().reason_code,
            "blocked-class-package-manager-recursion"
        );
    }

    #[test]
    fn blocked_classes_mark_dbus_and_debconf_for_review() {
        let registry = BlockedClassRegistry::default();

        let dbus =
            registry.match_invocation(&invocation("dbus-update-activation-environment", &[]));
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
    fn blocked_classes_match_command_forms() {
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
    }

    #[test]
    fn blocked_classes_review_systemd_runtime_user_and_deb_helpers() {
        let registry = BlockedClassRegistry::default();

        let runtime =
            registry.match_invocation(&invocation("systemctl", &["restart", "demo.service"]));
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

        let tmpfiles_remove =
            registry.match_invocation(&invocation("systemd-tmpfiles", &["--remove"]));
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
}
