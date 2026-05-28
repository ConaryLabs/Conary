// conary-core/tests/native_abi.rs

use conary_core::db::models::{Trove, TroveType};
use conary_core::packages::traits::{
    ArchNativeScriptletMetadata, DebTriggerAwaitMode, NativeArgumentValue, NativeLifecyclePath,
    NativeScriptletKind, NativeScriptletMetadata, NativeStdinContract, PackageFile, PackageFormat,
    ScriptletPhase,
};
use conary_core::packages::{arch::ArchPackage, deb::DebPackage, rpm::RpmPackage};
use flate2::{Compression, write::GzEncoder};
use std::collections::BTreeSet;
use std::fs::File;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

#[test]
fn package_format_trait_exposes_native_abi_default_empty_for_test_double() {
    struct EmptyPackage;

    impl PackageFormat for EmptyPackage {
        fn parse(_path: &str) -> conary_core::Result<Self> {
            Ok(Self)
        }

        fn name(&self) -> &str {
            "empty"
        }

        fn version(&self) -> &str {
            "0"
        }

        fn architecture(&self) -> Option<&str> {
            None
        }

        fn description(&self) -> Option<&str> {
            None
        }

        fn files(&self) -> &[PackageFile] {
            &[]
        }

        fn dependencies(&self) -> &[conary_core::packages::traits::Dependency] {
            &[]
        }

        fn extract_file_contents(
            &self,
        ) -> conary_core::Result<Vec<conary_core::packages::traits::ExtractedFile>> {
            Ok(Vec::new())
        }

        fn to_trove(&self) -> Trove {
            Trove::new("empty".to_string(), "0".to_string(), TroveType::Package)
        }
    }

    let package = EmptyPackage;

    assert!(package.native_scriptlet_abi().is_empty());
}

#[test]
fn parser_types_expose_native_abi_method() {
    fn assert_native_abi_method<P: PackageFormat>() {}

    assert_native_abi_method::<RpmPackage>();
    assert_native_abi_method::<DebPackage>();
    assert_native_abi_method::<ArchPackage>();
}

#[test]
fn rpm_parser_preserves_native_scriptlet_and_trigger_slots() {
    let temp = TempDir::new().expect("tempdir");
    let path = write_rpm_fixture(temp.path());
    let package =
        RpmPackage::parse(path.to_str().expect("utf8 rpm path")).expect("parse rpm fixture");
    let slots = native_slots(&package);

    assert_contains_all(
        &slots,
        &[
            "%pre",
            "%post",
            "%preun",
            "%postun",
            "%pretrans",
            "%posttrans",
            "%preuntrans",
            "%postuntrans",
            "%verify",
            "%triggerprein",
            "%triggerin",
            "%triggerun",
            "%triggerpostun",
            "%filetriggerin",
            "%filetriggerun",
            "%filetriggerpostun",
            "%transfiletriggerin",
            "%transfiletriggerun",
            "%transfiletriggerpostun",
        ],
    );
    assert!(
        package
            .scriptlets()
            .iter()
            .any(|scriptlet| scriptlet.phase == ScriptletPhase::PreInstall)
    );
    assert!(
        !package
            .scriptlets()
            .iter()
            .any(|scriptlet| scriptlet.phase == ScriptletPhase::Trigger)
    );
    assert!(
        !package
            .scriptlets()
            .iter()
            .any(|scriptlet| scriptlet.content.contains("verify"))
    );

    let verify = package
        .native_scriptlet_abi()
        .iter()
        .find(|entry| entry.native_slot == "%verify")
        .expect("verify entry");
    assert_eq!(
        verify.support.reason_code(),
        Some("rpm-verify-scriptlet-deferred")
    );

    let trans_postun = package
        .native_scriptlet_abi()
        .iter()
        .find(|entry| entry.native_slot == "%transfiletriggerpostun")
        .expect("trans file trigger postun");
    assert_eq!(trans_postun.invocation.stdin, NativeStdinContract::None);
    assert_eq!(trans_postun.invocation.args.len(), 1);
    let NativeScriptletMetadata::Rpm(meta) = &trans_postun.metadata else {
        panic!("expected rpm metadata");
    };
    assert_eq!(
        meta.trigger.as_ref().expect("trigger metadata").file_globs,
        vec!["/usr/bin".to_string()]
    );
}

