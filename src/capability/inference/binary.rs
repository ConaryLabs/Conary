// src/capability/inference/binary.rs
//! ELF binary analysis for capability inference
//!
//! This module implements Tier 4 inference using goblin to analyze ELF binaries.
//! It extracts:
//! - Linked libraries (network, database, GUI dependencies)
//! - Imported symbols (socket calls, file operations)
//! - Section hints (presence of certain sections)
//!
//! Binary analysis is the slowest but most accurate tier.

use super::confidence::{Confidence, ConfidenceBuilder};
use super::error::InferenceError;
use super::{InferenceResult, InferenceSource, InferredCapabilities, PackageFile};
use goblin::elf::Elf;
use goblin::Object;
use std::collections::HashSet;

/// ELF binary analyzer using goblin
pub struct BinaryAnalyzer;

impl BinaryAnalyzer {
    /// Analyze multiple executables and combine results
    pub fn analyze_all(files: &[&PackageFile]) -> InferenceResult<InferredCapabilities> {
        let mut combined = InferredCapabilities {
            source: InferenceSource::BinaryAnalysis,
            tier_used: 4,
            // Default to no network when we have no evidence
            network: super::InferredNetwork {
                no_network: true,
                ..Default::default()
            },
            ..Default::default()
        };

        let mut all_libs = HashSet::new();
        let mut all_symbols = HashSet::new();
        let mut confidence_builder = ConfidenceBuilder::new();

        for file in files {
            if let Some(ref content) = file.content {
                match Self::analyze_binary(content) {
                    Ok(analysis) => {
                        all_libs.extend(analysis.libraries);
                        all_symbols.extend(analysis.symbols);

                        // Merge network findings
                        if analysis.uses_sockets {
                            combined.network.no_network = false;
                            confidence_builder
                                .add_network_evidence(&file.path, Confidence::High);
                        }

                        // Merge filesystem findings
                        for path in analysis.filesystem_hints {
                            if path.contains("log") || path.contains("cache") || path.contains("tmp") {
                                if !combined.filesystem.write_paths.contains(&path) {
                                    combined.filesystem.write_paths.push(path);
                                }
                            } else if !combined.filesystem.read_paths.contains(&path) {
                                combined.filesystem.read_paths.push(path);
                            }
                        }
                    }
                    Err(e) => {
                        // Log but continue - some binaries may fail to parse
                        tracing::debug!("Failed to analyze {}: {}", file.path, e);
                    }
                }
            }
        }

        // Infer from collected libraries
        let lib_hints = analyze_libraries(&all_libs);

        if lib_hints.has_network {
            combined.network.no_network = false;
            confidence_builder.add_network_evidence("Links against network libraries", Confidence::High);
        }

        if lib_hints.has_ssl {
            combined.network.outbound_ports.push("443".to_string());
            confidence_builder.add_network_evidence("Links against SSL/TLS libraries", Confidence::High);
        }

        if lib_hints.has_database {
            confidence_builder
                .add_network_evidence("Links against database libraries", Confidence::Medium);
            // Add common database ports based on library
            if all_libs.iter().any(|l| l.contains("pq")) {
                combined.network.outbound_ports.push("5432".to_string());
            }
            if all_libs.iter().any(|l| l.contains("mysql") || l.contains("mariadb")) {
                combined.network.outbound_ports.push("3306".to_string());
            }
        }

        if lib_hints.has_gui {
            combined.syscall_profile = Some("gui-app".to_string());
            confidence_builder.add_syscall_evidence("Links against GUI libraries", Confidence::High);
        }

        // Infer from collected symbols
        let symbol_hints = analyze_symbols(&all_symbols);

        if symbol_hints.uses_sockets {
            combined.network.no_network = false;
            confidence_builder
                .add_network_evidence("Uses socket system calls", Confidence::High);
        }

        if symbol_hints.uses_privileged {
            if combined.syscall_profile.is_none() {
                combined.syscall_profile = Some("system-daemon".to_string());
            }
            confidence_builder
                .add_syscall_evidence("Uses privileged system calls", Confidence::High);
        }

        if symbol_hints.uses_exec {
            combined
                .filesystem
                .execute_paths
                .push("/usr/bin/*".to_string());
            confidence_builder
                .add_filesystem_evidence("Uses exec system calls", Confidence::Medium);
        }

        // Set confidence
        combined.confidence = confidence_builder.build();

        // Set network confidence based on evidence
        combined.network.confidence = if combined.network.no_network {
            Confidence::Medium // We're guessing it doesn't need network
        } else {
            Confidence::High // We have positive evidence
        };

        combined.filesystem.confidence = if combined.filesystem.read_paths.is_empty()
            && combined.filesystem.write_paths.is_empty()
        {
            Confidence::Low
        } else {
            Confidence::High
        };

        // Generate rationale
        combined.rationale = format!(
            "Binary analysis of {} file(s): {} libraries, {} symbols analyzed",
            files.len(),
            all_libs.len(),
            all_symbols.len()
        );

        Ok(combined)
    }

