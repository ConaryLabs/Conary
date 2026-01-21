// src/capability/inference/heuristics.rs
//! Heuristic-based capability inference
//!
//! This module implements Tier 2 inference using rule-based heuristics:
//! - File path patterns (e.g., /usr/sbin suggests daemon)
//! - Dependency analysis (e.g., depends on libssl suggests network)
//! - Package name patterns (e.g., ends with "-server" suggests listens)
//! - Systemd service file analysis
//!
//! Heuristics are faster than binary analysis but less precise.

use super::confidence::{Confidence, ConfidenceBuilder};
use super::{
    InferenceResult, InferenceSource, InferredCapabilities, InferredFilesystem, InferredNetwork,
    PackageFile, PackageMetadataRef,
};
use regex::Regex;
use std::sync::LazyLock;

/// Rule-based heuristic inferrer
pub struct HeuristicInferrer;

impl HeuristicInferrer {
    /// Infer capabilities using heuristic rules
    pub fn infer(
        files: &[PackageFile],
        metadata: &PackageMetadataRef,
    ) -> InferenceResult<InferredCapabilities> {
        let mut confidence_builder = ConfidenceBuilder::new();
        let mut network = InferredNetwork::default();
        let mut filesystem = InferredFilesystem::default();
        let mut syscall_profile: Option<String> = None;
        let mut rationale_parts = Vec::new();

        // Analyze package name
        let name_hints = analyze_package_name(&metadata.name);
        if name_hints.is_server {
            syscall_profile = Some("network-server".to_string());
            confidence_builder
                .add_network_evidence("Package name suggests server", Confidence::Medium);
            rationale_parts.push(format!(
                "Package name '{}' suggests network server",
                metadata.name
            ));
        }

        // Analyze file paths
        let path_analysis = analyze_file_paths(files);

        if path_analysis.has_sbin_executables {
            if syscall_profile.is_none() {
                syscall_profile = Some("system-daemon".to_string());
            }
            confidence_builder
                .add_syscall_evidence("Has /sbin or /usr/sbin executables", Confidence::Medium);
            rationale_parts.push("Contains system binaries (sbin)".to_string());
        }

        if !path_analysis.config_dirs.is_empty() {
            for dir in &path_analysis.config_dirs {
                if !filesystem.read_paths.contains(dir) {
                    filesystem.read_paths.push(dir.clone());
                }
            }
            confidence_builder
                .add_filesystem_evidence("Has configuration directories", Confidence::High);
        }

        if !path_analysis.log_paths.is_empty() {
            for path in &path_analysis.log_paths {
                if !filesystem.write_paths.contains(path) {
                    filesystem.write_paths.push(path.clone());
                }
            }
            confidence_builder.add_filesystem_evidence("Has log directories", Confidence::High);
        }

        if !path_analysis.var_lib_paths.is_empty() {
            for path in &path_analysis.var_lib_paths {
                if !filesystem.write_paths.contains(path) {
                    filesystem.write_paths.push(path.clone());
                }
            }
            confidence_builder
                .add_filesystem_evidence("Has /var/lib data directories", Confidence::High);
        }

        // Analyze systemd service files
        for file in files.iter().filter(|f| f.is_systemd_service()) {
            if let Some(ref content) = file.content
                && let Ok(text) = std::str::from_utf8(content)
            {
                let service_analysis = analyze_systemd_service(text);

                if service_analysis.has_network {
                    network.no_network = false;
                    confidence_builder.add_network_evidence(
                        "Systemd service uses network",
                        Confidence::High,
                    );
                }

                if !service_analysis.ports.is_empty() {
                    for port in service_analysis.ports {
                        if !network.listen_ports.contains(&port) {
                            network.listen_ports.push(port);
                        }
                    }
                    confidence_builder
                        .add_network_evidence("Systemd service specifies ports", Confidence::High);
                }

                if service_analysis.is_daemon {
                    if syscall_profile.is_none() {
                        syscall_profile = Some("system-daemon".to_string());
                    }
                    rationale_parts.push("Systemd service file found".to_string());
                }
            }
        }

        // Analyze dependencies
        let dep_hints = analyze_dependencies(&metadata.dependencies);

        if dep_hints.has_network_libs {
            network.no_network = false;
            if dep_hints.has_ssl {
                network.outbound_ports.push("443".to_string());
            }
            confidence_builder.add_network_evidence(
                "Dependencies include networking libraries",
                Confidence::Medium,
            );
        }

        if dep_hints.has_database_libs {
            // Common database ports
            if metadata.dependencies.iter().any(|d| d.contains("pq") || d.contains("postgres")) {
                network.outbound_ports.push("5432".to_string());
            }
            if metadata.dependencies.iter().any(|d| d.contains("mysql")) {
                network.outbound_ports.push("3306".to_string());
            }
            confidence_builder.add_network_evidence(
                "Dependencies include database libraries",
                Confidence::Medium,
            );
        }

        if dep_hints.has_gui_libs {
            syscall_profile = Some("gui-app".to_string());
            confidence_builder
                .add_syscall_evidence("Dependencies include GUI libraries", Confidence::High);
        }

        // Set no_network if we found no network evidence
        if network.listen_ports.is_empty()
            && network.outbound_ports.is_empty()
            && !dep_hints.has_network_libs
            && !name_hints.is_server
        {
            network.no_network = true;
            network.confidence = Confidence::Low; // Not sure, just no evidence
        } else {
            network.confidence = Confidence::Medium;
        }

        filesystem.confidence = if filesystem.read_paths.is_empty() && filesystem.write_paths.is_empty()
        {
            Confidence::Low
        } else {
            Confidence::Medium
        };

        let confidence = confidence_builder.build();

        Ok(InferredCapabilities {
            network,
            filesystem,
            syscall_profile,
            confidence,
            tier_used: 2,
            rationale: if rationale_parts.is_empty() {
                "Heuristic analysis found no strong indicators".to_string()
            } else {
                rationale_parts.join("; ")
            },
            source: InferenceSource::Heuristic,
        })
    }
}

