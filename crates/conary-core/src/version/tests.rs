// crates/conary-core/src/version/tests.rs

use super::*;

#[test]
fn test_rpm_version_parse_simple() {
    let v = RpmVersion::parse("1.2.3").unwrap();
    assert_eq!(v.epoch, 0);
    assert_eq!(v.version, "1.2.3");
    assert_eq!(v.release, None);
}

#[test]
fn test_rpm_version_parse_with_epoch() {
    let v = RpmVersion::parse("2:1.2.3").unwrap();
    assert_eq!(v.epoch, 2);
    assert_eq!(v.version, "1.2.3");
    assert_eq!(v.release, None);
}

#[test]
fn test_rpm_version_parse_with_release() {
    let v = RpmVersion::parse("1.2.3-4.el8").unwrap();
    assert_eq!(v.epoch, 0);
    assert_eq!(v.version, "1.2.3");
    assert_eq!(v.release, Some("4.el8".to_string()));
}

#[test]
fn test_rpm_version_parse_full() {
    let v = RpmVersion::parse("1:2.3.4-5.el8").unwrap();
    assert_eq!(v.epoch, 1);
    assert_eq!(v.version, "2.3.4");
    assert_eq!(v.release, Some("5.el8".to_string()));
}

#[test]
fn test_rpm_version_compare_epochs() {
    let v1 = RpmVersion::parse("1:1.0.0").unwrap();
    let v2 = RpmVersion::parse("0:2.0.0").unwrap();
    assert!(v1 > v2); // Higher epoch wins even with lower version
}

#[test]
fn test_rpm_version_compare_versions() {
    let v1 = RpmVersion::parse("1.2.3").unwrap();
    let v2 = RpmVersion::parse("1.2.4").unwrap();
    assert!(v1 < v2);
}

#[test]
fn test_rpm_version_compare_releases() {
    let v1 = RpmVersion::parse("1.2.3-1").unwrap();
    let v2 = RpmVersion::parse("1.2.3-2").unwrap();
    assert!(v1 < v2);
}

#[test]
fn test_version_constraint_parse_exact() {
    let c = VersionConstraint::parse("1.2.3").unwrap();
    let v = RpmVersion::parse("1.2.3").unwrap();
    assert!(c.satisfies(&v));
}

#[test]
fn test_version_constraint_parse_greater_or_equal() {
    let c = VersionConstraint::parse(">= 1.2.0").unwrap();
    let v1 = RpmVersion::parse("1.2.0").unwrap();
    let v2 = RpmVersion::parse("1.3.0").unwrap();
    let v3 = RpmVersion::parse("1.1.0").unwrap();

    assert!(c.satisfies(&v1));
    assert!(c.satisfies(&v2));
    assert!(!c.satisfies(&v3));
}

#[test]
fn test_version_constraint_parse_less_than() {
    let c = VersionConstraint::parse("< 2.0.0").unwrap();
    let v1 = RpmVersion::parse("1.9.9").unwrap();
    let v2 = RpmVersion::parse("2.0.0").unwrap();

    assert!(c.satisfies(&v1));
    assert!(!c.satisfies(&v2));
}

#[test]
fn test_version_constraint_and() {
    let c = VersionConstraint::parse(">= 1.0.0, < 2.0.0").unwrap();
    let v1 = RpmVersion::parse("1.5.0").unwrap();
    let v2 = RpmVersion::parse("2.0.0").unwrap();
    let v3 = RpmVersion::parse("0.9.0").unwrap();

    assert!(c.satisfies(&v1));
    assert!(!c.satisfies(&v2));
    assert!(!c.satisfies(&v3));
}

#[test]
fn test_version_constraint_any() {
    let c = VersionConstraint::parse("*").unwrap();
    let v = RpmVersion::parse("99.99.99").unwrap();
    assert!(c.satisfies(&v));
}

#[test]
fn test_rpm_version_rejects_empty_epoch() {
    assert!(RpmVersion::parse(":1.02.208-2.fc44").is_err());
}

#[test]
fn test_rpm_version_display() {
    let v1 = RpmVersion::parse("1.2.3").unwrap();
    assert_eq!(v1.to_string(), "1.2.3");

    let v2 = RpmVersion::parse("2:1.2.3-4.el8").unwrap();
    assert_eq!(v2.to_string(), "2:1.2.3-4.el8");
}

