// apps/conary/src/commands/bootstrap/types.rs

/// Options for the `bootstrap run` command.
pub struct BootstrapRunOptions<'a> {
    /// Path to system manifest TOML.
    pub manifest: &'a str,
    /// Working directory for build artifacts.
    pub work_dir: &'a str,
    /// Path to seed directory.
    pub seed: &'a str,
    /// Recipe directory.
    pub recipe_dir: &'a str,
    /// Stop after completing this stage.
    pub up_to: Option<&'a str>,
    /// Only build these packages.
    pub only: Option<&'a [String]>,
    /// Also rebuild reverse dependents of `only` targets.
    pub cascade: bool,
    /// Preserve build logs for successful builds.
    pub keep_logs: bool,
    /// Spawn interactive shell on build failure.
    pub shell_on_failure: bool,
    /// Show verbose build output.
    pub verbose: bool,
    /// Skip remote substituters.
    pub no_substituters: bool,
    /// Auto-publish successful builds.
    pub publish: bool,
}
