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
    if let Some(ref needle) = assertion.stderr_contains
        && !stderr.contains(needle.as_str())
    {
        bail!("stderr does not contain \"{needle}\"");
    }
    Ok(())
}