/// Hints derived from package name
#[allow(dead_code)] // Fields reserved for future use
struct NameHints {
    is_server: bool,
    is_client: bool,
    is_lib: bool,
    is_dev: bool,
}

/// Analyze package name for hints
fn analyze_package_name(name: &str) -> NameHints {
    let lower = name.to_lowercase();

    NameHints {
        is_server: lower.ends_with("-server")
            || lower.ends_with("d") && !lower.ends_with("lib")
            || lower.contains("daemon")
            || lower.contains("service"),
        is_client: lower.ends_with("-client") || lower.ends_with("-cli"),
        is_lib: lower.starts_with("lib") || lower.ends_with("-libs"),
        is_dev: lower.ends_with("-dev") || lower.ends_with("-devel"),
    }
}

/// Path analysis results
struct PathAnalysis {
    has_sbin_executables: bool,
    config_dirs: Vec<String>,
    log_paths: Vec<String>,
    var_lib_paths: Vec<String>,
}

/// Analyze file paths for capability hints
fn analyze_file_paths(files: &[PackageFile]) -> PathAnalysis {
    let mut result = PathAnalysis {
        has_sbin_executables: false,
        config_dirs: Vec::new(),
        log_paths: Vec::new(),
        var_lib_paths: Vec::new(),
    };

    // Regex patterns
    static CONFIG_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^/etc/([^/]+)").unwrap());
    static LOG_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^/var/log/([^/]+)").unwrap());
    static VAR_LIB_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^/var/lib/([^/]+)").unwrap());

    for file in files {
        // Check for sbin executables
        if file.path.starts_with("/sbin/") || file.path.starts_with("/usr/sbin/") {
            result.has_sbin_executables = true;
        }

        // Extract config directories
        if let Some(caps) = CONFIG_RE.captures(&file.path) {
            let dir = format!("/etc/{}", caps.get(1).unwrap().as_str());
            if !result.config_dirs.contains(&dir) {
                result.config_dirs.push(dir);
            }
        }

        // Extract log paths
        if let Some(caps) = LOG_RE.captures(&file.path) {
            let path = format!("/var/log/{}", caps.get(1).unwrap().as_str());
            if !result.log_paths.contains(&path) {
                result.log_paths.push(path);
            }
        }

        // Extract var/lib paths
        if let Some(caps) = VAR_LIB_RE.captures(&file.path) {
            let path = format!("/var/lib/{}", caps.get(1).unwrap().as_str());
            if !result.var_lib_paths.contains(&path) {
                result.var_lib_paths.push(path);
            }
        }
    }

    result
}

/// Systemd service analysis results
struct ServiceAnalysis {
    is_daemon: bool,
    has_network: bool,
    ports: Vec<String>,
}

/// Analyze systemd service file content
fn analyze_systemd_service(content: &str) -> ServiceAnalysis {
    let mut result = ServiceAnalysis {
        is_daemon: true, // It's a service file, so it's a daemon
        has_network: false,
        ports: Vec::new(),
    };

    // Check for network-related directives
    static NETWORK_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)(After|Wants|Requires)=.*network").unwrap()
    });

    // Match systemd socket directives: ListenStream, ListenDatagram, ListenSequentialPacket
    // as well as general Listen= and Port= patterns
    static PORT_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)(?:Listen(?:Stream|Datagram|SequentialPacket)?|Port)[=:]?\s*(\d{1,5})").unwrap());

    static PRIVATE_NETWORK_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"PrivateNetwork\s*=\s*true").unwrap());

    if NETWORK_RE.is_match(content) && !PRIVATE_NETWORK_RE.is_match(content) {
        result.has_network = true;
    }

    // Extract ports
    for caps in PORT_RE.captures_iter(content) {
        if let Some(port) = caps.get(1) {
            let port_str = port.as_str().to_string();
            if !result.ports.contains(&port_str) {
                result.ports.push(port_str);
            }
        }
    }

    result
}

