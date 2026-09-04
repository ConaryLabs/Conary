// crates/conary-core/src/generation/verity_policy/tests.rs

use super::*;

#[test]
fn presence_and_last_exact_argument_control_policy() {
    for (cmdline, expected) in [
        ("quiet", VerityPolicy::Verified),
        ("conary.verity=on", VerityPolicy::Verified),
        ("conary.verity=off", VerityPolicy::ExplicitlyOff),
        ("conary.verity=off conary.verity=on", VerityPolicy::Verified),
        (
            "conary.verity=on\tconary.verity=off",
            VerityPolicy::ExplicitlyOff,
        ),
        ("other.conary.verity=off", VerityPolicy::Verified),
        (
            "conary.verity=",
            VerityPolicy::Invalid {
                value: String::new(),
            },
        ),
        (
            "conary.verity=invalid",
            VerityPolicy::Invalid {
                value: "invalid".into(),
            },
        ),
        (
            "conary.verity=off conary.verity=",
            VerityPolicy::Invalid {
                value: String::new(),
            },
        ),
    ] {
        assert_eq!(
            VerityPolicy::from_kernel_cmdline(cmdline),
            expected,
            "{cmdline}"
        );
    }
}

#[test]
fn binary_free_initramfs_adapter_conforms_to_rust_policy() {
    let tmp = tempfile::tempdir().unwrap();
    let cmdline_path = tmp.path().join("cmdline");
    let adapter = include_str!("../../../../../packaging/dracut/90conary/conary-verity.sh");
    let script = format!(
        "{adapter}\nvalue=\"$(conary_read_verity \"$1\")\"\n\
         conary_composefs_options \"$value\" /conary/objects\n"
    );
    let arguments = [
        "quiet",
        "conary.verity=on",
        "conary.verity=off",
        "conary.verity=",
        "conary.verity=invalid",
        "conary.verity=OFF",
        "other.conary.verity=off",
    ];
    for first in arguments {
        for last in arguments {
            let cmdline = format!("{first}\t{last}\n");
            let policy = VerityPolicy::from_kernel_cmdline(&cmdline);
            std::fs::write(&cmdline_path, &cmdline).unwrap();
            let output = std::process::Command::new("/bin/sh")
                .args(["-c", &script, "verity-conformance"])
                .arg(&cmdline_path)
                .output()
                .unwrap();
            match policy.requires_verification() {
                Ok(required) => {
                    assert!(output.status.success(), "{cmdline}");
                    let suffix = if required { ",verity_check=1" } else { "" };
                    assert_eq!(
                        String::from_utf8(output.stdout).unwrap(),
                        format!("basedir=/conary/objects{suffix}\n"),
                        "{cmdline}"
                    );
                    assert_eq!(
                        String::from_utf8(output.stderr).unwrap(),
                        policy
                            .warning()
                            .map(|warning| format!("{warning}\n"))
                            .unwrap_or_default(),
                        "{cmdline}"
                    );
                }
                Err(error) => {
                    assert!(!output.status.success(), "{cmdline}");
                    assert!(output.stdout.is_empty(), "{cmdline}");
                    assert_eq!(
                        String::from_utf8(output.stderr).unwrap(),
                        format!("conary: {error}\n"),
                        "{cmdline}"
                    );
                }
            }
        }
    }
}
