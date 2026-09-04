// crates/conary-core/src/flavor/tests.rs

use super::*;

// === FlavorOp tests ===

#[test]
fn test_flavor_op_parse_required() {
    let (op, name) = FlavorOp::parse_with_name("ssl").unwrap();
    assert_eq!(op, FlavorOp::Required);
    assert_eq!(name, "ssl");
}

#[test]
fn test_flavor_op_parse_not() {
    let (op, name) = FlavorOp::parse_with_name("!debug").unwrap();
    assert_eq!(op, FlavorOp::Not);
    assert_eq!(name, "debug");
}

#[test]
fn test_flavor_op_parse_prefers() {
    let (op, name) = FlavorOp::parse_with_name("~vmware").unwrap();
    assert_eq!(op, FlavorOp::Prefers);
    assert_eq!(name, "vmware");
}

#[test]
fn test_flavor_op_parse_prefers_not() {
    let (op, name) = FlavorOp::parse_with_name("~!xen").unwrap();
    assert_eq!(op, FlavorOp::PrefersNot);
    assert_eq!(name, "xen");
}

#[test]
fn test_flavor_op_parse_with_spaces() {
    let (op, name) = FlavorOp::parse_with_name("  ~! xen  ").unwrap();
    assert_eq!(op, FlavorOp::PrefersNot);
    assert_eq!(name, "xen");
}

#[test]
fn test_flavor_op_parse_empty_error() {
    assert!(FlavorOp::parse_with_name("").is_err());
    assert!(FlavorOp::parse_with_name("   ").is_err());
}

#[test]
fn test_flavor_op_parse_missing_name_error() {
    assert!(FlavorOp::parse_with_name("!").is_err());
    assert!(FlavorOp::parse_with_name("~").is_err());
    assert!(FlavorOp::parse_with_name("~!").is_err());
}

// === FlavorItem tests ===

#[test]
fn test_flavor_item_display() {
    assert_eq!(
        FlavorItem::new(FlavorOp::Required, "ssl").to_string(),
        "ssl"
    );
    assert_eq!(
        FlavorItem::new(FlavorOp::Not, "debug").to_string(),
        "!debug"
    );
    assert_eq!(
        FlavorItem::new(FlavorOp::Prefers, "vmware").to_string(),
        "~vmware"
    );
    assert_eq!(
        FlavorItem::new(FlavorOp::PrefersNot, "xen").to_string(),
        "~!xen"
    );
}

// === FlavorSpec parsing tests ===

#[test]
fn test_flavor_spec_parse_empty_brackets() {
    let spec = FlavorSpec::parse("[]").unwrap();
    assert!(spec.items.is_empty());
    assert!(spec.arch.is_none());
    assert!(spec.is_empty());
}

#[test]
fn test_flavor_spec_parse_empty_string() {
    let spec = FlavorSpec::parse("").unwrap();
    assert!(spec.is_empty());
}

#[test]
fn test_flavor_spec_parse_single_item() {
    let spec = FlavorSpec::parse("[ssl]").unwrap();
    assert_eq!(spec.items.len(), 1);
    assert_eq!(spec.items[0].op, FlavorOp::Required);
    assert_eq!(spec.items[0].name, "ssl");
    assert!(spec.arch.is_none());
}

#[test]
fn test_flavor_spec_parse_arch_only() {
    let spec = FlavorSpec::parse("[is: x86_64]").unwrap();
    assert!(spec.items.is_empty());
    assert_eq!(
        spec.arch.as_ref().unwrap().architectures,
        vec!["x86_64".to_string()]
    );
}

#[test]
fn test_flavor_spec_parse_multi_arch() {
    let spec = FlavorSpec::parse("[is: x86 x86_64]").unwrap();
    assert!(spec.items.is_empty());
    // Canonicalized (sorted)
    assert_eq!(
        spec.arch.as_ref().unwrap().architectures,
        vec!["x86".to_string(), "x86_64".to_string()]
    );
}