#[test]
fn test_version_constraint_display() {
    let c1 = VersionConstraint::parse(">= 1.2.0").unwrap();
    assert_eq!(c1.to_string(), ">= 1.2.0");

    let c2 = VersionConstraint::parse(">= 1.0.0, < 2.0.0").unwrap();
    assert_eq!(c2.to_string(), ">= 1.0.0, < 2.0.0");
}

#[test]
fn test_exact_match_normalizes_epoch_and_release() {
    // "1.2.3" (no epoch/release) should match "0:1.2.3" (explicit epoch 0)
    let c = VersionConstraint::parse("= 1.2.3").unwrap();
    let v = RpmVersion::parse("0:1.2.3").unwrap();
    assert!(c.satisfies(&v));

    // "0:1.2.3" constraint should match "1.2.3" (no epoch)
    let c = VersionConstraint::parse("= 0:1.2.3").unwrap();
    let v = RpmVersion::parse("1.2.3").unwrap();
    assert!(c.satisfies(&v));

    // Release None vs Some should not match
    let c = VersionConstraint::parse("= 1.2.3").unwrap();
    let v = RpmVersion::parse("1.2.3-1.fc44").unwrap();
    assert!(!c.satisfies(&v));
}

#[test]
fn test_rpmvercmp_digits_beat_alpha() {
    // Digit segments always sort after alpha segments in RPM
    let v1 = RpmVersion::parse("1.0a").unwrap();
    let v2 = RpmVersion::parse("1.01").unwrap();
    assert_eq!(v1.compare(&v2), Ordering::Less);
}

#[test]
fn test_rpmvercmp_leading_zeros() {
    let v1 = RpmVersion::parse("1.001").unwrap();
    let v2 = RpmVersion::parse("1.1").unwrap();
    assert_eq!(v1.compare(&v2), Ordering::Equal);
}

#[test]
fn test_rpmvercmp_mixed_alpha_numeric() {
    let v1 = RpmVersion::parse("2.0.1a").unwrap();
    let v2 = RpmVersion::parse("2.0.1b").unwrap();
    assert!(v1 < v2);
}

// --- is_compatible_with tests ---

#[test]
fn test_compatible_any_with_anything() {
    let any = VersionConstraint::Any;
    let exact = VersionConstraint::parse("= 1.0").unwrap();
    let range = VersionConstraint::parse("> 1.0").unwrap();
    assert!(any.is_compatible_with(&exact));
    assert!(any.is_compatible_with(&range));
    assert!(any.is_compatible_with(&VersionConstraint::Any));
}

#[test]
fn test_compatible_exact_vs_exact_same() {
    let c1 = VersionConstraint::parse("= 1.0").unwrap();
    let c2 = VersionConstraint::parse("= 1.0").unwrap();
    assert!(c1.is_compatible_with(&c2));
}

#[test]
fn test_compatible_exact_vs_exact_different() {
    let c1 = VersionConstraint::parse("= 1.0").unwrap();
    let c2 = VersionConstraint::parse("= 2.0").unwrap();
    assert!(!c1.is_compatible_with(&c2));
}

#[test]
fn test_compatible_exact_vs_range_satisfies() {
    // 1.5 satisfies >= 1.0 and < 2.0
    let exact = VersionConstraint::parse("= 1.5").unwrap();
    let ge = VersionConstraint::parse(">= 1.0").unwrap();
    let lt = VersionConstraint::parse("< 2.0").unwrap();
    assert!(exact.is_compatible_with(&ge));
    assert!(exact.is_compatible_with(&lt));
    assert!(ge.is_compatible_with(&exact));
}

#[test]
fn test_compatible_exact_vs_range_does_not_satisfy() {
    // 0.5 does not satisfy >= 1.0
    let exact = VersionConstraint::parse("= 0.5").unwrap();
    let ge = VersionConstraint::parse(">= 1.0").unwrap();
    assert!(!exact.is_compatible_with(&ge));
    assert!(!ge.is_compatible_with(&exact));
}

#[test]
fn test_compatible_same_direction_ranges() {
    // Both GT — always overlap
    let c1 = VersionConstraint::parse("> 1.0").unwrap();
    let c2 = VersionConstraint::parse("> 3.0").unwrap();
    assert!(c1.is_compatible_with(&c2));

    // Both LT — always overlap
    let c3 = VersionConstraint::parse("< 2.0").unwrap();
    let c4 = VersionConstraint::parse("< 5.0").unwrap();
    assert!(c3.is_compatible_with(&c4));

    // GE + GE
    let c5 = VersionConstraint::parse(">= 1.0").unwrap();
    let c6 = VersionConstraint::parse(">= 2.0").unwrap();
    assert!(c5.is_compatible_with(&c6));
}