    /// Analyze a single binary
    fn analyze_binary(content: &[u8]) -> InferenceResult<BinaryAnalysis> {
        let mut analysis = BinaryAnalysis::default();

        match Object::parse(content) {
            Ok(Object::Elf(elf)) => {
                Self::analyze_elf(&elf, &mut analysis)?;
            }
            Ok(Object::Mach(_)) => {
                // macOS binaries - not supported on Linux package manager
                return Err(InferenceError::UnsupportedFormat {
                    path: "binary".to_string(),
                    format: "Mach-O".to_string(),
                });
            }
            Ok(Object::PE(_)) => {
                return Err(InferenceError::UnsupportedFormat {
                    path: "binary".to_string(),
                    format: "PE".to_string(),
                });
            }
            Ok(Object::Archive(_)) => {
                return Err(InferenceError::UnsupportedFormat {
                    path: "binary".to_string(),
                    format: "Archive".to_string(),
                });
            }
            Ok(Object::Unknown(_)) | Ok(_) => {
                return Err(InferenceError::UnsupportedFormat {
                    path: "binary".to_string(),
                    format: "Unknown".to_string(),
                });
            }
            Err(e) => {
                return Err(InferenceError::BinaryParseError {
                    path: "binary".to_string(),
                    reason: e.to_string(),
                });
            }
        }

        Ok(analysis)
    }

    /// Analyze an ELF binary
    fn analyze_elf(elf: &Elf, analysis: &mut BinaryAnalysis) -> InferenceResult<()> {
        // Extract dynamic libraries
        for lib in &elf.libraries {
            analysis.libraries.insert((*lib).to_string());
        }

        // Extract imported symbols from dynamic symbols
        for sym in &elf.dynsyms {
            if sym.is_import()
                && let Some(name) = elf.dynstrtab.get_at(sym.st_name)
            {
                analysis.symbols.insert(name.to_string());

                // Check for specific system calls
                match name {
                    "socket" | "bind" | "listen" | "accept" | "connect" | "send" | "recv"
                    | "sendto" | "recvfrom" | "getaddrinfo" | "gethostbyname" => {
                        analysis.uses_sockets = true;
                    }
                    "setuid" | "setgid" | "setreuid" | "setregid" | "seteuid" | "setegid"
                    | "cap_set_proc" | "prctl" => {
                        analysis.uses_privileged = true;
                    }
                    "execve" | "execl" | "execv" | "execle" | "execvp" | "execlp"
                    | "posix_spawn" | "system" | "popen" => {
                        analysis.uses_exec = true;
                    }
                    _ => {}
                }
            }
        }

        // Look for interesting sections
        for section in &elf.section_headers {
            if let Some(name) = elf.shdr_strtab.get_at(section.sh_name) {
                // .rodata might contain paths
                if name == ".rodata" && section.sh_size > 0 && section.sh_size < 1_000_000 {
                    // We'd need the actual binary content to extract strings
                    // For now, just note that rodata exists
                }
            }
        }

        Ok(())
    }
}

/// Result of analyzing a single binary
#[derive(Debug, Default)]
struct BinaryAnalysis {
    libraries: HashSet<String>,
    symbols: HashSet<String>,
    uses_sockets: bool,
    uses_privileged: bool,
    uses_exec: bool,
    filesystem_hints: Vec<String>,
}

/// Library analysis hints
struct LibraryHints {
    has_network: bool,
    has_ssl: bool,
    has_database: bool,
    has_gui: bool,
}

