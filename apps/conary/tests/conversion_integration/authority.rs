// tests/conversion_integration/authority.rs

use super::*;

#[test]
fn golden_conversion_native_free_is_current_without_entries() {
    let temp_dir = TempDir::new().unwrap();
    let converter = passive_converter(temp_dir.path());
    let metadata = create_test_metadata("adapter-registry-native-free");
    let files = create_test_files("adapter-registry-native-free");

    let result = converter
        .convert_in_memory_for_test(
            &metadata,
            &files,
            "rpm",
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .expect("native-free conversion succeeds");
    let parsed = parse_converted_package(&result);
    let bundle = parsed
        .manifest()
        .native_lifecycle
        .as_ref()
        .expect("converted package should carry scriptlet bundle");

    assert!(bundle.entries.is_empty());
    assert_eq!(bundle.scriptlet_fidelity, ScriptletFidelity::NativeFree);
    assert_eq!(result.scriptlet_metadata.scriptlet_fidelity, "native-free");
}

#[test]
fn golden_conversion_preserves_source_lifecycle_without_static_effect_authority() {
    let temp_dir = TempDir::new().unwrap();
    let converter = passive_converter(temp_dir.path());
    let mut metadata = create_test_metadata("adapter-registry-fully-replaced");
    metadata.native_scriptlet_abi = vec![rpm_lifecycle_entry(
        "%post",
        RpmScriptletSlot::Post,
        NativeLifecyclePath::PostInstall,
        NativeTransactionPosition::AfterPayload,
        "\
/sbin/ldconfig
systemctl daemon-reload
systemctl enable demo.service
systemd-tmpfiles --create /usr/lib/tmpfiles.d/demo.conf
systemd-sysusers /usr/lib/sysusers.d/demo.conf
update-mime-database /usr/share/mime
restorecon -R /usr/bin/adapter-registry-fully-replaced
semanage fcontext -a -t demo_exec_t /usr/bin/adapter-registry-fully-replaced
semodule -i /usr/share/selinux/packages/demo.pp
setsebool -P demo_can_network on
apparmor_parser -r /etc/apparmor.d/usr.bin.demo
update-alternatives --install /usr/bin/editor editor /usr/bin/demo-editor 50
",
    )];
    let files = golden_payload_files("adapter-registry-fully-replaced");

    let result = converter
        .convert_in_memory_for_test(
            &metadata,
            &files,
            "rpm",
            "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        )
        .expect("adapter-backed conversion succeeds");
    let parsed = parse_converted_package(&result);
    let bundle = parsed
        .manifest()
        .native_lifecycle
        .as_ref()
        .expect("converted package should carry scriptlet bundle");

    assert_eq!(
        bundle.scriptlet_fidelity,
        ScriptletFidelity::NativeLifecycle
    );
    let entry = bundle.entries.first().expect("preserved lifecycle entry");
    assert!(entry.body.contains("/sbin/ldconfig"));
    assert!(entry.body.contains("restorecon -R"));
    assert!(entry.body.contains("apparmor_parser -r"));
    assert_eq!(
        result.scriptlet_metadata.scriptlet_fidelity,
        "native-lifecycle"
    );
}

#[test]
fn golden_conversion_unknown_command_preserves_typed_lifecycle() {
    let temp_dir = TempDir::new().unwrap();
    let converter = passive_converter(temp_dir.path());
    let mut metadata = create_test_metadata("lifecycle-execution-unknown-shell");
    metadata.native_scriptlet_abi = vec![rpm_lifecycle_entry(
        "%post",
        RpmScriptletSlot::Post,
        NativeLifecyclePath::PostInstall,
        NativeTransactionPosition::AfterPayload,
        "custom-helper --do-thing\n",
    )];
    let files = create_test_files("lifecycle-execution-unknown-shell");

    let result = converter
        .convert_in_memory_for_test(
            &metadata,
            &files,
            "rpm",
            "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
        )
        .expect("unknown shell conversion succeeds");
    let parsed = parse_converted_package(&result);
    let bundle = parsed
        .manifest()
        .native_lifecycle
        .as_ref()
        .expect("converted package should carry scriptlet bundle");

    assert_eq!(
        bundle.scriptlet_fidelity,
        ScriptletFidelity::NativeLifecycle
    );
    assert!(
        bundle
            .entries
            .iter()
            .any(|entry| entry.body == "custom-helper --do-thing\n")
    );
    assert_eq!(
        result.scriptlet_metadata.scriptlet_fidelity,
        "native-lifecycle"
    );
}
