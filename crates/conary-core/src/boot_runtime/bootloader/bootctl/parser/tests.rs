// crates/conary-core/src/boot_runtime/bootloader/bootctl/parser/tests.rs

use super::*;

#[test]
fn covers_all_path_policy_and_install_options() {
    let parsed = parse_bootctl(&[
        "--esp-path=/efi",
        "--path=/efi2",
        "--boot-path=/boot",
        "--root=/root",
        "--image-policy=root=verity",
        "--install-source=host",
        "--variables=auto",
        "--no-variables",
        "--random-seed=yes",
        "--no-pager",
        "--graceful",
        "-q",
        "--entry-token=literal:demo",
        "--make-entry-directory=yes",
        "--make-machine-id-directory=auto",
        "--json=pretty",
        "--all-architectures",
        "--efi-boot-option-description=Conary",
        "--efi-boot-option-description-with-device=no",
        "--secure-boot-auto-enroll=yes",
        "--private-key=/key",
        "--private-key-source=provider:pkcs11",
        "--certificate=/cert",
        "--certificate-source=provider:store",
        "--keep-free=1G500M",
        "--entry-title=Conary",
        "--entry-version=1.2^3",
        "--entry-commit=7",
        "-X/extra.cred",
        "--tries-left=3",
        "install",
    ])
    .unwrap();
    assert_eq!(parsed.action, BootctlAction::Install);
    assert_eq!(parsed.options.len(), 30);
}

#[test]
fn covers_every_print_selector_and_declared_alias() {
    for args in [
        vec!["-p"],
        vec!["--print-path"],
        vec!["-x"],
        vec!["--print-loader-path"],
        vec!["--print-stub-path"],
        vec!["-R"],
        vec!["-RR"],
        vec!["--print-efi-architecture"],
    ] {
        assert!(
            matches!(
                parse_bootctl(&args).unwrap().action,
                BootctlAction::Print(_)
            ),
            "{args:?}"
        );
    }
    assert!(matches!(
        parse_bootctl(&["-RR"]).unwrap().action,
        BootctlAction::Print(BootctlPrintTarget::RootDisk)
    ));
    assert!(parse_bootctl(&["-p", "-x"]).is_err());
}

#[test]
fn covers_all_status_and_mutation_verbs() {
    assert_eq!(parse_bootctl(&[]).unwrap().action, BootctlAction::Status);
    assert_eq!(
        parse_bootctl(&["reboot-to-firmware"]).unwrap().action,
        BootctlAction::RebootToFirmwareQuery
    );
    assert_eq!(
        parse_bootctl(&["reboot-to-firmware", "yes"])
            .unwrap()
            .action,
        BootctlAction::RebootToFirmwareSet(true)
    );
    assert_eq!(
        parse_bootctl(&["list"]).unwrap().action,
        BootctlAction::List
    );
    assert!(matches!(
        parse_bootctl(&["unlink", "demo"]).unwrap().action,
        BootctlAction::Unlink(Some(_))
    ));
    assert!(matches!(
        parse_bootctl(&["--oldest=yes", "unlink"]).unwrap().action,
        BootctlAction::Unlink(None)
    ));
    assert!(matches!(
        parse_bootctl(&["link", "/vmlinuz"]).unwrap().action,
        BootctlAction::Link(_)
    ));
    assert_eq!(
        parse_bootctl(&["link-auto"]).unwrap().action,
        BootctlAction::LinkAuto
    );
    assert_eq!(
        parse_bootctl(&["cleanup"]).unwrap().action,
        BootctlAction::Cleanup
    );
    assert_eq!(
        parse_bootctl(&["install"]).unwrap().action,
        BootctlAction::Install
    );
    assert_eq!(
        parse_bootctl(&["update"]).unwrap().action,
        BootctlAction::Update
    );
    assert_eq!(
        parse_bootctl(&["remove"]).unwrap().action,
        BootctlAction::Remove
    );
    assert_eq!(
        parse_bootctl(&["is-installed"]).unwrap().action,
        BootctlAction::IsInstalled
    );
    assert_eq!(
        parse_bootctl(&["random-seed"]).unwrap().action,
        BootctlAction::RandomSeed
    );
    assert!(matches!(
        parse_bootctl(&["kernel-identify", "/vmlinuz"])
            .unwrap()
            .action,
        BootctlAction::KernelIdentify(_)
    ));
    assert!(matches!(
        parse_bootctl(&["kernel-inspect", "/vmlinuz"])
            .unwrap()
            .action,
        BootctlAction::KernelInspect(_)
    ));
}