/// Analyze library dependencies
fn analyze_libraries(libs: &HashSet<String>) -> LibraryHints {
    let mut hints = LibraryHints {
        has_network: false,
        has_ssl: false,
        has_database: false,
        has_gui: false,
    };

    for lib in libs {
        let lower = lib.to_lowercase();

        // Network libraries
        if lower.contains("curl")
            || lower.contains("http")
            || lower.contains("socket")
            || lower.contains("nghttp")
        {
            hints.has_network = true;
        }

        // SSL/TLS
        if lower.contains("ssl") || lower.contains("tls") || lower.contains("crypto") {
            hints.has_ssl = true;
            hints.has_network = true;
        }

        // Database libraries
        if lower.contains("pq")
            || lower.contains("mysql")
            || lower.contains("sqlite")
            || lower.contains("mariadb")
            || lower.contains("odbc")
        {
            hints.has_database = true;
        }

        // GUI libraries
        if lower.contains("gtk")
            || lower.contains("qt")
            || lower.starts_with("libx")
            || lower.contains("wayland")
            || lower.contains("xcb")
        {
            hints.has_gui = true;
        }
    }

    hints
}

/// Symbol analysis hints
struct SymbolHints {
    uses_sockets: bool,
    uses_privileged: bool,
    uses_exec: bool,
}

/// Analyze imported symbols
fn analyze_symbols(symbols: &HashSet<String>) -> SymbolHints {
    let mut hints = SymbolHints {
        uses_sockets: false,
        uses_privileged: false,
        uses_exec: false,
    };

    // Socket-related symbols
    let socket_symbols = [
        "socket",
        "bind",
        "listen",
        "accept",
        "accept4",
        "connect",
        "send",
        "recv",
        "sendto",
        "recvfrom",
        "sendmsg",
        "recvmsg",
        "getaddrinfo",
        "gethostbyname",
        "gethostbyaddr",
        "getpeername",
        "getsockname",
        "setsockopt",
        "getsockopt",
    ];

    // Privileged operation symbols
    let privileged_symbols = [
        "setuid",
        "setgid",
        "setreuid",
        "setregid",
        "seteuid",
        "setegid",
        "setresuid",
        "setresgid",
        "cap_set_proc",
        "cap_get_proc",
        "prctl",
        "chroot",
        "pivot_root",
        "mount",
        "umount",
        "unshare",
        "clone",
        "ioctl",
        "mknod",
    ];

    // Exec symbols
    let exec_symbols = [
        "execve",
        "execl",
        "execle",
        "execlp",
        "execv",
        "execvp",
        "execvpe",
        "fexecve",
        "posix_spawn",
        "posix_spawnp",
        "system",
        "popen",
        "fork",
        "vfork",
    ];

    for sym in symbols {
        if socket_symbols.contains(&sym.as_str()) {
            hints.uses_sockets = true;
        }
        if privileged_symbols.contains(&sym.as_str()) {
            hints.uses_privileged = true;
        }
        if exec_symbols.contains(&sym.as_str()) {
            hints.uses_exec = true;
        }
    }

    hints
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_analyze_libraries() {
        let mut libs = HashSet::new();
        libs.insert("libssl.so.3".to_string());
        libs.insert("libcurl.so.4".to_string());
        libs.insert("libpq.so.5".to_string());

        let hints = analyze_libraries(&libs);
        assert!(hints.has_network);
        assert!(hints.has_ssl);
        assert!(hints.has_database);
        assert!(!hints.has_gui);
    }

    #[test]
    fn test_analyze_symbols() {
        let mut symbols = HashSet::new();
        symbols.insert("socket".to_string());
        symbols.insert("connect".to_string());
        symbols.insert("fork".to_string());

        let hints = analyze_symbols(&symbols);
        assert!(hints.uses_sockets);
        assert!(!hints.uses_privileged);
        assert!(hints.uses_exec);
    }

    #[test]
    fn test_analyze_gui_libs() {
        let mut libs = HashSet::new();
        libs.insert("libgtk-3.so.0".to_string());
        libs.insert("libX11.so.6".to_string());

        let hints = analyze_libraries(&libs);
        assert!(hints.has_gui);
    }

    // Integration test with a real binary would go here
    // but requires test fixtures
    #[test]
    fn test_empty_binary_analysis() {
        let result = BinaryAnalyzer::analyze_all(&[]);
        assert!(result.is_ok());
        let caps = result.unwrap();
        assert!(caps.network.no_network); // Default assumption
    }
}