#[test]
fn deb_parser_preserves_maintainer_scripts_and_triggers_control_artifacts() {
    let temp = TempDir::new().expect("tempdir");
    let path = write_deb_fixture(temp.path());
    let package =
        DebPackage::parse(path.to_str().expect("utf8 deb path")).expect("parse deb fixture");
    let slots = native_slots(&package);

    assert_contains_all(
        &slots,
        &[
            "config", "preinst", "postinst", "prerm", "postrm", "triggers",
        ],
    );

    let preinst = package
        .native_scriptlet_abi()
        .iter()
        .find(|entry| entry.native_slot == "preinst")
        .expect("preinst entry");
    assert_eq!(preinst.interpreter.as_deref(), Some("/usr/bin/perl"));
    assert_eq!(preinst.interpreter_args, vec!["-w".to_string()]);

    let flattened_preinst = package
        .scriptlets()
        .iter()
        .find(|scriptlet| scriptlet.phase == ScriptletPhase::PreInstall)
        .expect("flattened preinst");
    assert_eq!(flattened_preinst.interpreter, "/usr/bin/perl -w");

    let triggers = package
        .native_scriptlet_abi()
        .iter()
        .find(|entry| entry.native_slot == "triggers")
        .expect("triggers entry");
    assert_eq!(triggers.kind, NativeScriptletKind::ControlArtifact);
    assert_eq!(
        triggers.support.reason_code(),
        Some("deb-trigger-semantics-deferred")
    );
    let NativeScriptletMetadata::Deb(meta) = &triggers.metadata else {
        panic!("expected deb metadata");
    };
    assert_eq!(meta.trigger_declarations.len(), 2);
    assert_eq!(
        meta.trigger_declarations[0].await_mode,
        DebTriggerAwaitMode::NoAwait
    );

    assert!(
        package
            .scriptlets()
            .iter()
            .any(|scriptlet| scriptlet.phase == ScriptletPhase::PostInstall)
    );
    assert!(
        !package
            .scriptlets()
            .iter()
            .any(|scriptlet| scriptlet.content.contains("/usr/share/debconf/confmodule"))
    );
}

#[test]
fn arch_parser_preserves_install_source_and_packaged_alpm_hook() {
    let temp = TempDir::new().expect("tempdir");
    let path = write_arch_fixture(temp.path());
    let package =
        ArchPackage::parse(path.to_str().expect("utf8 arch path")).expect("parse arch fixture");
    let slots = native_slots(&package);

    assert_contains_all(
        &slots,
        &[
            "pre_install",
            "post_install",
            "pre_upgrade",
            "post_upgrade",
            "pre_remove",
            "post_remove",
        ],
    );
    assert!(
        slots
            .iter()
            .any(|slot| slot.starts_with("alpm-hook:/usr/share/libalpm/hooks/"))
    );

    let post_install = package
        .native_scriptlet_abi()
        .iter()
        .find(|entry| entry.native_slot == "post_install")
        .expect("post_install entry");
    assert!(
        post_install
            .body
            .text
            .as_deref()
            .expect("utf8 install source")
            .contains("post_upgrade()")
    );
    let NativeScriptletMetadata::Arch(ArchNativeScriptletMetadata::Install(meta)) =
        &post_install.metadata
    else {
        panic!("expected arch install metadata");
    };
    assert_eq!(meta.function_body.as_deref(), Some("echo arch-post"));

    let post_upgrade = package
        .native_scriptlet_abi()
        .iter()
        .find(|entry| entry.native_slot == "post_upgrade")
        .expect("post_upgrade entry");
    assert_eq!(post_upgrade.invocation.args[0].index, 1);
    assert_eq!(post_upgrade.invocation.args[0].name, "old-version");
    assert_eq!(
        post_upgrade.invocation.args[0].value,
        NativeArgumentValue::OldVersion
    );
    assert_eq!(post_upgrade.invocation.args[1].index, 2);
    assert_eq!(post_upgrade.invocation.args[1].name, "new-version");
    assert_eq!(
        post_upgrade.invocation.args[1].value,
        NativeArgumentValue::NewVersion
    );

    let hook = package
        .native_scriptlet_abi()
        .iter()
        .find(|entry| entry.native_slot.contains("alpm-hook:"))
        .expect("alpm hook entry");
    assert_eq!(hook.kind, NativeScriptletKind::ControlArtifact);
    assert_eq!(hook.primary_lifecycle, NativeLifecyclePath::Trigger);
    assert_eq!(hook.invocation.stdin, NativeStdinContract::Paths);
    assert!(
        package
            .scriptlets()
            .iter()
            .any(|scriptlet| scriptlet.content == "echo arch-post")
    );
}

fn native_slots(package: &impl PackageFormat) -> BTreeSet<&str> {
    package
        .native_scriptlet_abi()
        .iter()
        .map(|entry| entry.native_slot.as_str())
        .collect()
}

fn assert_contains_all(actual: &BTreeSet<&str>, expected: &[&str]) {
    for slot in expected {
        assert!(actual.contains(slot), "missing native slot {slot}");
    }
}

