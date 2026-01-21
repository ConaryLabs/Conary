// tests/inference_integration.rs
//! Integration tests for capability inference using real system binaries
//!
//! These tests validate the inference module against actual ELF binaries
//! from the system to ensure accuracy in real-world scenarios.

use conary::capability::inference::{
    infer_capabilities, BinaryAnalyzer, HeuristicInferrer, InferenceOptions, PackageFile,
    PackageMetadataRef, WellKnownProfiles,
};
use std::fs;

/// Helper to load a system binary if it exists
fn load_binary(path: &str) -> Option<PackageFile> {
    if std::path::Path::new(path).exists() {
        match fs::read(path) {
            Ok(content) => Some(PackageFile::with_content(path, content)),
            Err(_) => None,
        }
    } else {
        None
    }
}

#[test]
fn test_wellknown_coverage() {
    // Verify we have profiles for common packages
    let common_packages = [
        "nginx",
        "postgresql",
        "redis",
        "curl",
        "git",
        "docker",
        "systemd",
        "openssh-server",
    ];

    for pkg in common_packages {
        assert!(
            WellKnownProfiles::has_profile(pkg),
            "Missing well-known profile for: {}",
            pkg
        );
    }
}

#[test]
fn test_wellknown_nginx_profile() {
    let profile = WellKnownProfiles::lookup("nginx").unwrap();

    // Nginx should listen on 80 and 443
    assert!(
        profile.network.listen_ports.contains(&"80".to_string()),
        "nginx should listen on port 80"
    );
    assert!(
        profile.network.listen_ports.contains(&"443".to_string()),
        "nginx should listen on port 443"
    );

    // Nginx should have network-server syscall profile
    assert_eq!(profile.syscall_profile, Some("network-server".to_string()));

    // Nginx should have high confidence
    assert!(profile.confidence.primary >= conary::capability::inference::Confidence::High);
}

#[test]
fn test_wellknown_curl_profile() {
    let profile = WellKnownProfiles::lookup("curl").unwrap();

    // Curl should have outbound network access
    assert!(
        !profile.network.outbound_ports.is_empty(),
        "curl should have outbound ports"
    );
    assert!(
        profile.network.outbound_ports.contains(&"443".to_string()),
        "curl should have HTTPS outbound"
    );

    // Curl should NOT need to listen
    assert!(
        profile.network.listen_ports.is_empty(),
        "curl should not listen on ports"
    );
}

#[test]
fn test_heuristic_inference_network_server() {
    // Simulate a network server package
    let files = vec![
        PackageFile::new("/usr/sbin/myserver"),
        PackageFile::new("/etc/myserver/config.conf"),
        PackageFile::new("/var/log/myserver/server.log"),
        PackageFile::new("/var/lib/myserver/data"),
    ];

    let metadata = PackageMetadataRef {
        name: "myserver-server".to_string(),
        version: "1.0.0".to_string(),
        dependencies: vec!["libssl3".to_string(), "libc6".to_string()],
        ..Default::default()
    };

    let result = HeuristicInferrer::infer(&files, &metadata).unwrap();

    // Should detect server nature from name
    assert!(
        !result.network.no_network,
        "Server should need network access"
    );

    // Should detect config directory
    assert!(
        result.filesystem.read_paths.contains(&"/etc/myserver".to_string()),
        "Should detect config dir"
    );

    // Should detect log directory
    assert!(
        result.filesystem.write_paths.contains(&"/var/log/myserver".to_string()),
        "Should detect log dir"
    );
}

#[test]
fn test_heuristic_inference_cli_tool() {
    // Simulate a simple CLI tool package
    let files = vec![
        PackageFile::new("/usr/bin/mytool"),
        PackageFile::new("/usr/share/man/man1/mytool.1.gz"),
    ];

    let metadata = PackageMetadataRef {
        name: "mytool".to_string(),
        version: "1.0.0".to_string(),
        dependencies: vec!["libc6".to_string()],
        ..Default::default()
    };

    let result = HeuristicInferrer::infer(&files, &metadata).unwrap();

    // CLI tool without network deps should likely have no_network=true
    // (though confidence is low since we can't be certain)
    assert!(
        result.network.listen_ports.is_empty(),
        "CLI tool should not listen on ports"
    );
}

#[test]
fn test_binary_analysis_curl() {
    // Skip if curl not available
    let Some(curl_file) = load_binary("/usr/bin/curl") else {
        eprintln!("Skipping test: /usr/bin/curl not found");
        return;
    };

    let files = vec![&curl_file];
    let result = BinaryAnalyzer::analyze_all(&files).unwrap();

    // Curl should link against network libraries
    assert!(
        !result.network.no_network,
        "curl should need network access based on binary analysis"
    );

    println!("curl binary analysis result:");
    println!("  Network: no_network={}", result.network.no_network);
    println!("  Outbound ports: {:?}", result.network.outbound_ports);
    println!("  Syscall profile: {:?}", result.syscall_profile);
    println!("  Confidence: {:?}", result.confidence.primary);
}

#[test]
fn test_binary_analysis_ls() {
    // Skip if ls not available
    let Some(ls_file) = load_binary("/usr/bin/ls") else {
        eprintln!("Skipping test: /usr/bin/ls not found");
        return;
    };

    let files = vec![&ls_file];
    let result = BinaryAnalyzer::analyze_all(&files).unwrap();

    // ls should NOT need network
    assert!(
        result.network.outbound_ports.is_empty(),
        "ls should not have outbound ports"
    );
    assert!(
        result.network.listen_ports.is_empty(),
        "ls should not listen on ports"
    );

    println!("ls binary analysis result:");
    println!("  Network: no_network={}", result.network.no_network);
    println!("  Syscall profile: {:?}", result.syscall_profile);
}

