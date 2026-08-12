// apps/conary/src/commands/adopt/convert/tests.rs

use super::*;

#[test]
fn payload_paths_are_normalized_without_accepting_dot_segments() {
    assert_eq!(
        canonical_payload_path("usr/bin/tool").unwrap(),
        "/usr/bin/tool"
    );
    assert_eq!(
        canonical_payload_path("/etc/tool.conf").unwrap(),
        "/etc/tool.conf"
    );
    assert!(canonical_payload_path("usr/../bin/tool").is_err());
    assert!(canonical_payload_path("./usr/bin/tool").is_err());
    assert!(canonical_payload_path("").is_err());
}

#[test]
fn artifact_cache_identity_requires_an_exact_sha256() {
    let digest = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    assert_eq!(exact_sha256(digest).unwrap(), digest);
    assert_eq!(exact_sha256(&format!("sha256:{digest}")).unwrap(), digest);
    assert!(exact_sha256("sha256:abcd").is_err());
}

#[test]
fn hardlink_comparison_uses_topology_not_capture_specific_identity() {
    let left = PayloadNodeKind::Hardlink {
        target: "/usr/lib/tool".to_string(),
        identity: "artifact-group-1".to_string(),
    };
    let right = PayloadNodeKind::Hardlink {
        target: "/usr/lib/tool".to_string(),
        identity: "adopted-group-9".to_string(),
    };
    assert_eq!(comparable_kind(&left), comparable_kind(&right));
}
