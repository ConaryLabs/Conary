// conary-test/src/engine/assertions.rs

use crate::config::manifest::Assertion;
use anyhow::{bail, Result};

pub fn evaluate_assertion(
    assertion: &Assertion,
    exit_code: i32,
    stdout: &str,
    stderr: &str,
) -> Result<()> {
    if let Some(expected) = assertion.exit_code
        && exit_code != expected
    {
        bail!("expected exit code {expected}, got {exit_code}");
    }
    if let Some(not_expected) = assertion.exit_code_not
        && exit_code == not_expected
    {
        bail!("expected exit code other than {not_expected}, got {exit_code}");
    }
    if let Some(ref needle) = assertion.stdout_contains
        && !stdout.contains(needle.as_str())
    {
        bail!("stdout does not contain \"{needle}\"");
    }
    if let Some(ref needle) = assertion.stdout_not_contains
        && stdout.contains(needle.as_str())
    {
        bail!("stdout unexpectedly contains \"{needle}\"");
    }
    if let Some(ref needles) = assertion.stdout_contains_all {
        for needle in needles {
            if !stdout.contains(needle.as_str()) {
                bail!("stdout does not contain \"{needle}\" (stdout_contains_all)");
            }
        }
    }
    if let Some(ref needles) = assertion.stdout_contains_any
        && !needles.iter().any(|n| stdout.contains(n.as_str()))
    {
        bail!(
            "stdout does not contain any of {:?} (stdout_contains_any)",
            needles
        );
    }
    // Conditional assertions: only checked when exit code is 0.
    if exit_code == 0 {
        if let Some(ref needle) = assertion.stdout_contains_if_success
            && !stdout.contains(needle.as_str())
        {
            bail!("stdout does not contain \"{needle}\" (stdout_contains_if_success)");
        }
        if let Some(ref needles) = assertion.stdout_contains_any_if_success
            && !needles.iter().any(|n| stdout.contains(n.as_str()))
        {
            bail!(
                "stdout does not contain any of {:?} (stdout_contains_any_if_success)",
                needles
            );
        }
    }
    if let Some(ref needle) = assertion.stderr_contains
        && !stderr.contains(needle.as_str())
    {
        bail!("stderr does not contain \"{needle}\"");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_assertion() -> Assertion {
        Assertion {
            exit_code: None,
            exit_code_not: None,
            stdout_contains: None,
            stdout_not_contains: None,
            stdout_contains_all: None,
            stdout_contains_any: None,
            stdout_contains_if_success: None,
            stdout_contains_any_if_success: None,
            stderr_contains: None,
            file_exists: None,
            file_not_exists: None,
            file_checksum: None,
        }
    }

    #[test]
    fn test_stdout_contains_all_pass() {
        let mut a = base_assertion();
        a.stdout_contains_all = Some(vec!["foo".into(), "bar".into()]);
        assert!(evaluate_assertion(&a, 0, "foo bar baz", "").is_ok());
    }

    #[test]
    fn test_stdout_contains_all_fail() {
        let mut a = base_assertion();
        a.stdout_contains_all = Some(vec!["foo".into(), "missing".into()]);
        assert!(evaluate_assertion(&a, 0, "foo bar", "").is_err());
    }

    #[test]
    fn test_stdout_contains_any_pass() {
        let mut a = base_assertion();
        a.stdout_contains_any = Some(vec!["nope".into(), "bar".into()]);
        assert!(evaluate_assertion(&a, 0, "foo bar", "").is_ok());
    }

    #[test]
    fn test_stdout_contains_any_fail() {
        let mut a = base_assertion();
        a.stdout_contains_any = Some(vec!["nope".into(), "missing".into()]);
        assert!(evaluate_assertion(&a, 0, "foo bar", "").is_err());
    }

    #[test]
    fn test_stdout_contains_if_success_skipped_on_failure() {
        let mut a = base_assertion();
        a.stdout_contains_if_success = Some("DRY RUN".into());
        // exit_code != 0, so the assertion is skipped
        assert!(evaluate_assertion(&a, 1, "no match", "").is_ok());
    }

    #[test]
    fn test_stdout_contains_if_success_checked_on_zero() {
        let mut a = base_assertion();
        a.stdout_contains_if_success = Some("DRY RUN".into());
        assert!(evaluate_assertion(&a, 0, "no match", "").is_err());
        assert!(evaluate_assertion(&a, 0, "DRY RUN complete", "").is_ok());
    }

    #[test]
    fn test_stdout_contains_any_if_success_skipped_on_failure() {
        let mut a = base_assertion();
        a.stdout_contains_any_if_success =
            Some(vec!["composefs".into(), "EROFS".into()]);
        assert!(evaluate_assertion(&a, 1, "no match", "").is_ok());
    }

    #[test]
    fn test_stdout_contains_any_if_success_checked_on_zero() {
        let mut a = base_assertion();
        a.stdout_contains_any_if_success =
            Some(vec!["composefs".into(), "EROFS".into()]);
        assert!(evaluate_assertion(&a, 0, "using EROFS", "").is_ok());
        assert!(evaluate_assertion(&a, 0, "no match", "").is_err());
    }
}