#[test]
fn test_compatible_opposite_ranges_overlapping() {
    // > 1.0 and < 3.0 — overlap in (1.0, 3.0)
    let c1 = VersionConstraint::parse("> 1.0").unwrap();
    let c2 = VersionConstraint::parse("< 3.0").unwrap();
    assert!(c1.is_compatible_with(&c2));

    // >= 1.0 and <= 2.0 — overlap at [1.0, 2.0]
    let c3 = VersionConstraint::parse(">= 1.0").unwrap();
    let c4 = VersionConstraint::parse("<= 2.0").unwrap();
    assert!(c3.is_compatible_with(&c4));

    // >= 2.0 and <= 2.0 — single-point overlap at 2.0
    let c5 = VersionConstraint::parse(">= 2.0").unwrap();
    let c6 = VersionConstraint::parse("<= 2.0").unwrap();
    assert!(c5.is_compatible_with(&c6));
}

#[test]
fn test_compatible_opposite_ranges_non_overlapping() {
    // > 3.0 and < 1.0 — no overlap
    let c1 = VersionConstraint::parse("> 3.0").unwrap();
    let c2 = VersionConstraint::parse("< 1.0").unwrap();
    assert!(!c1.is_compatible_with(&c2));

    // >= 3.0 and <= 2.0 — no overlap
    let c3 = VersionConstraint::parse(">= 3.0").unwrap();
    let c4 = VersionConstraint::parse("<= 2.0").unwrap();
    assert!(!c3.is_compatible_with(&c4));

    // > 2.0 and < 2.0 — touching but not overlapping (strict)
    let c5 = VersionConstraint::parse("> 2.0").unwrap();
    let c6 = VersionConstraint::parse("< 2.0").unwrap();
    assert!(!c5.is_compatible_with(&c6));
}

#[test]
fn test_compatible_not_equal() {
    let ne = VersionConstraint::parse("!= 1.0").unwrap();
    let exact_same = VersionConstraint::parse("= 1.0").unwrap();
    let exact_diff = VersionConstraint::parse("= 2.0").unwrap();
    let range = VersionConstraint::parse("> 1.0").unwrap();

    // NotEqual vs different Exact — compatible (2.0 satisfies != 1.0)
    assert!(ne.is_compatible_with(&exact_diff));
    // NotEqual vs range — compatible
    assert!(ne.is_compatible_with(&range));
    // NotEqual vs Exact(same): the Exact arm fires first (range.satisfies(v)),
    // and != 1.0 does NOT satisfy version 1.0, so false.
    assert!(!ne.is_compatible_with(&exact_same));
}

#[test]
fn test_compatible_and_constraint() {
    // And(>= 1.0, < 2.0) is compatible with > 1.5
    let and_c = VersionConstraint::parse(">= 1.0, < 2.0").unwrap();
    let gt = VersionConstraint::parse("> 1.5").unwrap();
    assert!(and_c.is_compatible_with(&gt));

    // And(>= 1.0, < 2.0) is NOT compatible with > 3.0
    let gt_high = VersionConstraint::parse("> 3.0").unwrap();
    assert!(!and_c.is_compatible_with(&gt_high));
}

#[test]
fn test_rpm_version_four_component() {
    let v1 = RpmVersion::parse("1.2.3.4").unwrap();
    let v2 = RpmVersion::parse("1.2.3.5").unwrap();
    assert_eq!(v1.compare(&v2), Ordering::Less);

    let v3 = RpmVersion::parse("1.2.3").unwrap();
    let v4 = RpmVersion::parse("1.2.3.4").unwrap();
    assert_eq!(v3.compare(&v4), Ordering::Less);
}

#[test]
fn test_exact_constraint_uses_rpmvercmp_semantics() {
    // "1.001-1" should match "1.1-1" under Exact because rpmvercmp
    // treats leading zeros as insignificant in numeric segments.
    let c = VersionConstraint::parse("= 1.001-1").unwrap();
    let v = RpmVersion::parse("1.1-1").unwrap();
    assert!(
        c.satisfies(&v),
        "Exact constraint should use rpmvercmp: 1.001-1 == 1.1-1"
    );

    // And the reverse direction
    let c2 = VersionConstraint::parse("= 1.1-1").unwrap();
    let v2 = RpmVersion::parse("1.001-1").unwrap();
    assert!(
        c2.satisfies(&v2),
        "Exact constraint should use rpmvercmp: 1.1-1 == 1.001-1"
    );
}