#[test]
fn test_full_inference_pipeline() {
    // Test the full inference pipeline with nginx-like package
    let files = vec![
        PackageFile::new("/usr/sbin/nginx"),
        PackageFile::new("/etc/nginx/nginx.conf"),
        PackageFile::new("/var/log/nginx/access.log"),
        PackageFile::new("/usr/share/nginx/html/index.html"),
    ];

    let metadata = PackageMetadataRef {
        name: "nginx".to_string(),
        version: "1.24.0".to_string(),
        dependencies: vec!["libssl3".to_string(), "libpcre2-8".to_string()],
        ..Default::default()
    };

    // Use fast options (tiers 1-2 only)
    let options = InferenceOptions::fast();
    let result = infer_capabilities(&files, &metadata, &options).unwrap();

    // Should get well-known profile for nginx
    assert_eq!(result.tier_used, 1, "Should use tier 1 (well-known) for nginx");
    assert!(
        result.network.listen_ports.contains(&"80".to_string()),
        "nginx should listen on port 80"
    );
    assert!(
        result.network.listen_ports.contains(&"443".to_string()),
        "nginx should listen on port 443"
    );

    // Convert to declaration and verify
    let decl = result.to_declaration();
    assert!(!decl.network.listen.is_empty(), "Declaration should have listen ports");
    assert!(decl.rationale.is_some(), "Declaration should have rationale");
}

#[test]
fn test_unknown_package_inference() {
    // Test inference for an unknown package (no well-known profile)
    let files = vec![
        PackageFile::new("/usr/bin/obscure-tool"),
        PackageFile::new("/etc/obscure-tool/config.ini"),
    ];

    let metadata = PackageMetadataRef {
        name: "obscure-tool".to_string(),
        version: "0.1.0".to_string(),
        dependencies: vec!["libc6".to_string()],
        ..Default::default()
    };

    let options = InferenceOptions::default();
    let result = infer_capabilities(&files, &metadata, &options).unwrap();

    // Should use heuristics (tier 2) for unknown package
    assert!(result.tier_used >= 2, "Should use heuristics for unknown package");

    // Should detect config directory
    assert!(
        result.filesystem.read_paths.contains(&"/etc/obscure-tool".to_string()),
        "Should detect config directory from file paths"
    );
}

#[test]
fn test_systemd_service_analysis() {
    // Test heuristic analysis of systemd service content
    let service_content = br#"
[Unit]
Description=My Network Service
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/usr/bin/myservice --port 8080
ListenStream=8080
User=myservice

[Install]
WantedBy=multi-user.target
"#;

    let files = vec![
        PackageFile::new("/usr/bin/myservice"),
        PackageFile::with_content(
            "/usr/lib/systemd/system/myservice.service",
            service_content.to_vec(),
        ),
    ];

    let metadata = PackageMetadataRef {
        name: "myservice".to_string(),
        version: "1.0.0".to_string(),
        dependencies: vec![],
        ..Default::default()
    };

    let result = HeuristicInferrer::infer(&files, &metadata).unwrap();

    // Should detect network usage from systemd service
    assert!(
        !result.network.no_network,
        "Should detect network requirement from systemd service"
    );

    // Should detect port 8080 from ListenStream
    assert!(
        result.network.listen_ports.contains(&"8080".to_string()),
        "Should detect port 8080 from systemd service"
    );
}

#[test]
fn test_confidence_levels() {
    use conary::capability::inference::Confidence;

    // Well-known profiles should have high confidence
    let nginx = WellKnownProfiles::lookup("nginx").unwrap();
    assert!(
        nginx.confidence.primary >= Confidence::High,
        "Well-known profiles should have high confidence"
    );

    // Heuristic inference should have medium confidence
    let files = vec![PackageFile::new("/usr/bin/test")];
    let metadata = PackageMetadataRef {
        name: "test-pkg".to_string(),
        version: "1.0.0".to_string(),
        dependencies: vec![],
        ..Default::default()
    };
    let result = HeuristicInferrer::infer(&files, &metadata).unwrap();
    assert!(
        result.confidence.primary <= Confidence::High,
        "Heuristic inference should not exceed high confidence without strong evidence"
    );
}

#[test]
fn test_merge_capabilities() {
    use conary::capability::inference::{Confidence, InferredCapabilities};

    // Test 1: When both have same confidence, ports are merged
    let mut base = InferredCapabilities::default();
    base.network.listen_ports.push("80".to_string());
    base.network.confidence = Confidence::Medium;

    let mut other = InferredCapabilities::default();
    other.network.listen_ports.push("443".to_string());
    other.network.confidence = Confidence::Medium; // Same confidence

    base.merge(&other);

    // Should have both ports when confidences are equal
    assert!(
        base.network.listen_ports.contains(&"80".to_string()),
        "Should keep original port 80"
    );
    assert!(
        base.network.listen_ports.contains(&"443".to_string()),
        "Should add new port 443"
    );

    // Test 2: When other has higher confidence, it replaces network
    let mut base2 = InferredCapabilities::default();
    base2.network.listen_ports.push("80".to_string());
    base2.network.confidence = Confidence::Low;

    let mut other2 = InferredCapabilities::default();
    other2.network.listen_ports.push("443".to_string());
    other2.network.confidence = Confidence::High;

    base2.merge(&other2);

    // When other has higher confidence, it replaces the network entirely
    assert!(
        base2.network.listen_ports.contains(&"443".to_string()),
        "Should have port from higher-confidence source"
    );
    // Port 80 is lost because the higher-confidence result takes precedence
}