fn write_rpm_fixture(dir: &Path) -> PathBuf {
    let mut builder = rpm::PackageBuilder::new(
        "native-abi-fixture",
        "1.0.0",
        "MIT",
        "x86_64",
        "native abi fixture",
    );
    builder
        .pre_install_script("echo pre")
        .post_install_script("echo post")
        .pre_uninstall_script("echo preun")
        .post_uninstall_script("echo postun")
        .pre_trans_script("echo pretrans")
        .post_trans_script("echo posttrans")
        .pre_untrans_script("echo preuntrans")
        .post_untrans_script("echo postuntrans")
        .verify_script("echo verify")
        .trigger_prein("bash", None, "echo triggerprein")
        .trigger_in(
            "bash",
            Some((rpm::DependencyFlags::GREATER, "5.0")),
            "echo triggerin",
        )
        .trigger_un("bash", None, "echo triggerun")
        .trigger_postun("bash", None, "echo triggerpostun")
        .file_trigger_in("/usr/lib", None, "echo filetriggerin")
        .file_trigger_un("/usr/lib", None, "echo filetriggerun")
        .file_trigger_postun("/usr/lib", None, "echo filetriggerpostun")
        .trans_file_trigger_in("/usr/bin", None, "echo transfiletriggerin")
        .trans_file_trigger_un("/usr/bin", None, "echo transfiletriggerun")
        .trans_file_trigger_postun("/usr/bin", None, "echo transfiletriggerpostun");
    let package = builder.build().expect("build rpm");
    let path = dir.join("native-abi-fixture.rpm");
    package.write_file(&path).expect("write rpm");
    path
}

fn write_deb_fixture(dir: &Path) -> PathBuf {
    let control = b"Package: native-abi-deb\nVersion: 1.0\nArchitecture: amd64\nDescription: native abi fixture\n";
    let config = b"#!/bin/sh\n. /usr/share/debconf/confmodule\n";
    let preinst = b"#!/usr/bin/perl -w\nprint \"preinst\\n\";\n";
    let postinst = b"#!/bin/sh\necho postinst\n";
    let prerm = b"#!/bin/sh\necho prerm\n";
    let postrm = b"#!/bin/sh\necho postrm\n";
    let triggers = b"interest-noawait update-icon-caches\nactivate ldconfig\n";
    let control_tar = tar_bytes(&[
        ("control", control.as_slice()),
        ("config", config.as_slice()),
        ("preinst", preinst.as_slice()),
        ("postinst", postinst.as_slice()),
        ("prerm", prerm.as_slice()),
        ("postrm", postrm.as_slice()),
        ("triggers", triggers.as_slice()),
    ]);
    let data_tar = tar_bytes(&[("usr/bin/native-abi", b"#!/bin/sh\n".as_slice())]);

    let path = dir.join("native-abi.deb");
    let file = File::create(&path).expect("create deb");
    let mut builder = ar::Builder::new(file);
    append_ar_member(&mut builder, "debian-binary", b"2.0\n");
    append_ar_member(&mut builder, "control.tar", &control_tar);
    append_ar_member(&mut builder, "data.tar", &data_tar);
    path
}

fn write_arch_fixture(dir: &Path) -> PathBuf {
    let pkginfo =
        b"pkgname = native-abi-arch\npkgver = 1.0-1\npkgdesc = native abi fixture\narch = x86_64\n";
    let install = br#"pre_install() {
    echo arch-pre
}

post_install() {
    echo arch-post
}

pre_upgrade() {
    echo arch-pre-upgrade
}

post_upgrade() {
    echo arch-post-upgrade
}

pre_remove() {
    echo arch-pre-remove
}

post_remove() {
    echo arch-post-remove
}
"#;
    let hook = b"[Trigger]\nOperation = Install\nType = Path\nTarget = usr/share/mime/*\n\n[Action]\nWhen = PostTransaction\nExec = /usr/bin/update-mime-database /usr/share/mime\nNeedsTargets\n";
    let raw_tar = tar_bytes(&[
        (".PKGINFO", pkginfo.as_slice()),
        (".INSTALL", install.as_slice()),
        (
            "usr/share/libalpm/hooks/30-native-abi.hook",
            hook.as_slice(),
        ),
    ]);
    let gz = gzip(raw_tar);
    let path = dir.join("native-abi.pkg.tar.gz");
    std::fs::write(&path, gz).expect("write arch package");
    path
}

fn tar_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, body) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, *path, Cursor::new(*body))
            .expect("append tar entry");
    }
    builder.into_inner().expect("finish tar")
}

fn gzip(bytes: Vec<u8>) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&bytes).expect("gzip write");
    encoder.finish().expect("gzip finish")
}

fn append_ar_member(builder: &mut ar::Builder<File>, name: &str, body: &[u8]) {
    let header = ar::Header::new(name.as_bytes().to_vec(), body.len() as u64);
    builder
        .append(&header, Cursor::new(body))
        .expect("append ar member");
}
