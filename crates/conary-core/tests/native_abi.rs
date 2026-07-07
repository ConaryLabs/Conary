// conary-core/tests/native_abi.rs

use conary_core::ccs::convert::{ConversionOptions, FidelityLevel, LegacyConverter};
use conary_core::db::models::{Trove, TroveType};
use conary_core::packages::common::PackageMetadata;
use conary_core::packages::native_scriptlet_support::upstream_native_scriptlet_support_rows;
use conary_core::packages::traits::{
    ArchAlpmHookOperation, ArchAlpmHookTriggerType, ArchNativeScriptletMetadata, DebControlMember,
    DebTriggerAwaitMode, DebTriggerDirective, NativeArgumentValue, NativeLifecyclePath,
    NativeScriptletFormat, NativeScriptletKind, NativeScriptletMetadata, NativeScriptletSupport,
    NativeStdinContract, NativeTransactionPosition, PackageFile, PackageFormat, ScriptletPhase,
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

    assert_support_matrix_matches_parser(&package, NativeScriptletFormat::Rpm);
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

    assert_support_matrix_matches_parser(&package, NativeScriptletFormat::Deb);
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
    assert_eq!(meta.trigger_declarations.len(), 6);
    assert_eq!(
        meta.trigger_declarations
            .iter()
            .filter(|declaration| declaration.directive == DebTriggerDirective::Interest)
            .count(),
        3
    );
    assert_eq!(
        meta.trigger_declarations
            .iter()
            .filter(|declaration| declaration.directive == DebTriggerDirective::Activate)
            .count(),
        3
    );
    assert!(
        meta.trigger_declarations
            .iter()
            .any(|declaration| declaration.await_mode == DebTriggerAwaitMode::Await)
    );
    assert!(
        meta.trigger_declarations
            .iter()
            .any(|declaration| declaration.await_mode == DebTriggerAwaitMode::NoAwait)
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

    assert_support_matrix_matches_parser(&package, NativeScriptletFormat::Arch);
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
    assert_eq!(post_upgrade.invocation.args[0].name, "new-version");
    assert_eq!(
        post_upgrade.invocation.args[0].value,
        NativeArgumentValue::NewVersion
    );
    assert_eq!(post_upgrade.invocation.args[1].index, 2);
    assert_eq!(post_upgrade.invocation.args[1].name, "old-version");
    assert_eq!(
        post_upgrade.invocation.args[1].value,
        NativeArgumentValue::OldVersion
    );

    let hook = package
        .native_scriptlet_abi()
        .iter()
        .find(|entry| entry.native_slot.contains("alpm-hook:"))
        .expect("alpm hook entry");
    assert_eq!(hook.kind, NativeScriptletKind::ControlArtifact);
    assert_eq!(hook.primary_lifecycle, NativeLifecyclePath::Trigger);
    assert_eq!(hook.invocation.stdin, NativeStdinContract::Paths);
    let NativeScriptletMetadata::Arch(ArchNativeScriptletMetadata::AlpmHook(meta)) = &hook.metadata
    else {
        panic!("expected arch alpm hook metadata");
    };
    assert_eq!(meta.triggers.len(), 3);
    assert_eq!(
        meta.triggers[0].operations,
        vec![
            ArchAlpmHookOperation::Install,
            ArchAlpmHookOperation::Upgrade,
            ArchAlpmHookOperation::Remove,
        ]
    );
    assert_eq!(meta.triggers[0].trigger_type, ArchAlpmHookTriggerType::Path);
    assert_eq!(
        meta.triggers[1].trigger_type,
        ArchAlpmHookTriggerType::Package
    );
    assert_eq!(meta.triggers[2].trigger_type, ArchAlpmHookTriggerType::Path);
    let action = meta.action.as_ref().expect("alpm hook action");
    assert_eq!(action.when, NativeTransactionPosition::BeforeTransaction);
    assert_eq!(action.depends, vec!["shared-mime-info".to_string()]);
    assert!(action.abort_on_fail);
    assert!(action.needs_targets);
    assert!(
        package
            .scriptlets()
            .iter()
            .any(|scriptlet| scriptlet.content == "echo arch-post")
    );
}

#[test]
fn conversion_preserves_upstream_native_entries_in_ccs_scriptlet_bundle() {
    let temp = TempDir::new().expect("tempdir");

    let rpm_path = write_rpm_fixture(temp.path());
    let rpm = RpmPackage::parse(rpm_path.to_str().expect("utf8 rpm path")).expect("parse rpm");
    assert_conversion_preserves_native_entries(&rpm, &rpm_path, "rpm");

    let deb_path = write_deb_fixture(temp.path());
    let deb = DebPackage::parse(deb_path.to_str().expect("utf8 deb path")).expect("parse deb");
    assert_conversion_preserves_native_entries(&deb, &deb_path, "deb");

    let arch_path = write_arch_fixture(temp.path());
    let arch = ArchPackage::parse(arch_path.to_str().expect("utf8 arch path")).expect("parse arch");
    assert_conversion_preserves_native_entries(&arch, &arch_path, "arch");
}

fn native_slots(package: &impl PackageFormat) -> BTreeSet<&str> {
    package
        .native_scriptlet_abi()
        .iter()
        .map(|entry| entry.native_slot.as_str())
        .collect()
}

fn assert_support_matrix_matches_parser(
    package: &impl PackageFormat,
    format: NativeScriptletFormat,
) {
    let rows = upstream_native_scriptlet_support_rows()
        .iter()
        .filter(|row| row.format == format)
        .collect::<Vec<_>>();
    assert!(
        !rows.is_empty(),
        "missing support-matrix rows for {format:?}"
    );

    for row in &rows {
        let entry = package
            .native_scriptlet_abi()
            .iter()
            .find(|entry| row.matches_entry(entry))
            .unwrap_or_else(|| panic!("missing parser entry for {}", row.slot_label()));

        assert_eq!(
            entry.kind,
            row.kind,
            "native kind drifted for {}",
            row.slot_label()
        );
        assert_eq!(
            entry.primary_lifecycle,
            row.primary_lifecycle,
            "primary lifecycle drifted for {}",
            row.slot_label()
        );
        assert!(
            row.support.matches(&entry.support),
            "support expectation drifted for {}: expected {:?}, got {:?}",
            row.slot_label(),
            row.support,
            entry.support
        );
    }

    for entry in package.native_scriptlet_abi() {
        assert!(
            rows.iter().any(|row| row.matches_entry(entry)),
            "parsed native slot {} is missing from upstream support matrix",
            entry.native_slot
        );
    }
}

fn assert_contains_all(actual: &BTreeSet<&str>, expected: &[&str]) {
    for slot in expected {
        assert!(actual.contains(slot), "missing native slot {slot}");
    }
}

fn assert_conversion_preserves_native_entries(
    package: &impl PackageFormat,
    package_path: &Path,
    format: &str,
) {
    let output = TempDir::new().expect("converter output tempdir");
    let converter = LegacyConverter::new(ConversionOptions {
        capture_scriptlets: false,
        enable_inference: false,
        min_fidelity: FidelityLevel::Low,
        output_dir: output.path().to_path_buf(),
        ..ConversionOptions::default()
    });
    let metadata = metadata_from_package(package, package_path);
    let files = package
        .extract_file_contents()
        .expect("extract package file contents");

    let result = converter
        .convert(
            &metadata,
            &files,
            format,
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .expect("convert package");

    let bundle = result
        .build_result
        .manifest
        .legacy_scriptlets
        .as_ref()
        .expect("ccs legacy scriptlet bundle");
    for entry in package.native_scriptlet_abi() {
        let bundle_entry = bundle
            .entries
            .iter()
            .find(|bundle_entry| bundle_entry.id == entry.id)
            .unwrap_or_else(|| {
                panic!(
                    "CCS legacy scriptlet bundle dropped native ABI entry {}",
                    entry.id
                )
            });
        assert_eq!(bundle_entry.native_slot, entry.native_slot);
        assert_eq!(bundle_entry.body_sha256, entry.body.sha256);

        match &entry.support {
            NativeScriptletSupport::Parsed => {}
            NativeScriptletSupport::DeferredReview { reason_code } => {
                assert_eq!(bundle_entry.decision.as_str(), "review", "{}", entry.id);
                assert_eq!(&bundle_entry.reason_code, reason_code, "{}", entry.id);
            }
            NativeScriptletSupport::Unpreservable { reason_code } => {
                assert_eq!(bundle_entry.decision.as_str(), "blocked", "{}", entry.id);
                assert_eq!(&bundle_entry.reason_code, reason_code, "{}", entry.id);
            }
        }

        match &entry.metadata {
            NativeScriptletMetadata::Rpm(metadata) => {
                if let Some(trigger) = &metadata.trigger {
                    let projected = bundle_entry.rpm_trigger.as_ref().unwrap_or_else(|| {
                        panic!("missing RPM trigger projection for {}", entry.id)
                    });
                    assert_eq!(projected.file_globs, trigger.file_globs);
                    assert!(projected.transaction_order.is_some());
                }
            }
            NativeScriptletMetadata::Deb(metadata) => {
                if metadata.control_member == DebControlMember::Triggers {
                    let projected = bundle_entry.deb_maintainer.as_ref().unwrap_or_else(|| {
                        panic!("missing DEB trigger projection for {}", entry.id)
                    });
                    assert_eq!(projected.triggers_content, entry.body.text);
                    assert_eq!(
                        projected.trigger_names.len(),
                        metadata.trigger_declarations.len()
                    );
                    assert!(bundle_entry.extra.contains_key("deb_trigger_declarations"));
                }
            }
            NativeScriptletMetadata::Arch(ArchNativeScriptletMetadata::Install(metadata)) => {
                let projected = bundle_entry
                    .arch_install
                    .as_ref()
                    .unwrap_or_else(|| panic!("missing Arch install projection for {}", entry.id));
                assert_eq!(
                    projected.called_function.as_deref(),
                    Some(metadata.function_name.as_str())
                );
                assert_eq!(
                    projected.install_digest.as_deref(),
                    Some(metadata.install_source_sha256.as_str())
                );
            }
            NativeScriptletMetadata::Arch(ArchNativeScriptletMetadata::AlpmHook(_)) => {
                assert!(bundle_entry.extra.contains_key("arch_alpm_hook"));
            }
        }
    }
}

fn metadata_from_package(package: &impl PackageFormat, package_path: &Path) -> PackageMetadata {
    PackageMetadata {
        package_path: package_path.to_path_buf(),
        name: package.name().to_string(),
        version: package.version().to_string(),
        architecture: package.architecture().map(str::to_string),
        description: package.description().map(str::to_string),
        files: package.files().to_vec(),
        dependencies: package.dependencies().to_vec(),
        provides: package.provides().to_vec(),
        scriptlets: package.scriptlets().to_vec(),
        native_scriptlet_abi: package.native_scriptlet_abi().to_vec(),
        config_files: package.config_files().to_vec(),
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
    let triggers = b"interest cache-default\ninterest-await cache-await\ninterest-noawait update-icon-caches\nactivate ldconfig\nactivate-await ldconfig-await\nactivate-noawait ldconfig-noawait\n";
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
    let hook = b"[Trigger]\nOperation = Install\nOperation = Upgrade\nOperation = Remove\nType = Path\nTarget = usr/share/mime/*\n\n[Trigger]\nOperation = Install\nType = Package\nTarget = shared-mime-info\n\n[Trigger]\nOperation = Remove\nType = File\nTarget = usr/share/icons/*\n\n[Action]\nDescription = update mime cache\nWhen = PreTransaction\nExec = /usr/bin/update-mime-database /usr/share/mime\nDepends = shared-mime-info\nAbortOnFail\nNeedsTargets\n";
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