#[test]
fn covers_entry_target_timeout_and_information_branches() {
    for (verb, value) in [
        ("set-default", "@current"),
        ("set-oneshot", "@oneshot"),
        ("set-sysfail", "@sysfail"),
        ("set-preferred", "@saved"),
    ] {
        assert!(parse_bootctl(&[verb, value]).is_ok(), "{verb}");
    }
    for (verb, value) in [
        ("set-timeout", "menu-force"),
        ("set-timeout", "5s"),
        ("set-timeout-oneshot", "1min 30s"),
        ("set-timeout", "1m"),
        ("set-timeout", "5μs"),
        ("set-timeout", "1 2"),
        ("set-timeout", " infinity "),
    ] {
        assert!(parse_bootctl(&[verb, value]).is_ok(), "{verb} {value}");
    }
    assert_eq!(
        parse_bootctl(&["--help"]).unwrap().action,
        BootctlAction::Help
    );
    assert_eq!(
        parse_bootctl(&["--version"]).unwrap().action,
        BootctlAction::Version
    );
    assert_eq!(
        parse_bootctl(&["--json=help"]).unwrap().action,
        BootctlAction::JsonHelp
    );
}

#[test]
fn enforces_cross_option_and_value_contracts() {
    assert!(parse_bootctl(&["--root=/r", "--image=/i"]).is_err());
    assert!(parse_bootctl(&["--install-source=host"]).is_err());
    assert!(parse_bootctl(&["--dry-run", "install"]).is_err());
    assert!(parse_bootctl(&["--dry-run", "cleanup"]).is_ok());
    assert!(parse_bootctl(&["--secure-boot-auto-enroll=yes", "install"]).is_err());
    assert!(parse_bootctl(&["--entry-token=literal:a/b"]).is_err());
    assert!(parse_bootctl(&["--variables=maybe"]).is_err());
    assert!(parse_bootctl(&["--variables="]).is_ok());
    assert!(parse_bootctl(&["--make-entry-directory="]).is_err());
    assert!(parse_bootctl(&["--install-source=auto"]).is_ok());
    assert!(parse_bootctl(&["--keep-free=1Q"]).is_err());
    assert!(parse_bootctl(&["--entry-version=1+2"]).is_err());
    assert!(parse_bootctl(&["--entry-commit=0"]).is_err());
    assert!(parse_bootctl(&["--tries-left=4294967295"]).is_err());
    assert!(parse_bootctl(&["set-default", "@future"]).is_err());
    assert!(parse_bootctl(&["set-timeout", "later"]).is_err());
    assert!(parse_bootctl(&["set-timeout", ".5s"]).is_err());
    assert!(parse_bootctl(&["set-timeout", "3."]).is_err());
    assert!(parse_bootctl(&["unlink"]).is_err());
    assert!(parse_bootctl(&["--oldest=yes", "unlink", "id"]).is_err());
    assert!(parse_bootctl(&["--future"]).is_err());
}

#[test]
fn follows_systemd_iec_size_grammar() {
    assert_eq!(parse_size("--keep-free", "3.5 K").unwrap(), 3 * 1024 + 512);
    assert_eq!(
        parse_size("--keep-free", "4 M 11.5K").unwrap(),
        4 * 1024 * 1024 + 11 * 1024 + 512
    );
    assert_eq!(
        parse_size("--keep-free", "3.5G3B").unwrap(),
        3 * 1024 * 1024 * 1024 + 512 * 1024 * 1024 + 3
    );
    assert!(parse_size("--keep-free", ".5K").is_err());
    assert!(parse_size("--keep-free", "3B3.5G").is_err());
    assert!(parse_size("--keep-free", "12P12P").is_err());
    assert!(parse_size("--keep-free", "1024E").is_err());
}
