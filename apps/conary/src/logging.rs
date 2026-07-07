// apps/conary/src/logging.rs
//! CLI logging helpers.

/// Map the global `--quiet` / `--verbose` flags to a `tracing` EnvFilter
/// directive. `RUST_LOG`, when set, overrides this at init time.
pub fn verbosity_directive(quiet: bool, verbose: u8) -> &'static str {
    if quiet {
        return "error";
    }

    match verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_maps_to_error() {
        assert_eq!(verbosity_directive(true, 0), "error");
    }

    #[test]
    fn default_is_warn() {
        assert_eq!(verbosity_directive(false, 0), "warn");
    }

    #[test]
    fn verbose_counts_escalate() {
        assert_eq!(verbosity_directive(false, 1), "info");
        assert_eq!(verbosity_directive(false, 2), "debug");
        assert_eq!(verbosity_directive(false, 3), "trace");
        assert_eq!(verbosity_directive(false, 9), "trace");
    }
}