#[test]
fn test_flavor_spec_parse_mixed() {
    let spec = FlavorSpec::parse("[ssl, !debug, is: x86_64]").unwrap();
    assert_eq!(spec.items.len(), 2);
    // Canonicalized (sorted by name): debug comes before ssl
    assert_eq!(spec.items[0].op, FlavorOp::Not);
    assert_eq!(spec.items[0].name, "debug");
    assert_eq!(spec.items[1].op, FlavorOp::Required);
    assert_eq!(spec.items[1].name, "ssl");
    assert_eq!(
        spec.arch.as_ref().unwrap().architectures,
        vec!["x86_64".to_string()]
    );
}

#[test]
fn test_flavor_spec_parse_all_operators() {
    let spec = FlavorSpec::parse("[ssl, !debug, ~vmware, ~!xen]").unwrap();
    assert_eq!(spec.items.len(), 4);
    // Sorted: debug, ssl, vmware, xen
    assert_eq!(spec.items[0].name, "debug");
    assert_eq!(spec.items[0].op, FlavorOp::Not);
    assert_eq!(spec.items[1].name, "ssl");
    assert_eq!(spec.items[1].op, FlavorOp::Required);
    assert_eq!(spec.items[2].name, "vmware");
    assert_eq!(spec.items[2].op, FlavorOp::Prefers);
    assert_eq!(spec.items[3].name, "xen");
    assert_eq!(spec.items[3].op, FlavorOp::PrefersNot);
}

#[test]
fn test_flavor_spec_parse_without_brackets() {
    let spec = FlavorSpec::parse("ssl, !debug").unwrap();
    assert_eq!(spec.items.len(), 2);
}

#[test]
fn test_flavor_spec_parse_original_conary_example() {
    // From conaryopedia: [!dom0, ~!domU, ~vmware, ~!xen is: x86 x86_64]
    let spec = FlavorSpec::parse("[!dom0, ~!domU, ~vmware, ~!xen, is: x86 x86_64]").unwrap();
    assert_eq!(spec.items.len(), 4);
    assert!(spec.arch.is_some());
    assert_eq!(
        spec.arch.as_ref().unwrap().architectures,
        vec!["x86".to_string(), "x86_64".to_string()]
    );
}

// === Canonicalization tests ===

#[test]
fn test_flavor_spec_canonicalization_order() {
    let spec1 = FlavorSpec::parse("[ssl, debug]").unwrap();
    let spec2 = FlavorSpec::parse("[debug, ssl]").unwrap();
    assert_eq!(spec1.to_string(), spec2.to_string());
    assert_eq!(spec1.to_string(), "[debug, ssl]");
}

#[test]
fn test_flavor_spec_canonicalization_arch_order() {
    let spec = FlavorSpec::parse("[is: x86_64 x86]").unwrap();
    // Sorted: x86 before x86_64
    assert_eq!(
        spec.arch.as_ref().unwrap().architectures,
        vec!["x86".to_string(), "x86_64".to_string()]
    );
    assert_eq!(spec.to_string(), "[is: x86 x86_64]");
}

#[test]
fn test_flavor_spec_canonicalization_dedup_arch() {
    let spec = FlavorSpec::parse("[is: x86_64 x86 x86_64]").unwrap();
    // Deduped
    assert_eq!(spec.arch.as_ref().unwrap().architectures.len(), 2);
}

// === Display/round-trip tests ===

#[test]
fn test_flavor_spec_display_empty() {
    let spec = FlavorSpec::empty();
    assert_eq!(spec.to_string(), "");
}

#[test]
fn test_flavor_spec_display_roundtrip() {
    let original = "[!debug, ssl, ~vmware, is: x86 x86_64]";
    let spec = FlavorSpec::parse(original).unwrap();
    let displayed = spec.to_string();
    let reparsed = FlavorSpec::parse(&displayed).unwrap();
    assert_eq!(spec, reparsed);
}

// === Matching tests ===

#[test]
fn test_matching_required_present() {
    let spec = FlavorSpec::parse("[ssl]").unwrap();
    let system = SystemFlavor::new("x86_64").with_feature("ssl");
    let (matches, score) = spec.matches(&system);
    assert!(matches);
    assert!(score > 0);
}