/// Dependency analysis hints
struct DependencyHints {
    has_network_libs: bool,
    has_ssl: bool,
    has_database_libs: bool,
    has_gui_libs: bool,
}

/// Analyze dependencies for capability hints
fn analyze_dependencies(deps: &[String]) -> DependencyHints {
    let mut hints = DependencyHints {
        has_network_libs: false,
        has_ssl: false,
        has_database_libs: false,
        has_gui_libs: false,
    };

    for dep in deps {
        let lower = dep.to_lowercase();

        // Network libraries
        if lower.contains("curl")
            || lower.contains("http")
            || lower.contains("socket")
            || lower.contains("net")
            || lower.contains("network")
        {
            hints.has_network_libs = true;
        }

        // SSL/TLS
        if lower.contains("ssl") || lower.contains("tls") || lower.contains("crypto") {
            hints.has_ssl = true;
            hints.has_network_libs = true; // SSL implies network
        }

        // Database libraries
        if lower.contains("pq")
            || lower.contains("postgres")
            || lower.contains("mysql")
            || lower.contains("sqlite")
            || lower.contains("mariadb")
            || lower.contains("odbc")
        {
            hints.has_database_libs = true;
        }

        // GUI libraries
        if lower.contains("gtk")
            || lower.contains("qt")
            || lower.contains("x11")
            || lower.contains("wayland")
            || lower.contains("xcb")
        {
            hints.has_gui_libs = true;
        }
    }

    hints
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_analyze_package_name() {
        let hints = analyze_package_name("nginx-server");
        assert!(hints.is_server);
        assert!(!hints.is_client);

        let hints = analyze_package_name("postgresql-client");
        assert!(!hints.is_server);
        assert!(hints.is_client);

        let hints = analyze_package_name("libssl-dev");
        assert!(hints.is_lib);
        assert!(hints.is_dev);

        let hints = analyze_package_name("sshd");
        assert!(hints.is_server); // ends with 'd'
    }

    #[test]
    fn test_analyze_file_paths() {
        let files = vec![
            PackageFile::new("/usr/sbin/nginx"),
            PackageFile::new("/etc/nginx/nginx.conf"),
            PackageFile::new("/var/log/nginx/access.log"),
            PackageFile::new("/var/lib/nginx/cache"),
        ];

        let analysis = analyze_file_paths(&files);
        assert!(analysis.has_sbin_executables);
        assert!(analysis.config_dirs.contains(&"/etc/nginx".to_string()));
        assert!(analysis.log_paths.contains(&"/var/log/nginx".to_string()));
        assert!(analysis.var_lib_paths.contains(&"/var/lib/nginx".to_string()));
    }

    #[test]
    fn test_analyze_systemd_service() {
        let content = r#"
[Unit]
Description=The NGINX HTTP and reverse proxy server
After=syslog.target network-online.target remote-fs.target nss-lookup.target

[Service]
Type=forking
ExecStart=/usr/sbin/nginx
ListenStream=80

[Install]
WantedBy=multi-user.target
"#;

        let analysis = analyze_systemd_service(content);
        assert!(analysis.is_daemon);
        assert!(analysis.has_network);
        assert!(analysis.ports.contains(&"80".to_string()));
    }

    #[test]
    fn test_analyze_dependencies() {
        let deps = vec![
            "libssl3".to_string(),
            "libcurl4".to_string(),
            "libpq5".to_string(),
        ];

        let hints = analyze_dependencies(&deps);
        assert!(hints.has_network_libs);
        assert!(hints.has_ssl);
        assert!(hints.has_database_libs);
        assert!(!hints.has_gui_libs);
    }

    #[test]
    fn test_heuristic_inference() {
        let files = vec![
            PackageFile::new("/usr/sbin/myservice"),
            PackageFile::new("/etc/myservice/config.conf"),
            PackageFile::new("/var/log/myservice/service.log"),
        ];

        let metadata = PackageMetadataRef {
            name: "myservice-server".to_string(),
            version: "1.0.0".to_string(),
            dependencies: vec!["libssl3".to_string()],
            ..Default::default()
        };

        let result = HeuristicInferrer::infer(&files, &metadata).unwrap();
        assert_eq!(result.source, InferenceSource::Heuristic);
        assert_eq!(result.tier_used, 2);
        assert!(!result.network.no_network);
        assert!(result.filesystem.read_paths.contains(&"/etc/myservice".to_string()));
    }
}