#[test]
fn test_matching_required_absent() {
    let spec = FlavorSpec::parse("[ssl]").unwrap();
    let system = SystemFlavor::new("x86_64");
    let (matches, _) = spec.matches(&system);
    assert!(!matches);
}

#[test]
fn test_matching_not_present() {
    let spec = FlavorSpec::parse("[!debug]").unwrap();
    let system = SystemFlavor::new("x86_64").with_feature("debug");
    let (matches, _) = spec.matches(&system);
    assert!(!matches);
}

#[test]
fn test_matching_not_absent() {
    let spec = FlavorSpec::parse("[!debug]").unwrap();
    let system = SystemFlavor::new("x86_64");
    let (matches, score) = spec.matches(&system);
    assert!(matches);
    assert!(score > 0);
}

#[test]
fn test_matching_prefers_scoring() {
    let spec = FlavorSpec::parse("[~vmware]").unwrap();
    let system_with = SystemFlavor::new("x86_64").with_feature("vmware");
    let system_without = SystemFlavor::new("x86_64");

    let (matches_with, score_with) = spec.matches(&system_with);
    let (matches_without, score_without) = spec.matches(&system_without);

    // Both match, but with feature should score higher
    assert!(matches_with);
    assert!(matches_without);
    assert!(score_with > score_without);
}

#[test]
fn test_matching_prefers_not_scoring() {
    let spec = FlavorSpec::parse("[~!xen]").unwrap();
    let system_with = SystemFlavor::new("x86_64").with_feature("xen");
    let system_without = SystemFlavor::new("x86_64");

    let (matches_with, score_with) = spec.matches(&system_with);
    let (matches_without, score_without) = spec.matches(&system_without);

    // Both match, but without feature should score higher
    assert!(matches_with);
    assert!(matches_without);
    assert!(score_without > score_with);
}

#[test]
fn test_matching_architecture() {
    let spec = FlavorSpec::parse("[is: x86_64]").unwrap();
    let system_match = SystemFlavor::new("x86_64");
    let system_no_match = SystemFlavor::new("aarch64");

    assert!(spec.matches(&system_match).0);
    assert!(!spec.matches(&system_no_match).0);
}

#[test]
fn test_matching_multi_architecture() {
    let spec = FlavorSpec::parse("[is: x86 x86_64]").unwrap();
    let system_x86 = SystemFlavor::new("x86");
    let system_x86_64 = SystemFlavor::new("x86_64");
    let system_arm = SystemFlavor::new("aarch64");

    assert!(spec.matches(&system_x86).0);
    assert!(spec.matches(&system_x86_64).0);
    assert!(!spec.matches(&system_arm).0);
}

#[test]
fn test_matching_empty_spec() {
    let spec = FlavorSpec::empty();
    let system = SystemFlavor::new("x86_64").with_feature("ssl");
    let (matches, score) = spec.matches(&system);
    assert!(matches);
    assert_eq!(score, 0); // No preferences to score
}

// === Select best tests ===

#[test]
fn test_select_best() {
    let candidates = vec![
        (FlavorSpec::parse("[ssl]").unwrap(), "pkg-ssl"),
        (FlavorSpec::parse("[!ssl]").unwrap(), "pkg-no-ssl"),
        (FlavorSpec::parse("[~ssl]").unwrap(), "pkg-prefers-ssl"),
    ];

    let system_with_ssl = SystemFlavor::new("x86_64").with_feature("ssl");
    let system_without_ssl = SystemFlavor::new("x86_64");

    // System with SSL should get pkg-ssl (required match beats preference)
    let best_with = FlavorSpec::select_best(&candidates, &system_with_ssl);
    assert_eq!(best_with, Some(&"pkg-ssl"));

    // System without SSL should get pkg-no-ssl
    let best_without = FlavorSpec::select_best(&candidates, &system_without_ssl);
    assert_eq!(best_without, Some(&"pkg-no-ssl"));
}

#[test]
fn test_select_best_no_match() {
    let candidates = vec![(FlavorSpec::parse("[ssl]").unwrap(), "pkg-ssl")];

    let system = SystemFlavor::new("x86_64"); // No ssl feature
    let best = FlavorSpec::select_best(&candidates, &system);
    assert!(best.is_none());
}
